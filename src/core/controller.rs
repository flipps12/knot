// src/core/controllar.rs

use tokio::sync::mpsc::{ Receiver, Sender };

use crate::{
    KnotMessage,
    ingress::socket::IngressCommand,
    network::swarm::{ NetworkCommand, NetworkResponse },
    utils::tou64::peer_id_to_u64,
};

pub async fn start_core(
    to_net_tx: Sender<NetworkCommand>,
    to_ing_tx: Sender<IngressCommand>,
    mut hub_rx: Receiver<KnotMessage>
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
                        // let addr_parsed: Multiaddr = addr.parse().expect("Invalid Addr");
                        let _ = to_net_tx.send(NetworkCommand::DialAddress(addr)).await;
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
                    KnotMessage::NetworkData { peer, mut frame } => {
                        let target_u64 = peer_id_to_u64(&peer);
                        
                        if frame.app_id == 0 {
                            frame.app_id = 1;
                            let _ = to_net_tx.send(NetworkCommand::SendFrame {
                                target_u64,
                                frame: frame.encode()
                            }).await;
                        } else {
                            frame.peer_id = target_u64;
                            let _ = to_ing_tx.send(IngressCommand::SendFrameToClient {
                                frame: frame.encode()
                            }).await;
                        }
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
