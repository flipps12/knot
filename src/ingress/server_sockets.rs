// src/ingress/server_sockets.rs

use std::collections::HashMap;

use bytes::BytesMut;
use futures::{ SinkExt, StreamExt };
use libp2p::{ Multiaddr, PeerId };
use tokio::io::{ AsyncReadExt, AsyncWriteExt };
use tokio::net::TcpListener;
use tokio::sync::{ mpsc, oneshot };
use tokio_util::codec::{ Framed, LinesCodec };

use crate::ingress::socket::{ CentralEvent, Message, ResponseTcp };
use crate::utils::framing::BinaryFrame;

pub async fn start_managed_server(
    central_tx: mpsc::Sender<CentralEvent>,
    port: u16
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;

    println!("[Servidor {}] Started", port);
    
    let version = env!("CARGO_PKG_VERSION");

    loop {
        let (socket, _addr) = listener.accept().await?;
        let tx_clone = central_tx.clone();

        tokio::spawn(async move {
            let mut framed = Framed::new(socket, LinesCodec::new());

            while let Some(Ok(linea)) = framed.next().await {
                // Deserialización directa al Enum
                if let Ok(msg) = serde_json::from_str::<Message>(&linea) {
                    match msg {
                        Message::Version => {
                            let resp = ResponseTcp {
                                command: "version".into(),
                                response: version.to_string(),
                                error: "".into(),
                            };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                        }
                        Message::Protocol => {
                            let resp = ResponseTcp {
                                command: "protocol".into(),
                                response: "v0.1.0".into(),
                                error: "".into(),
                            };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                        }
                        Message::GetCommands => {
                            let resp = ResponseTcp {
                                command: "getcommands".into(),
                                response: serde_json::to_string(&Message::command_list()).unwrap(),
                                error: "".into(),
                            };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                        }
                        Message::Status => {
                            let resp = ResponseTcp {
                                command: "status".into(),
                                response: "OK".into(),
                                error: "".into(),
                            };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                        }
                        Message::Register { app_id, port } => {
                            let _ = tx_clone.send(CentralEvent::Register { app_id, port }).await;
                            // let _ = framed.send(format!("OK: Registered ID {}", app_id)).await;
                            let resp = ResponseTcp {
                                command: "register".into(),
                                response: app_id.to_string(),
                                error: "".into(),
                            };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                        }
                        Message::Connect { multiaddr } => {
                            let parsed_addr = multiaddr.parse();
                            let _ = tx_clone.send(CentralEvent::Connect { 
                                addr: match parsed_addr {
                                    Ok(parsed_addr) => parsed_addr,
                                    Err(err) => {
                                        let resp = ResponseTcp {
                                            command: "connect".into(),
                                            response: "".into(),
                                            error: err.to_string(),
                                        };
                                    
                                        let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                                        return;
                                    }
                                },
                            }).await;

                            // let _ = framed.send("Connecting...").await;

                            let resp = ResponseTcp {
                                command: "connect".into(),
                                response: "OK".into(),
                                error: "".into(),
                            };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                        }
                        Message::ConnectRelay { relay_addr, relay_id } => {
                            let parsed_addr = relay_addr.parse();
                            let parsed_id = relay_id.parse();

                            let _ = tx_clone.send(CentralEvent::ConnectRelay {
                                relay_addr: match parsed_addr {
                                    Ok(parsed_addr) => parsed_addr,
                                    Err(err) => {
                                        let resp = ResponseTcp {
                                            command: "connectrelay".into(),
                                            response: "".into(),
                                            error: err.to_string(),
                                        };
                                        let _ = framed.send(
                                            serde_json::to_string(&resp).unwrap()
                                        ).await;
                                        return;
                                    }
                                },
                                relay_peer_id: match parsed_id {
                                    Ok(parsed_id) => parsed_id,
                                    Err(err) => {
                                        let resp = ResponseTcp {
                                            command: "connectrelay".into(),
                                            response: "".into(),
                                            error: err.to_string(),
                                        };
                                        let _ = framed.send(
                                            serde_json::to_string(&resp).unwrap()
                                        ).await;
                                        return;
                                    }
                                },
                            }).await;
                            // let _ = framed.send("Relay request sent").await;

                            let resp = ResponseTcp {
                                command: "connectrelay".into(),
                                response: "OK".into(),
                                error: "".into(),
                            };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                        }
                        Message::Discover { peer_id } => {
                            let parsed_id: PeerId = match peer_id.parse() {
                                Ok(peer_id) => peer_id,
                                Err(err) => {
                                    let resp = ResponseTcp {
                                        command: "discover".into(),
                                        response: "".into(),
                                        error: err.to_string(),
                                    };
                                    let _ = framed.send(
                                        serde_json::to_string(&resp).unwrap()
                                    ).await;
                                    return;
                                }
                            };

                            let (resp_tx, resp_rx) = oneshot::channel::<String>();

                            let _ = tx_clone.send(CentralEvent::Discover {
                                peerid: parsed_id,
                                return_tx: resp_tx,
                            }).await;

                            // 2. Esperamos la respuesta del Core/Worker B
                            let response_from_core = resp_rx.await;

                            // 3. Respondemos al cliente TCP original
                            let resp = match response_from_core {
                                Ok(response) =>
                                    ResponseTcp {
                                        command: "discover".into(),
                                        response: format!("{:?}", response),
                                        error: "".into(),
                                    },
                                Err(_) =>
                                    ResponseTcp {
                                        command: "discover".into(),
                                        response: "".into(),
                                        error: "Core dropped the responder".into(),
                                    },
                            };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                        }
                        Message::GetPeers => {
                            let (resp_tx, resp_rx) = oneshot::channel::<
                                HashMap<PeerId, Vec<Multiaddr>>
                            >();

                            let _ = tx_clone.send(CentralEvent::GetPeers(resp_tx)).await;

                            let response_from_core = resp_rx.await;

                            // 3. Respondemos al cliente TCP original
                            let resp = match response_from_core {
                                Ok(response) =>
                                    ResponseTcp {
                                        command: "getpeers".into(),
                                        response: format!("{:?}", response),
                                        error: "".into(),
                                    },
                                Err(_) =>
                                    ResponseTcp {
                                        command: "getpeers".into(),
                                        response: "".into(),
                                        error: "Core dropped the responder".into(),
                                    },
                            };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                        }
                        Message::GetPeerId => {
                            let (resp_tx, resp_rx) = oneshot::channel::<PeerId>();

                            let _ = tx_clone.send(CentralEvent::GetLocalPeerId(resp_tx)).await;

                            let response_from_core = resp_rx.await;

                            let resp = match response_from_core {
                                Ok(response) =>
                                    ResponseTcp {
                                        command: "getpeers".into(),
                                        response: format!("{:?}", response),
                                        error: "".into(),
                                    },
                                Err(_) =>
                                    ResponseTcp {
                                        command: "getpeers".into(),
                                        response: "".into(),
                                        error: "Core dropped the responder".into(),
                                    },
                            };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                        }
                    }
                } else {
                    let resp = ResponseTcp {
                        command: "server".into(),
                        response: "".into(),
                        error: "Error unknow command".into(),
                    };
                    let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                }
            }
        });
    }
}

