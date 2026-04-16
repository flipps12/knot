// src/network/swarm.rs

use libp2p::{
    Multiaddr,
    PeerId,
    SwarmBuilder,
    Transport,
    core::upgrade,
    identify,
    identity,
    kad,
    mdns,
    noise,
    ping,
    relay,
    request_response,
    swarm::{ NetworkBehaviour, SwarmEvent },
    tcp,
    yamux,
};
use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;
use std::{ error::Error, fs, path::PathBuf };
use std::time::Duration;
use std::collections::HashMap;
use bytes::{ Bytes, BytesMut };
use serde::{ Deserialize, Serialize };

use crate::{ KnotMessage, utils::tou64::peer_id_to_u64 };
use crate::utils::framing::BinaryFrame;

#[derive(Debug, Clone)]
pub struct FrameRequest {
    pub raw: Bytes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameResponse {
    pub ok: bool,
}

pub enum NetworkCommand {
    SendFrame {
        target_u64: u64,
        frame: Bytes,
    },
    GetPeers,
    DialAddress(libp2p::Multiaddr),
    LookupPeer(libp2p::PeerId),
    ConnectRelay {
        relay_addr: Multiaddr,
        relay_peer_id: PeerId,
    },
}

#[derive(Debug)]
pub enum NetworkResponse {
    PeersList(Vec<(libp2p::PeerId, Vec<libp2p::Multiaddr>)>),
    CommandAccepted,
}

#[derive(Debug, Clone, Default)]
pub struct FrameCodec;

#[async_trait]
impl request_response::Codec for FrameCodec {
    type Protocol = String;
    type Request = FrameRequest;
    type Response = FrameResponse;

    async fn read_request<T>(&mut self, _: &String, io: &mut T) -> std::io::Result<Self::Request>
        where T: futures::AsyncRead + Unpin + Send
    {
        use futures::AsyncReadExt;
        let mut len_buf = [0u8; 4];
        io.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;

        let mut buf = BytesMut::with_capacity(len);
        buf.resize(len, 0);

        io.read_exact(&mut buf).await?;
        Ok(FrameRequest { raw: Bytes::from(buf) })
    }

    async fn read_response<T>(&mut self, _: &String, io: &mut T) -> std::io::Result<Self::Response>
        where T: futures::AsyncRead + Unpin + Send
    {
        use futures::AsyncReadExt;
        let mut buf = [0u8; 1];
        io.read_exact(&mut buf).await?;
        Ok(FrameResponse { ok: buf[0] == 1 })
    }

    async fn write_request<T>(
        &mut self,
        _: &String,
        io: &mut T,
        req: Self::Request
    ) -> std::io::Result<()>
        where T: futures::AsyncWrite + Unpin + Send
    {
        use futures::AsyncWriteExt;
        let len = req.raw.len() as u32;
        io.write_all(&len.to_be_bytes()).await?;
        io.write_all(&req.raw).await?;
        Ok(())
    }

