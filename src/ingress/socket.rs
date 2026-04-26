// src/ingress/socket.rs

use bytes::{ Bytes, BytesMut };
use libp2p::{ Multiaddr, PeerId };
use serde::{ Deserialize, Serialize };
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{ Mutex, mpsc };

use crate::KnotMessage;
use crate::ingress::client_sockets::send_to_local_app;
use crate::ingress::server_sockets::{ start_binary_data_server, start_managed_server };
use crate::utils::framing::BinaryFrame;

// El estado compartido que guardará todos los canales de retorno
type ServerRegistry = Arc<Mutex<HashMap<u64, u16>>>;
pub type ConnectionMap = Arc<Mutex<HashMap<u16, mpsc::Sender<Bytes>>>>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Message {
    Version,
    Protocol,
    #[serde(rename = "getcommands")] GetCommands,
    Status,
    #[serde(rename = "register")] Register {
        app_id: u64,
        port: u16,
    },
    Connect {
        multiaddr: String,
    },
    Discover {
        peer_id: String,
    },
    #[serde(rename = "connectrelay")] ConnectRelay {
        relay_addr: String,
        relay_id: String,
    },
    #[serde(rename = "getpeers")] GetPeers,
    #[serde(rename = "getpeerid")] GetPeerId,
}

impl Message {
    pub fn command_list() -> Vec<&'static str> {
        vec![
            "version",
            "protocol",
            "getcommands",
            "status",
            "register",
            "connect",
            "discover",
            "connectrelay",
            "getpeers",
            "getpeerid",
        ]
    }
}

#[derive(Debug)]
pub enum CentralEvent {
    Register {
        app_id: u64,
        port: u16,
    }, // save appid with a socekt port for redirect frames
    Connect {
        addr: Multiaddr,
    }, // dial peer with address
    Discover {
        peerid: PeerId,
        return_tx: tokio::sync::oneshot::Sender<String>,
    }, // use dht for discover address with a peerid
    RouteBinary {
        from_ip: String,
        frame: BinaryFrame,
    }, // Send frame data with BinaryFrame
    ConnectRelay {
        relay_addr: libp2p::Multiaddr,
        relay_peer_id: libp2p::PeerId,
    },
    GetPeers(tokio::sync::oneshot::Sender<HashMap<PeerId, Vec<Multiaddr>>>),
    GetLocalPeerId(tokio::sync::oneshot::Sender<PeerId>),
}

#[derive(Serialize, Debug)]
pub struct ResponseTcp {
    pub command: String,
    pub response: String,
    pub error: String,
}

pub enum IngressCommand {
    SendFrameToClient {
        frame: Bytes,
    },
}

pub async fn start_ingress(
    mut rx: mpsc::Receiver<IngressCommand>,
    hub_tx: mpsc::Sender<KnotMessage>,
    port: u16,
    binary_port: u16
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (central_tx, mut central_rx) = mpsc::channel::<CentralEvent>(100);

    // Conecctions appid - port
    let registry: ServerRegistry = Arc::new(Mutex::new(HashMap::new()));
    // connections (sockets) with binary port - sender
    let local_connections: ConnectionMap = Arc::new(Mutex::new(HashMap::new()));

    // TASK HUB
    let registry_for_central = Arc::clone(&registry);
    let conns = Arc::clone(&local_connections);
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                IngressCommand::SendFrameToClient { frame } => {
                    let buf = BytesMut::from(&frame[..]);
                    if buf.len() < 24 {
                        eprintln!("Error: El frame es demasiado corto para el encabezado de Knot");
                        return;
                    }
                    let decoded_frame = BinaryFrame::decode(buf);
                    let app_id = decoded_frame.app_id;

                    let reg = registry_for_central.lock().await;
                    if let Some(&target_port) = reg.get(&app_id) {
                        println!("  -> Reenviando a aplicación local en puerto: {}", target_port);
                        send_to_local_app(
                            Arc::clone(&conns),
                            target_port,
                            decoded_frame.payload
                        ).await;
                    } else {
                        println!("  -> AppID {} not registered", app_id);
                    }
                }
            }
        }
    });

    // TASK CENTRAL: El controlador
    let registry_for_hub = Arc::clone(&registry);
    tokio::spawn(async move {
        let reg_handle = Arc::clone(&registry_for_hub);
        while let Some(event) = central_rx.recv().await {
            match event {
                CentralEvent::Register { app_id, port } => {
                    let mut reg = reg_handle.lock().await;
                    if !reg.contains_key(&app_id) {
                        reg.insert(app_id, port);
                        println!("[Ingress] new appname on hashmap: {} -> {}", app_id, port);
                    }
                }
                CentralEvent::Connect { addr } => {
                    println!("[Ingress] Sending Connect command to [Network]: {}", addr);
                    let _ = hub_tx.send(KnotMessage::ConnectToNetwork { addr }).await;
                }
                CentralEvent::Discover { peerid, return_tx } => {
                    println!("[Ingress] Sending Discover command to [Network]: {}", peerid);
                    let _ = hub_tx.send(KnotMessage::DiscoverNetwork { peerid, return_tx }).await;
                }
                CentralEvent::RouteBinary { from_ip, frame } => {
                    #[cfg(debug_assertions)]
                    println!("  Data form {} to {}", from_ip, frame.peer_id);

                    // Aquí buscarías en tu Registro quién tiene ese PeerID y le mandas el SendRaw
                    let _ = hub_tx.send(KnotMessage::ClientData { from_ip, frame }).await;
                }
                CentralEvent::ConnectRelay { relay_addr, relay_peer_id } => {
                    let _ = hub_tx.send(KnotMessage::ConnectRelay {
                        relay_addr,
                        relay_peer_id,
                    }).await;
                }
                CentralEvent::GetPeers(oneshot) => {
                    let _ = hub_tx.send(KnotMessage::GetPeersNetwork(oneshot)).await;
                }
                CentralEvent::GetLocalPeerId(oneshot) => {
                    let _ = hub_tx.send(KnotMessage::GetLocalPeerIdNetwork(oneshot)).await;
                }
            }
        }
    });

    let tx = central_tx.clone();
    let txx = central_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = start_managed_server(tx, port).await {
            eprintln!("Error en servidor: {}", e);
        }
    });

    tokio::spawn(async move {
        if let Err(e) = start_binary_data_server(txx, binary_port).await {
            eprintln!("Error en servidor: {}", e);
        }
    });

    tokio::signal::ctrl_c().await?;
    Ok(())
}
