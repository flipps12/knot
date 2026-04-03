// src/ingress/socket.rs

use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::codec::{Framed, LinesCodec, LengthDelimitedCodec};
use bytes::Bytes;
use futures::{StreamExt, SinkExt}; // Para el .next() del stream
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use crate::KnotMessage;
use crate::utils::framing::BinaryFrame;

// Mensajes que el SERVIDOR envía a la CENTRAL
#[derive(Debug)]
enum CentralEvent {
    Register { port: u16, tx: mpsc::Sender<ServerCommand> },
    Alert { port: u16, msg: String },
    New { address: String, port: u16 },
    NewChannel { address: String, port: u16 },
    RouteBinary { from_ip: String, frame: BinaryFrame }    
}

// Mensajes que la CENTRAL envía al SERVIDOR (Las "vueltas")
#[derive(Debug)]
enum ServerCommand {
    SendMessage { text: String },
}

// El estado compartido que guardará todos los canales de retorno
type ServerRegistry = Arc<Mutex<HashMap<u16, mpsc::Sender<ServerCommand>>>>;
#[derive(Deserialize, Debug)]
struct Message {
    //id: u32,
    command: String,
    value: String,
}

#[derive(Serialize, Debug)]
struct ResponseTcp {
    command: String,
    value: String,
}

// #[derive(Deserialize, Serialize, Debug)]
// struct ResponseSocket {
//     command: String,
//     value: String,
// }

