// src/network/swarm.rs

use libp2p::{
    Multiaddr, PeerId, SwarmBuilder, identify, kad, mdns, request_response, swarm::{NetworkBehaviour, SwarmEvent}
};
use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;
use std::error::Error;
use std::time::Duration;
use std::collections::HashMap;
use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};

use crate::KnotMessage;
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
        frame: Bytes 
    },
    GetPeers,
    DialAddress(libp2p::Multiaddr),
    LookupPeer(libp2p::PeerId),
    PrepareHolePunch(libp2p::PeerId),
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

    async fn read_request<T>(
        &mut self,
        _: &String,
        io: &mut T,
    ) -> std::io::Result<Self::Request>
    where
        T: futures::AsyncRead + Unpin + Send,
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

    async fn read_response<T>(
        &mut self,
        _: &String,
        io: &mut T,
    ) -> std::io::Result<Self::Response>
    where
        T: futures::AsyncRead + Unpin + Send,
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
        req: Self::Request,
    ) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
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
        res: Self::Response,
    ) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        use futures::AsyncWriteExt;
        io.write_all(&[res.ok as u8]).await?;
        Ok(())
    }
}


#[derive(NetworkBehaviour)]
pub struct KnotBehaviour {
    pub identify: identify::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    pub frames: request_response::Behaviour<FrameCodec>,
    pub mdns: mdns::tokio::Behaviour,
}

/// Tabla de peers conocidos: PeerId → lista de Multiaddr activas
type PeerTable = HashMap<PeerId, Vec<Multiaddr>>;

// ─────────────────────────────────────────────
//  Entry point: start_network
// ─────────────────────────────────────────────

pub async fn start_network(
    mut rx: mpsc::Receiver<NetworkCommand>,
    hub_tx: mpsc::Sender<KnotMessage>,
    port: u16
) {
    match run_network(rx, hub_tx, port).await {
        Ok(_) => println!("[Network] Shutdown limpio"),
        Err(e) => eprintln!("[Network] Error fatal: {}", e),
    }
}