    async fn write_response<T>(
        &mut self,
        _: &String,
        io: &mut T,
        res: Self::Response
    ) -> std::io::Result<()>
        where T: futures::AsyncWrite + Unpin + Send
    {
        use futures::AsyncWriteExt;
        io.write_all(&[res.ok as u8]).await?;
        Ok(())
    }
}

#[derive(NetworkBehaviour)]
pub struct KnotBehaviour {
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    pub frames: request_response::Behaviour<FrameCodec>,
    pub mdns: mdns::tokio::Behaviour,
    // For transveral nat
    pub relay_client: relay::client::Behaviour,
    pub dcutr: libp2p::dcutr::Behaviour,
}

type PeerTable = HashMap<PeerId, Vec<Multiaddr>>;
type RelayPeerTable = HashMap<PeerId, Multiaddr>;

pub async fn start_network(
    rx: mpsc::Receiver<NetworkCommand>,
    hub_tx: mpsc::Sender<KnotMessage>,
    temp_peerid: bool,
    port: u16
) {
    match run_network(rx, hub_tx, temp_peerid, port).await {
        Ok(_) => println!("[Network] Shutdown limpio"),
        Err(e) => eprintln!("[Network] Error fatal: {}", e),
    }
}

async fn run_network(
    mut command_rx: mpsc::Receiver<NetworkCommand>,
    hub_tx: mpsc::Sender<KnotMessage>,
    temp_peerid: bool,
    port: u16
) -> Result<(), Box<dyn Error>> {
    let mut local_key = libp2p::identity::Keypair::generate_ed25519();
    if !temp_peerid {
        local_key = load_or_create_identity();
    }
    let local_peer_id = PeerId::from(local_key.public());
    let (relay_transport, relay_client) = relay::client::new(local_peer_id);
    println!("[Network] Local Peer ID: {}", local_peer_id);

    let mut quic_config = libp2p_quic::Config::new(&local_key);

    quic_config.max_stream_data = 10_485_760; // 10MB por cada stream individual
    quic_config.max_connection_data = 15_728_640; // 15MB total de la conexión (agregado)
    quic_config.max_idle_timeout = 30_000;

    let mut swarm = SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_quic_config(|mut config| {
            config.max_stream_data = 10_485_760;
            config.max_connection_data = 15_728_640;
            config.max_idle_timeout = 30_000;
            config
        })
        .with_other_transport(|key| {
            Ok(
                relay_transport
                    .or_transport(libp2p::tcp::tokio::Transport::default())
                    .upgrade(upgrade::Version::V1)
                    .authenticate(noise::Config::new(key).unwrap())
                    .multiplex(yamux::Config::default())
            )
        })?
        .with_behaviour(|key| {
            let peer_id = PeerId::from(key.public());

            // Kademlia
            let mut kademlia = kad::Behaviour::new(peer_id, kad::store::MemoryStore::new(peer_id));
            kademlia.set_mode(Some(kad::Mode::Server));

            // Identify
            let identify = identify::Behaviour::new(
                identify::Config
                    ::new("/knot/1.0.0".into(), key.public())
                    .with_interval(Duration::from_secs(60))
            );

            let ping = ping::Behaviour::new(ping::Config::default());

            // Request/Response para BinaryFrame
            let frames = request_response::Behaviour::new(
                vec![("/knot/frame/1.0.0".to_string(), request_response::ProtocolSupport::Full)],
                request_response::Config
                    ::default()
                    .with_request_timeout(Duration::from_secs(20)) // Subimos a 20s
                    .with_max_concurrent_streams(1000)
            );

            let mdns = mdns::tokio::Behaviour::new(
                mdns::Config::default(),
                key.public().to_peer_id()
            )?;

            Ok(KnotBehaviour {
                identify,
                ping,
                kademlia,
                frames,
                mdns,
                relay_client,
                dcutr: libp2p::dcutr::Behaviour::new(local_peer_id),
            })
        })?
        .with_swarm_config(|c| {
            c.with_idle_connection_timeout(
                Duration::from_secs(60)
            ).with_max_negotiating_inbound_streams(100) // Evita cuellos de botella
        })
        .build();

    // /ip4/0.0.0.0/udp/<puerto>/quic-v1
    swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{}", port).parse()?)?;
    swarm.listen_on(format!("/ip4/0.0.0.0/udp/{}/quic-v1", port).parse()?)?;
    println!("[Network] Escuchando en QUIC: {}", port);

    // table with peerid - multiaddr for relay pending req
    let mut pending_listen: RelayPeerTable = HashMap::new();

    // table for saved peers
    let mut peer_table: PeerTable = HashMap::new();

    loop {
        tokio::select! {

            Some(cmd) = command_rx.recv() => {
                match cmd {
                    NetworkCommand::SendFrame { target_u64, frame } => {
                        handle_outbound_by_u64(&mut swarm, &peer_table, target_u64, frame);
                    }
                    NetworkCommand::GetPeers => {
                        let list = peer_table.clone().into_iter().collect();
                        let _ = hub_tx.send(KnotMessage::NetworkResponse(NetworkResponse::PeersList(list))).await;
                    }
                    NetworkCommand::DialAddress(addr) => {
                        println!("{}", addr);
                        let _ = swarm.dial(addr);
                    }
                    NetworkCommand::LookupPeer(peer_id) => {
                        swarm.behaviour_mut().kademlia.get_closest_peers(peer_id);
                    }
                    NetworkCommand::ConnectRelay { relay_addr, relay_peer_id }  => {
                        pending_listen.insert(relay_peer_id, relay_addr.clone().with(libp2p::multiaddr::Protocol::P2p(relay_peer_id)).with(libp2p::multiaddr::Protocol::P2pCircuit));
                        let _ = swarm.dial(relay_addr);
                    }
                }
            }

            event = swarm.next() => {
                let Some(event) = event else { break };

                // For relay connection
                if let SwarmEvent::ConnectionEstablished { peer_id, .. } = &event {
                    if let Some(addr) = pending_listen.remove(peer_id) {
                        println!("[Network] Connected to relay: {}", addr);
                        match swarm.listen_on(addr) {
                            Ok(id) => println!("[Network] listen_on circuit OK: {:?}", id),
                            Err(e) => eprintln!("[Network] listen_on circuit failed: {e}"),
                        }
                    }
                }

                handle_swarm_event(event, &mut swarm, &mut peer_table, &hub_tx).await;
            }
        }
    }

    Ok(())
}

async fn handle_swarm_event(
    event: SwarmEvent<KnotBehaviourEvent>,
    swarm: &mut libp2p::Swarm<KnotBehaviour>,
    peer_table: &mut PeerTable,
    hub_tx: &mpsc::Sender<KnotMessage>
) {
    match event {
        // ── Conexión establecida ───────────────────────────────────────
        SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
            println!("[Network] Conectado a {} via {:?}", peer_id, endpoint.get_remote_address());
            let addr = endpoint.get_remote_address().clone();
            peer_table.entry(peer_id).or_default().push(addr.clone());
            // Anunciar al peer en Kademlia
            swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
        }

        // ── Conexión cerrada ───────────────────────────────────────────
        SwarmEvent::ConnectionClosed { peer_id, .. } => {
            println!("[Network] Desconectado de {}", peer_id);
            // No lo eliminamos de peer_table; puede reconectar
        }

        // ── Nueva dirección de escucha confirmada ──────────────────────
        SwarmEvent::NewListenAddr { address, .. } => {
            println!("[Network] Escuchando en: {}", address);
        }

        SwarmEvent::IncomingConnectionError { error, .. } => {
            eprintln!(
                "[Network] Error en conexión entrante (posible fallo de reserva): {:?}",
                error
            );
        }
        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
            eprintln!("[Network] Error al contactar al peer {:?}: {:?}", peer_id, error);
        }

