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
        let (socket, _addr) = listener.accept().await?;
        let tx_clone = central_tx.clone();

        tokio::spawn(async move {            
            let mut framed = Framed::new(socket, LinesCodec::new());

            while let Some(Ok(linea)) = framed.next().await {
                // Deserialización directa al Enum
                if let Ok(msg) = serde_json::from_str::<Message>(&linea) {
                    match msg {
                        Message::Status => {
                            let resp = ResponseTcp { response: "OK".into() };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                        },
                        Message::Register { name, port } => {
                            let app_id = string_to_u64_rust(&name); 
                            let _ = tx_clone.send(CentralEvent::Register { app_id, port }).await;
                            // let _ = framed.send(format!("OK: Registered ID {}", app_id)).await;
                            let resp = ResponseTcp { response: app_id.to_string() };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                            return; 
                        },
                        Message::Connect { addr } => {
                            let _ = tx_clone.send(CentralEvent::Connect { addr }).await;
                            // let _ = framed.send("Connecting...").await;

                            let resp = ResponseTcp { response: "OK".into() };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                            return;
                        },
                        Message::ConnectRelay { relay_addr, relay_id } => {
                            let _ = tx_clone.send(CentralEvent::ConnectRelay { 
                                relay_addr: relay_addr.parse().unwrap(), 
                                relay_peer_id: relay_id.parse().unwrap() 
                            }).await;
                            // let _ = framed.send("Relay request sent").await;


                            let resp = ResponseTcp { response: "OK".into() };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                            return;
                        },
                        Message::Discover { peer_id } => {
                            let _ = tx_clone.send(CentralEvent::Discover { peerid: peer_id }).await;
                            // let _ = framed.send("Discovery started").await;


                            let resp = ResponseTcp { response: "OK".into() };
                            let _ = framed.send(serde_json::to_string(&resp).unwrap()).await;
                            return;
                        }
                    }
                } else {
                    let _ = framed.send("ERROR: Invalid Command or Params").await;
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