// src/ingress/server_sockets.rs

use std::collections::HashMap;

use bytes::BytesMut;
use futures::{ SinkExt, StreamExt };
use libp2p::{ Multiaddr, PeerId };
use tokio::io::{ AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt };
use tokio::sync::{ mpsc, oneshot };
use tokio_util::codec::{ Framed, LinesCodec };
#[cfg(unix)]
use tokio::net::{ UnixListener };

use crate::ingress::socket::{ CentralEvent, Message, ResponseTcp };
use crate::utils::framing::BinaryFrame;

pub async fn start_managed_server(
    central_tx: mpsc::Sender<CentralEvent>,
    port: u16
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use std::path::Path;

        let path = format!("/tmp/knot_managed_{}.sock", port);
        if Path::new(&path).exists() {
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        println!("[Servidor Unix: {}] Started", path);
        loop {
            let (socket, _) = listener.accept().await?;
            tokio::spawn(handle_managed_connection(socket, central_tx.clone()));
        }
    }

    #[cfg(not(unix))]
    {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
        println!("[Servidor TCP: {}] Started", port);
        loop {
            let (socket, _) = listener.accept().await?;
            tokio::spawn(handle_managed_connection(socket, central_tx.clone()));
        }
    }
}

async fn handle_managed_connection<S>(socket: S, central_tx: mpsc::Sender<CentralEvent>)
    where S: AsyncRead + AsyncWrite + Unpin + Send + 'static
{
    let version = env!("CARGO_PKG_VERSION");
    let mut framed = Framed::new(socket, LinesCodec::new());

    while let Some(Ok(linea)) = framed.next().await {
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
                    let _ = central_tx.send(CentralEvent::Register { app_id, port }).await;
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
                    let _ = central_tx.send(CentralEvent::Connect {
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

                    let _ = central_tx.send(CentralEvent::ConnectRelay {
                        relay_addr: match parsed_addr {
                            Ok(parsed_addr) => parsed_addr,
                            Err(err) => {
                                let resp = ResponseTcp {
                                    command: "connectrelay".into(),
                                    response: "".into(),
                                    error: err.to_string(),
                                };
                                let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
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
                                let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
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
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                            return;
                        }
                    };

                    let (resp_tx, resp_rx) = oneshot::channel::<String>();

                    let _ = central_tx.send(CentralEvent::Discover {
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
                    let (resp_tx, resp_rx) = oneshot::channel::<HashMap<PeerId, Vec<Multiaddr>>>();

                    let _ = central_tx.send(CentralEvent::GetPeers(resp_tx)).await;

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

                    let _ = central_tx.send(CentralEvent::GetLocalPeerId(resp_tx)).await;

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
                Message::Listeners => {
                    let (resp_tx, resp_rx) = oneshot::channel::<Vec<Multiaddr>>();

                    let _ = central_tx.send(CentralEvent::Listeners(resp_tx)).await;

                    let response_from_core = resp_rx.await;

                    // 3. Respondemos al cliente TCP original
                    let resp = match response_from_core {
                        Ok(response) =>
                            ResponseTcp {
                                command: "listeners".into(),
                                response: format!("{:?}", response),
                                error: "".into(),
                            },
                        Err(_) =>
                            ResponseTcp {
                                command: "listeners".into(),
                                response: "".into(),
                                error: "Core dropped the responder".into(),
                            },
                    };
                    let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                }
                // _ => {
                //     let resp = ResponseTcp {
                //         command: "server".into(),
                //         response: "".into(),
                //         error: "Error unknow command".into(),
                //     };
                //     let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                // }
            }
        }
    }
}

pub async fn start_binary_data_server(
    central_tx: mpsc::Sender<CentralEvent>,
    port: u16
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(unix)]
    {
        use std::path::Path;

        let path = format!("/tmp/knot_binary_{}.sock", port);
        if Path::new(&path).exists() {
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        println!("[Binary Unix: {}] Started", path);
        loop {
            let (socket, _) = listener.accept().await?;
            tokio::spawn(handle_binary_stream(socket, central_tx.clone()));
        }
    }

    #[cfg(not(unix))]
    {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
        println!("[Binary TCP: {}] Started", port);
        loop {
            let (socket, _) = listener.accept().await?;
            tokio::spawn(handle_binary_stream(socket, central_tx.clone()));
        }
    }
}

// Lógica de procesamiento de BinaryFrame - Genérica
async fn handle_binary_stream<S>(mut socket: S, tx: mpsc::Sender<CentralEvent>)
    where S: AsyncRead + AsyncWrite + Unpin + Send + 'static
{
    let mut header_buf = [0u8; 24];
    let mut payload_buffer = BytesMut::with_capacity(65536);

    loop {
        if socket.read_exact(&mut header_buf).await.is_err() {
            break;
        }

        let len = u32::from_be_bytes(header_buf[18..22].try_into().unwrap()) as usize;
        payload_buffer.resize(len, 0);

        if socket.read_exact(&mut payload_buffer).await.is_err() {
            break;
        }

        let payload_bytes = payload_buffer.split_to(len).freeze();
        let frame = BinaryFrame::from_raw(&header_buf, payload_bytes);

        // Feedback al socket (Benchmark)
        if socket.write_u8(1).await.is_err() {
            break;
        }

        if tx.send(CentralEvent::RouteBinary { frame }).await.is_err() {
            break;
        }
    }
}