async fn run_network(
    mut command_rx: mpsc::Receiver<NetworkCommand>,
    hub_tx: mpsc::Sender<KnotMessage>,
    port: u16
) -> Result<(), Box<dyn Error>> {

    // ── 1. Identidad local ──────────────────────────────────────────────
    let local_key = libp2p::identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    println!("[Network] Local Peer ID: {}", local_peer_id);

    let mut quic_config = libp2p_quic::Config::new(&local_key);

    // 10MB de margen para que los frames grandes fluyan sin pausas
    quic_config.max_stream_data = 10_485_760;     // 10MB por cada stream individual
    quic_config.max_connection_data = 15_728_640; // 15MB total de la conexión (agregado)
    quic_config.max_idle_timeout = 30_000;

    // ── 2. Swarm con QUIC ───────────────────────────────────────────────
    // SwarmBuilder::with_existing_identity toma el keypair y configura
    // el transporte QUIC automáticamente (TLS 1.3 integrado, 0-RTT).
    // ── 2. Swarm con QUIC Tuned ───────────────────────────────────────────
    let mut swarm = SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        // ESTO ES LO QUE CAMBIA: Usamos el método específico de QUIC
        .with_quic_config(|mut config| {
            config.max_stream_data = 10_485_760;     // 10MB por stream
            config.max_connection_data = 15_728_640; // 15MB total conexión
            config.max_idle_timeout = 30_000;        // 30 segundos
            config
        })
        .with_behaviour(|key| {
            let peer_id = PeerId::from(key.public());

            // Kademlia
            let mut kademlia = kad::Behaviour::new(
                peer_id,
                kad::store::MemoryStore::new(peer_id),
            );
            kademlia.set_mode(Some(kad::Mode::Server));

            // Identify
            let identify = identify::Behaviour::new(
                identify::Config::new("/knot/1.0.0".into(), key.public())
                    .with_interval(Duration::from_secs(60)),
            );

            // Request/Response para BinaryFrame
            let frames = request_response::Behaviour::new(
                vec![(
                    "/knot/frame/1.0.0".to_string(),
                    request_response::ProtocolSupport::Full,
                )],
                request_response::Config::default()
                    .with_request_timeout(Duration::from_secs(20)) // Subimos a 20s
                    .with_max_concurrent_streams(1000),
            );

            let mdns = mdns::tokio::Behaviour::new(
                mdns::Config::default(), 
                key.public().to_peer_id()
            )?;

            Ok(KnotBehaviour { identify, kademlia, frames, mdns })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    // ── 3. Escuchar en QUIC ─────────────────────────────────────────────
    // /ip4/0.0.0.0/udp/<puerto>/quic-v1  ← formato correcto para QUIC v1
    swarm.listen_on(format!("/ip4/0.0.0.0/udp/{}/quic-v1", port).parse()?)?;
    println!("[Network] Escuchando en QUIC: {}", port);

    // ── 4. Bootstrap con peers conocidos (opcional) ─────────────────────
    // En producción vendría de config/env; aquí dejamos el slot listo.
    // bootstrap_peers(&mut swarm);

    // ── 5. Tabla de peers conocidos ─────────────────────────────────────
    let mut peer_table: PeerTable = HashMap::new();

    // ── 6. Loop principal ───────────────────────────────────────────────
    loop {
        tokio::select! {

            // ── A. Mensajes del Core → enviar por P2P ──────────────────
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
                        let _ = swarm.dial(addr);
                    }
                    NetworkCommand::LookupPeer(peer_id) => {
                        swarm.behaviour_mut().kademlia.get_closest_peers(peer_id);
                    }
                    NetworkCommand::PrepareHolePunch(peer_id) => {
                        // Aquí iría la lógica de dcut/relay en el futuro
                        println!("[Network] Hole punching solicitado para {}", peer_id);
                    }
                }
            }

            // ── B. Eventos del Swarm ───────────────────────────────────
            event = swarm.next() => {
                let Some(event) = event else { break };
                handle_swarm_event(event, &mut swarm, &mut peer_table, &hub_tx).await;
            }
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────
//  Manejo de salida: Core → P2P
// ─────────────────────────────────────────────

fn handle_outbound(
    swarm: &mut libp2p::Swarm<KnotBehaviour>,
    peer_table: &PeerTable,
    raw: Bytes,
) {
    // Los bytes que llegan del Core son un BinaryFrame ya encodado.
    // Leemos el peer_id del header (bytes 2..10) para hacer routing.
    if raw.len() < 24 {
        eprintln!("[Network] Frame demasiado corto para routing: {} bytes", raw.len());
        return;
    }

    let peer_id_u64 = u64::from_be_bytes(raw[2..10].try_into().unwrap());

    // Buscar en peer_table algún PeerId cuyo hash coincida.
    // En tu sistema, peer_id en el frame es el u64 del PeerId de libp2p
    // (lo truncás al registrar). Buscamos coincidencia exacta.
    let target = peer_table
        .keys()
        .find(|pid| peer_id_to_u64(pid) == peer_id_u64)
        .copied();

    match target {
        Some(peer) => {
            let request = FrameRequest { raw };
            swarm.behaviour_mut().frames.send_request(&peer, request);
            #[cfg(debug_assertions)]
            println!("[Network] Frame enviado a {:?}", peer);
        }
        None => {
            eprintln!("[Network] Sin ruta para peer_id={} (aún no descubierto)", peer_id_u64);
            // Aquí podrías encolar el frame y reintentar tras descubrir el peer
        }
    }
}

// ─────────────────────────────────────────────
//  Manejo de eventos del Swarm
// ─────────────────────────────────────────────

async fn handle_swarm_event(
    event: SwarmEvent<KnotBehaviourEvent>,
    swarm: &mut libp2p::Swarm<KnotBehaviour>,
    peer_table: &mut PeerTable,
    hub_tx: &mpsc::Sender<KnotMessage>,
) {
    match event {

        SwarmEvent::Behaviour(KnotBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
            for (peer_id, addr) in list {
                println!("[Network] mDNS: Nuevo peer local hallado: {}", peer_id);
                // Lo añadimos a Kademlia para que el ruteo sepa dónde está
                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr.clone());
                // Lo registramos en nuestra tabla interna
                peer_table.entry(peer_id).or_default().push(addr);
            }
        }
        
        // --- mDNS: Peer local se desconectó ---
        SwarmEvent::Behaviour(KnotBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
            for (peer_id, _addr) in list {
                println!("[Network] mDNS: Peer local expirado: {}", peer_id);
                // Opcional: limpiar de la tabla si quieres ser estricto
            }
        }

        // ── Conexión establecida ───────────────────────────────────────
        SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
            println!("[Network] ✓ Conectado a {}", peer_id);
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
    hub_tx: &mpsc::Sender<KnotMessage>,
) {
    match event {

        // ── Identify: peer se identifica → actualizar tabla ────────────
        KnotBehaviourEvent::Identify(identify::Event::Received { peer_id, info, connection_id: _ }) => {
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

        // ── Kademlia: peer descubierto vía DHT ─────────────────────────
        KnotBehaviourEvent::Kademlia(kad::Event::RoutingUpdated { peer, addresses, .. }) => {
            println!("[Network] DHT routing actualizado para {}", peer);
            for addr in addresses.iter() {
                peer_table.entry(peer).or_default().push(addr.clone());
            }
        }

        KnotBehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed {
            result: kad::QueryResult::Bootstrap(Ok(kad::BootstrapOk { num_remaining, .. })),
            ..
        }) => {
            if num_remaining == 0 {
                println!("[Network] Bootstrap Kademlia completado");
            }
        }

        // ── Request/Response: frame recibido de otro peer ──────────────
        KnotBehaviourEvent::Frames(request_response::Event::Message {
            peer,
            message: request_response::Message::Request { request, channel, .. },
            connection_id: _,
        }) => {
            #[cfg(debug_assertions)]
            println!("[Network] Frame recibido de {}", peer);

            // Responder OK de inmediato
            let _ = swarm.behaviour_mut().frames.send_response(
                channel,
                FrameResponse { ok: true },
            );

            // Decodificar y reenviar al Core vía hub
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

        // ── Request/Response: confirmación de envío ────────────────────
        KnotBehaviourEvent::Frames(request_response::Event::Message {
            peer,
            message: request_response::Message::Response { response, .. },
            connection_id: _,
        }) => {
            #[cfg(debug_assertions)]
            if response.ok {
                println!("[Network] Entrega confirmada por {}", peer);
            } else {
                eprintln!("[Network] El peer {} rechazó el frame", peer);
            }
        }

        KnotBehaviourEvent::Frames(request_response::Event::OutboundFailure { peer, error, .. }) => {
            eprintln!("[Network] Error enviando a {}: {:?}", peer, error);
        }

        _ => {}
    }
}

// ─────────────────────────────────────────────
//  Utilidades
// ─────────────────────────────────────────────

/// Trunca un PeerId a u64 para el campo peer_id del BinaryFrame.
/// Se usa el hash del multihash subyacente.
pub fn peer_id_to_u64(peer_id: &PeerId) -> u64 {
    let bytes = peer_id.to_bytes();
    // Tomar los últimos 8 bytes del digest
    let start = bytes.len().saturating_sub(8);
    let slice = &bytes[start..];
    let mut arr = [0u8; 8];
    arr[..slice.len()].copy_from_slice(slice);
    u64::from_be_bytes(arr)
}

/// Conectar a un peer conocido por su dirección (bootstrap manual).
/// Llámalo desde main si tenés peers semilla en config.
pub fn dial_peer(swarm: &mut libp2p::Swarm<KnotBehaviour>, addr: Multiaddr) {
    match swarm.dial(addr.clone()) {
        Ok(_) => println!("[Network] Dial iniciado a {}", addr),
        Err(e) => eprintln!("[Network] Error dial {}: {}", addr, e),
    }
}

fn handle_outbound_by_u64(
    swarm: &mut libp2p::Swarm<KnotBehaviour>,
    peer_table: &PeerTable,
    target_u64: u64,
    raw: Bytes,
) {
    // Buscamos en nuestra tabla de peers si algún PeerId matchea con el u64
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
