// src/main.rs
mod core;
mod ingress;
mod network;
mod utils;

use crate::core::controller::start_core;
use crate::ingress::socket::{ IngressCommand, start_ingress };
use crate::network::swarm::{ NetworkCommand, NetworkResponse, start_network };
use crate::utils::framing::BinaryFrame;

use libp2p::{ Multiaddr, PeerId };
use std::collections::HashMap;
use std::env;
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
        peerid: PeerId,
        return_tx: tokio::sync::oneshot::Sender<String>,
    },
    GetPeersNetwork(tokio::sync::oneshot::Sender<HashMap<PeerId, Vec<Multiaddr>>>),
    GetLocalPeerIdNetwork(tokio::sync::oneshot::Sender<PeerId>),
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
        .unwrap_or(0); // Default P2P

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
    let (hub_tx, hub_rx) = mpsc::channel::<KnotMessage>(10000);

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

    start_core(to_net_tx, to_ing_tx, hub_rx).await;

    println!("\n\n[Knot] Shutting down");
}
