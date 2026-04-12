use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::codec::{Framed, LinesCodec};
use bytes::BytesMut;
use futures::{StreamExt, SinkExt};
use tokio::sync::mpsc;

use crate::ingress::socket::{CentralEvent, Message, ResponseTcp};
use crate::utils::framing::BinaryFrame;
use crate::utils::tou64::string_to_u64_rust;

pub async fn start_managed_server(
    central_tx: mpsc::Sender<CentralEvent>,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;

    println!("[Servidor {}] Started", port);

    loop {
        let (socket, addr) = listener.accept().await?;
        let tx_clone = central_tx.clone();

        tokio::spawn(async move {            
            let mut framed = Framed::new(socket, LinesCodec::new());

            while let Some(result) = framed.next().await {
                match result {
                    Ok(linea) => {
                        if let Ok(req) = serde_json::from_str::<Message>(&linea) {
                            
                            match req.command.as_str() {
                                "status" => {
                                    let resp = ResponseTcp { command: "REPORT".into(), value: "OK".into() };
                                    let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                                },
                                "newappname" => {
                                    
                                    let app_id = string_to_u64_rust(&req.value); 
                                    let _ = tx_clone.send(CentralEvent::Register { app_id, port: req.port }).await;
                                    
                                    let response_text = format!("OK: Registered ID {}", app_id);
                                    let _ = framed.send(response_text).await;
                                    return; 
                                },
                                "connect" => {
                                    let _ = tx_clone.send(CentralEvent::Connect { addr: req.value}).await;

                                    let _ = framed.send("trying...").await;
                                    return; 
                                },
                                "discover" => {
                                    let _ = tx_clone.send(CentralEvent::Discover { peerid: req.value}).await;

                                    let _ = framed.send("trying...").await;
                                    return; 
                                },
                                "connectrelay" => {
                                    let _ = tx_clone.send(CentralEvent::ConnectRelay { relay_addr: "/ip4/192.168.0.46/tcp/4001".parse().unwrap(), relay_peer_id: req.value.parse().unwrap() }).await;

                                    let _ = framed.send("trying...").await;
                                    return; 
                                },
                                _ => {
                                    let _ = framed.send("Comando desconocido").await;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Conexión cerrada con {}: {}", addr, e);
                        break;
                    }
                }
            }
        });
    }
}

pub async fn start_binary_data_server(
    central_tx: mpsc::Sender<CentralEvent>,
    port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    
    // Este log queda porque solo se ejecuta una vez al inicio
    println!("[Servidor {}] Ingress Binario iniciado", port);

    loop {
        let (mut socket, addr) = listener.accept().await?;
        let tx = central_tx.clone();

        tokio::spawn(async move {
            let mut header_buf = [0u8; 24];
            // Reservamos capacidad inicial para evitar allocs pequeños
            let mut payload_buffer = BytesMut::with_capacity(65536); 

            loop {
                // 1. Leer Header
                if socket.read_exact(&mut header_buf).await.is_err() { break; }

                // 2. Extraer longitud (Offset 18-22 según tu framing.rs)
                let len = u32::from_be_bytes(header_buf[18..22].try_into().unwrap()) as usize;

                // 3. Optimización de Memoria: Leer directamente al Buffer
                payload_buffer.resize(len, 0); 
                if socket.read_exact(&mut payload_buffer).await.is_err() { break; }
                
                // freeze() convierte BytesMut en Bytes (atómico y sin copia)
                let payload_bytes = payload_buffer.split_to(len).freeze();

                // 4. Construir el Frame
                let frame = BinaryFrame::from_raw(&header_buf, payload_bytes);

                // 5. Logs solo en modo DEBUG (No afectan al benchmark --release)
                #[cfg(debug_assertions)]
                println!("[Ingress] Data de {} para ID: {}", addr, frame.peer_id);


                // For benchmark only
                if socket.write_u8(1).await.is_err() { break; }

                // 6. Enviar a la Central
                if tx.send(CentralEvent::RouteBinary {
                    from_ip: addr.to_string(),
                    frame,
                }).await.is_err() { break; }
            }
        });
    }
}