pub async fn start_binary_data_server(
    central_tx: mpsc::Sender<CentralEvent>,
    port: u16
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;

    println!("[Servidor {}] Ingress Binario started", port);

    loop {
        let (mut socket, addr) = listener.accept().await?;
        let tx = central_tx.clone();

        tokio::spawn(async move {
            let mut header_buf = [0u8; 24];
            // Reservamos capacidad inicial para evitar allocs pequeños
            let mut payload_buffer = BytesMut::with_capacity(65536);

            loop {
                // 1. Leer Header
                if socket.read_exact(&mut header_buf).await.is_err() {
                    break;
                }

                // 2. Extraer longitud (Offset 18-22 según tu framing.rs)
                let len = u32::from_be_bytes(header_buf[18..22].try_into().unwrap()) as usize;

                // 3. Optimización de Memoria: Leer directamente al Buffer
                payload_buffer.resize(len, 0);
                if socket.read_exact(&mut payload_buffer).await.is_err() {
                    break;
                }

                // freeze() convierte BytesMut en Bytes (atómico y sin copia)
                let payload_bytes = payload_buffer.split_to(len).freeze();

                // 4. Construir el Frame
                let frame = BinaryFrame::from_raw(&header_buf, payload_bytes);

                // 5. Logs solo en modo DEBUG (No afectan al benchmark --release)
                #[cfg(debug_assertions)]
                println!("[Ingress] Data de {} para ID: {}", addr, frame.peer_id);

                // For benchmark only
                if socket.write_u8(1).await.is_err() {
                    break;
                }

                // 6. Enviar a la Central
                if
                    tx
                        .send(CentralEvent::RouteBinary {
                            from_ip: addr.to_string(),
                            frame,
                        }).await
                        .is_err()
                {
                    break;
                }
            }
        });
    }
}
