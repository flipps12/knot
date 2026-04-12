// src/main.rs
mod ingress;
mod network;
mod core;
mod utils;

use crate::ingress::socket::{ IngressCommand, start_ingress };
use crate::network::swarm::{ NetworkCommand, NetworkResponse, start_network };
use crate::utils::framing::BinaryFrame;

use std::env;
use libp2p::{ Multiaddr, PeerId };
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum KnotMessage {
    // De Ingress al Core: "Tengo datos de un cliente"
    ClientData {
        from_ip: String,
        frame: BinaryFrame,
    },
    ConnectToNetwork {
        addr: String,
    },
    DiscoverNetwork {
        peerid: String,
    },
    GetPeersNetwork,
    ConnectRelay {
        relay_addr: Multiaddr,
        relay_peer_id: PeerId,
    },
    // Del Network al Core: "Recibí algo del P2P"
    NetworkData {
        from_ip: String,
        frame: BinaryFrame,
    },
    NetworkResponse(NetworkResponse),
    Shutdown,
    Log(String),
}

#[tokio::main]
async fn main() {
    #[cfg(debug_assertions)]
    println!("Debug mode");

    // for benchamrk
    let mut count = 0;
    let mut count_rec = 0;

    // Args for ports
    let args: Vec<String> = env::args().collect();

    let port_ingress = args
        .get(1)
        .map(|s| s.parse::<u16>().expect("Puerto Ingress inválido"))
        .unwrap_or(12012); // Default Ingress

    let port_ingress_binary = args
        .get(2)
        .map(|s| s.parse::<u16>().expect("Puerto Ingress inválido"))
        .unwrap_or(12812); // Second Default Ingress

    let port_network = args
        .get(3)
        .map(|s| s.parse::<u16>().expect("Puerto Network inválido"))
        .unwrap_or(13013); // Default P2P

    let temp_peerid = args
        .get(4)
        .map(|s| s.parse::<bool>().expect("Puerto Network inválido"))
        .unwrap_or(false); // Default P2P

    println!("--- Knot P2P Protocol Node ---");
    println!("[Main] Ingress Port: {}", port_ingress);
    println!("[Main] Ingress Binary Port: {}", port_ingress_binary);
    println!("[Main] Network Port: {}", port_network);
    if temp_peerid {
        println!("Temporal PeerID");
    }

    // Main Channel
    // Ingress -> Core <- Network
    let (hub_tx, mut hub_rx) = mpsc::channel::<KnotMessage>(10000);

    // Secondary Channel
    // Core -> Network
    let (to_net_tx, to_net_rx) = mpsc::channel::<NetworkCommand>(10000);
    // Core -> Ingress
    let (to_ing_tx, to_ing_rx) = mpsc::channel::<IngressCommand>(10000);

    // Clone rx - tx
    // for Ingress
    let ing_hub_tx = hub_tx.clone();
    // for Network
    let net_hub_tx = hub_tx.clone();

    // Spawn Workers
    tokio::spawn(start_ingress(to_ing_rx, ing_hub_tx, port_ingress, port_ingress_binary));
    tokio::spawn(start_network(to_net_rx, net_hub_tx, temp_peerid, port_network));

    // For debug, Core is this
    loop {
        tokio::select! {
            Some(message) = hub_rx.recv() => {
                match message {
                    KnotMessage::ClientData { from_ip: _, frame } => {
                        // for benchmark
                        count += 1;
                        if count % 10000 == 0 {
                            println!("[Core] Frames procesados: {}", count);
                        }

                        
                        let target_u64 = frame.peer_id;

                        #[cfg(debug_assertions)]
                        println!("[Core] Ingress -> Network PeerID (u64): {}", target_u64);

                        // 3. Enviamos el comando a la red
                        let _ = to_net_tx.send(NetworkCommand::SendFrame { 
                            target_u64, 
                            frame: frame.encode()
                        }).await;
                    }
                    KnotMessage::ConnectToNetwork { addr } => {
                        let addr_parsed: Multiaddr = addr.parse().expect("Invalid Addr");
                        let _ = to_net_tx.send(NetworkCommand::DialAddress(addr_parsed)).await;
                    }
                    KnotMessage::DiscoverNetwork { peerid } => {
                        let peerid_parsed: PeerId = peerid.parse().expect("Invalid Addr");
                        let _ = to_net_tx.send(NetworkCommand::LookupPeer(peerid_parsed)).await;
                    }
                    KnotMessage::GetPeersNetwork => {
                        let _ = to_net_tx.send(NetworkCommand::GetPeers).await;
                    }
                    KnotMessage::ConnectRelay { relay_addr, relay_peer_id} => {
                        let _ = to_net_tx.send(NetworkCommand::ConnectRelay { relay_addr, relay_peer_id }).await;
                    }
                    KnotMessage::NetworkData { from_ip, frame } => {
                        // for benchmark
                        count_rec += 1;
                        if count_rec % 10000 == 0 {
                            println!("[Core] Frames procesados: {}", count_rec);
                        }

                        let _ = to_ing_tx.send(IngressCommand::SendFrameToClient { 
                            from_ip, 
                            frame: frame.encode()
                        }).await;
                    }
                    KnotMessage::NetworkResponse(response) => {
                        match response {
                            NetworkResponse::PeersList(list) => {
                                println!("{:?}", list);
                            }
                            NetworkResponse::CommandAccepted => {
                                // unused
                            }
                        }
                    }
                    KnotMessage::Log(msg) => {
                        println!("[LOG GLOBAL]: {}", msg);
                    }
                    KnotMessage::Shutdown => {
                        println!("Apagando Knot...");
                        break;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                break; 
            }
        }
    }

    println!("\n\n[Knot] Shutting down");
}