        // ── Eventos de comportamiento compuesto ────────────────────────
        SwarmEvent::Behaviour(behaviour_event) => {
            handle_behaviour_event(behaviour_event, swarm, peer_table, hub_tx).await;
        }

        _ => {}
    }
}

async fn handle_behaviour_event(
    event: KnotBehaviourEvent,
    swarm: &mut libp2p::Swarm<KnotBehaviour>,
    peer_table: &mut PeerTable,
    hub_tx: &mpsc::Sender<KnotMessage>
) {
    match event {
        // --- RELAY CLIENT EVENTS ---
        KnotBehaviourEvent::RelayClient(
            relay::client::Event::ReservationReqAccepted { relay_peer_id, .. },
        ) => {
            println!("[Network Relay] Reserva activa en Relay: {}", relay_peer_id);
        }
        KnotBehaviourEvent::RelayClient(
            relay::client::Event::InboundCircuitEstablished { src_peer_id, .. },
        ) => {
            eprintln!("[Network Relay] Circito creado via {}", src_peer_id);
        }
        KnotBehaviourEvent::RelayClient(
            relay::client::Event::OutboundCircuitEstablished { relay_peer_id, .. },
        ) => {
            println!("[Network Relay] Circuito de salida creado vía {}", relay_peer_id);
        }

        // --- DCUtR (HOLE PUNCHING) EVENTS ---
        KnotBehaviourEvent::Dcutr(libp2p::dcutr::Event { remote_peer_id, result }) => {
            match result {
                Ok(_) => {
                    println!("[Network] HOLE PUNCHING EXITOSO Conexión directa con {}", remote_peer_id);
                    let _ = hub_tx.send(
                        KnotMessage::Log(format!("P2P Directo con {}", remote_peer_id))
                    ).await;
                }
                Err(e) => {
                    eprintln!("[Network] Falló el upgrade directo con {}: {:?}", remote_peer_id, e);
                }
            }
        }

        // --- mDNS: Peer discovered ---
        KnotBehaviourEvent::Mdns(mdns::Event::Discovered(list)) => {
            for (peer_id, addr) in list {
                println!("[Network] mDNS: Nuevo peer local hallado: {}", peer_id);
                // Lo añadimos a Kademlia para que el ruteo sepa dónde está
                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr.clone());
                // Lo registramos en nuestra tabla interna
                peer_table.entry(peer_id).or_default().push(addr);
            }
        }

        // --- mDNS: Peer expired ---
        KnotBehaviourEvent::Mdns(mdns::Event::Expired(list)) => {
            for (peer_id, _addr) in list {
                println!("[Network] mDNS: Peer local expirado: {}", peer_id);
                // Opcional: limpiar de la tabla si quieres ser estricto
            }
        }

        // ── Identify: peer se identifica → actualizar tabla ────────────
        KnotBehaviourEvent::Identify(
            identify::Event::Received { peer_id, info, connection_id: _ },
        ) => {
            println!("[Network] Identify recibido de {}: agent={}", peer_id, info.agent_version);
            for addr in &info.listen_addrs {
                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr.clone());
                peer_table.entry(peer_id).or_default().push(addr.clone());
            }
            // Lanzar bootstrap si es el primer peer conocido
            if peer_table.len() == 1 {
                let _ = swarm.behaviour_mut().kademlia.bootstrap();
                println!("[Network] Kademlia bootstrap iniciado");
            }
        }

        KnotBehaviourEvent::Kademlia(kad::Event::RoutingUpdated { peer, addresses, .. }) => {
            println!("[Network] DHT routing actualizado para {}", peer);
            for addr in addresses.iter() {
                peer_table.entry(peer).or_default().push(addr.clone());
            }
        }

        KnotBehaviourEvent::Kademlia(
            kad::Event::OutboundQueryProgressed {
                result: kad::QueryResult::Bootstrap(Ok(kad::BootstrapOk { num_remaining, .. })),
                ..
            },
        ) => {
            if num_remaining == 0 {
                println!("[Network] Bootstrap Kademlia completado");
            }
        }

        // ── Request/Response: frame recibido de otro peer ──────────────
        KnotBehaviourEvent::Frames(
            request_response::Event::Message {
                peer,
                message: request_response::Message::Request { request, channel, .. },
                connection_id: _,
            },
        ) => {
            #[cfg(debug_assertions)]
            println!("[Network] Frame recibido de {}", peer);

            // Responder OK de inmediato
            let _ = swarm.behaviour_mut().frames.send_response(channel, FrameResponse { ok: true });

            if request.raw.len() >= 24 {
                let mut header = [0u8; 24];
                header.copy_from_slice(&request.raw[..24]);

                // El payload es un slice de Bytes (Zero-Copy)
                let payload = request.raw.slice(24..);

                let frame = BinaryFrame::from_raw(&header, payload);
                let from_ip = peer.to_string();

                let _ = hub_tx.send(KnotMessage::NetworkData { from_ip, frame }).await;
            } else {
                eprintln!("[Network] Frame recibido demasiado corto");
            }
        }

        KnotBehaviourEvent::Frames(
            request_response::Event::Message {
                peer,
                message: request_response::Message::Response { response, .. },
                connection_id: _,
            },
        ) => {
            #[cfg(debug_assertions)]
            if response.ok {
                println!("[Network] Entrega confirmada por {}", peer);
            } else {
                eprintln!("[Network] El peer {} rechazó el frame", peer);
            }
        }

        KnotBehaviourEvent::Frames(
            request_response::Event::OutboundFailure { peer, error, .. },
        ) => {
            eprintln!("[Network] Error enviando a {}: {:?}", peer, error);
        }

        _ => {}
    }
}

