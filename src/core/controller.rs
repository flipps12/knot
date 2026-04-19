// src/core/controllar.rs

use libp2p::Multiaddr;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::{
    KnotMessage,
    ingress::socket::IngressCommand,
    network::swarm::{NetworkCommand, NetworkResponse},
};

pub async fn start_core(
    to_net_tx: Sender<NetworkCommand>,
    to_ing_tx: Sender<IngressCommand>,
    mut hub_rx: Receiver<KnotMessage>,
) {
    loop {
        tokio::select! {
            Some(message) = hub_rx.recv() => {
                match message {
                    KnotMessage::ClientData { from_ip: _, frame } => {



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
                    KnotMessage::DiscoverNetwork { peerid, return_tx } => {
                        let _ = to_net_tx.send(NetworkCommand::LookupPeer(peerid, return_tx)).await;
                    }
                    KnotMessage::GetPeersNetwork(oneshot) => {
                        let _ = to_net_tx.send(NetworkCommand::GetPeers(oneshot)).await;
                    }
                    KnotMessage::GetLocalPeerIdNetwork(oneshot) => {
                        let _ = to_net_tx.send(NetworkCommand::GetLocalPeer(oneshot)).await;
                    }
                    KnotMessage::ConnectRelay { relay_addr, relay_peer_id} => {
                        let _ = to_net_tx.send(NetworkCommand::ConnectRelay { relay_addr, relay_peer_id }).await;
                    }
                    KnotMessage::NetworkData { from_ip, frame } => {
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
}