pub async fn start_ingress(mut rx: mpsc::Receiver<Vec<u8>>, hub_tx: mpsc::Sender<KnotMessage>, port: u16, binary_port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (central_tx, mut central_rx) = mpsc::channel::<CentralEvent>(100);
    let registry: ServerRegistry = Arc::new(Mutex::new(HashMap::new()));

    let registry_for_central = Arc::clone(&registry);
    let central_tx_clone = central_tx.clone();

    // TASK HUB



    // TASK CENTRAL: El controlador
    tokio::spawn(async move {
        while let Some(event) = central_rx.recv().await {
            match event {
                CentralEvent::Register { port, tx } => {
                    println!("  Central: Registrando servidor en puerto {}", port);
                    let mut reg = registry_for_central.lock().await;
                    reg.insert(port, tx);
                }
                CentralEvent::Alert { port, msg } => {
                    println!("  Alerta de [{}]: {}", port, msg);
                    
                    // EJEMPLO DE VUELTA: Si recibimos una alerta, enviamos un mensaje de confirmación
                    let reg = registry_for_central.lock().await;
                    if let Some(server_tx) = reg.get(&port) {
                        let _ = server_tx.send(ServerCommand::SendMessage {
                            text: format!("msg: {} ", msg)
                        }).await;
                    }
                }
                CentralEvent::New { address, port } => {
                    println!("New channel 127.0.0.1:{} create connection with {}", port, address);

                    let tx = central_tx_clone.clone();
                    let reg = Arc::clone(&registry_for_central);
                    tokio::spawn(async move {
                        if let Err(e) = start_managed_server(tx, reg, 0, false, address).await {
                            eprintln!("Error en servidor hijo: {}", e);
                        }
                    });
                }
                CentralEvent::NewChannel { address, port } => {
                    // println!("New datachannel 127.0.0.1:{} create connection with {}", port, address);

                    let tx = central_tx_clone.clone();
                    // let reg = Arc::clone(&registry_for_central);
                    tokio::spawn(async move {
                        if let Err(e) = start_binary_data_server(tx, port).await {
                            eprintln!("Error en servidor hijo: {}", e);
                        }
                    });
                }
                CentralEvent::RouteBinary { from_ip, frame } => {
                    println!("  Data form {} to {}", from_ip, frame.peer_id);
                    
                    // Aquí buscarías en tu Registro quién tiene ese PeerID y le mandas el SendRaw
                    let _ = hub_tx.send(KnotMessage::ClientData { from_ip, frame }).await;
                    
                }
            }
        }
    });

    
    let tx = central_tx.clone();
    let txx = central_tx.clone();
    let reg = Arc::clone(&registry);
    tokio::spawn(async move {
        if let Err(e) = start_managed_server(tx, reg, port, true, "".to_string()).await {
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

async fn start_managed_server(
    central_tx: mpsc::Sender<CentralEvent>,
    _registry: ServerRegistry, // Mantener si se usa en otro lado, sino ignorar
    port: u16,
    is_main: bool,
    response_address: String, // Dirección a donde este server responde/envía
) -> Result<(), Box<dyn std::error::Error>> {
    
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;

    // Creamos el canal para recibir órdenes de la Central
    let (_server_tx, mut server_rx) = mpsc::channel::<ServerCommand>(10);

    // 2. TASK DE ESCUCHA DE LA CENTRAL (Vueltas)
    // Esta tarea ahora puede enviar mensajes al exterior
    let response_address_clone = response_address.clone();
    tokio::spawn(async move {
        while let Some(command) = server_rx.recv().await {
            match command {
                ServerCommand::SendMessage { text } => {
                    println!("[Servidor {}] Orden de envío recibida: {}", port, text);
                    
                    // Si no es el main, intentamos enviar el mensaje al response_address
                    if !is_main {
                        match TcpStream::connect(&response_address_clone).await {
                            Ok(stream) => {
                                let mut framed = Framed::new(stream, LinesCodec::new());
                                // Enviamos el texto envuelto en un JSON o como línea cruda
                                if let Err(e) = framed.send(&text).await {
                                    eprintln!("Error al reenviar mensaje: {}", e);
                                }
                            }
                            Err(e) => eprintln!("No se pudo conectar a {}: {}", response_address_clone, e),
                        }
                    }
                }
            }
        }
    });

    println!("[Servidor {}] Started", port);

    // 3. LOOP DE ACEPTACIÓN
    loop {
        let (socket, addr) = listener.accept().await?;
        let tx_clone = central_tx.clone();
        // let is_main_flag = is_main;

        tokio::spawn(async move {
            let _ = tx_clone.send(CentralEvent::Alert { 
                port: port, 
                msg: format!("Cliente {} conectado", addr) 
            }).await;
            
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
                                "newchannel" => {
                                    // Solo si el protocolo lo permite o si es el main
                                    let _ = tx_clone.send(CentralEvent::New { 
                                        address: req.value, 
                                        port: port 
                                    }).await;
                                    let _ = framed.send("Comando New enviado a Central").await;
                                    return; 
                                },
                                "newdatachannel" => {
                                    // Solo si el protocolo lo permite o si es el main
                                    let _ = tx_clone.send(CentralEvent::NewChannel { 
                                        address: req.value, 
                                        port: 0
                                    }).await;
                                    let _ = framed.send("Comando NewData enviado a Central").await;
                                    return; 
                                },
                                "forward_to_central" => {
                                    // Este comando hace que este server le pida a la central 
                                    // que mande un mensaje a OTRO server usando SendMessage
                                    let _ = tx_clone.send(CentralEvent::Alert { 
                                        port: port, 
                                        msg: format!("RETRANSMIT: {}", req.value) 
                                    }).await;
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

async fn start_binary_data_server(
    central_tx: mpsc::Sender<CentralEvent>,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    
    println!("[Servidor {}] Started", port);

    loop {
        let (mut socket, addr) = listener.accept().await?;
        let tx = central_tx.clone();



        tokio::spawn(async move {
            let mut header_buf = [0u8; 24]; // Ahora el header mide 16 bytes

            loop {
                // 1. Leer Header
                if socket.read_exact(&mut header_buf).await.is_err() { break; }

                // 2. Extraer longitud del payload (está en los bytes 18 al 22)
                let len = u32::from_be_bytes(header_buf[18..22].try_into().unwrap()) as usize;

                // 3. Leer Payload directamente a un buffer de bytes
                let mut payload_raw = vec![0u8; len];
                if socket.read_exact(&mut payload_raw).await.is_err() { break; }
                
                // 4. Convertir a Bytes (Zero-Copy para el envío)
                let payload_bytes = Bytes::from(payload_raw);

                // 5. Construir el Frame usando su propio método
                let frame = BinaryFrame::from_raw(&header_buf, payload_bytes);

                // 6. Enviar el objeto completo a la Central
                // El envío de 'frame' no copia el payload, solo el puntero
                let _ = tx.send(CentralEvent::RouteBinary {
                    from_ip: addr.to_string(),
                    frame, // Enviamos la estructura completa
                }).await;
            }
        });
    }
}