fn handle_outbound_by_u64(
    swarm: &mut libp2p::Swarm<KnotBehaviour>,
    peer_table: &PeerTable,
    target_u64: u64,
    raw: Bytes
) {
    let target = peer_table
        .keys()
        .find(|pid| peer_id_to_u64(pid) == target_u64)
        .copied();

    match target {
        Some(peer) => {
            let request = FrameRequest { raw };
            swarm.behaviour_mut().frames.send_request(&peer, request);
            #[cfg(debug_assertions)]
            println!("[Network] Frame enviado a {:?}", peer);
        }
        None => {
            eprintln!("[Network] Sin ruta para peer_id={} (no descubierto)", target_u64);
        }
    }
}

fn get_knot_config_dir() -> PathBuf {
    // dirs::config_dir() devuelve:
    // Linux:   /home/usuario/.config
    // Windows: C:\Users\Usuario\AppData\Roaming
    let mut path = dirs::config_dir().expect("No se pudo encontrar el directorio de configuración");

    // Añadimos nuestra carpeta específica
    path.push("knot");

    // Nos aseguramos de que la carpeta exista (si no, fs::write fallará)
    if !path.exists() {
        fs::create_dir_all(&path).expect("No se pudo crear la carpeta de configuración de Knot");
    }

    path
}

fn load_or_create_identity() -> identity::Keypair {
    let mut config_path = get_knot_config_dir();
    config_path.push("identity.bin");

    if config_path.exists() {
        let bytes = fs::read(&config_path).expect("No se pudo leer la identidad");
        identity::Keypair::from_protobuf_encoding(&bytes).expect("Archivo de identidad corrupto")
    } else {
        let new_key = identity::Keypair::generate_ed25519();
        let encoded = new_key.to_protobuf_encoding().unwrap();

        fs::write(&config_path, encoded).expect("No se pudo guardar la identidad");
        println!("Identidad persistente creada en: {:?}", config_path);
        new_key
    }
}
