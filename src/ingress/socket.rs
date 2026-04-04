// src/ingress/socket.rs

use serde::de::value;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::codec::{Framed, LinesCodec, LengthDelimitedCodec};
use bytes::{Bytes, BytesMut};
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
    Register { app_id: u64, port: u16 },
    NewChannel { address: String, port: u16 },
    RouteBinary { from_ip: String, frame: BinaryFrame }    
}

// Mensajes que la CENTRAL envía al SERVIDOR (Las "vueltas")
#[derive(Debug)]
enum ServerCommand {
    SendMessage { text: String },
}

// El estado compartido que guardará todos los canales de retorno
type ServerRegistry = Arc<Mutex<HashMap<u64, u16>>>;
#[derive(Deserialize, Debug)]
struct Message {
    //id: u32,
    command: String,
    value: String,
    port: u16,
}

#[derive(Serialize, Debug)]
struct ResponseTcp {
    command: String,
    value: String,
}

pub enum IngressCommand {
    SendFrameToClient { 
        from_ip: String, 
        frame: Bytes 
    },
}

type ConnectionMap = Arc<Mutex<HashMap<u16, mpsc::Sender<Bytes>>>>;

// #[derive(Deserialize, Serialize, Debug)]
// struct ResponseSocket {
//     command: String,
//     value: String,
// }

pub async fn start_ingress(mut rx: mpsc::Receiver<IngressCommand>, hub_tx: mpsc::Sender<KnotMessage>, port: u16, binary_port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (central_tx, mut central_rx) = mpsc::channel::<CentralEvent>(100);
    let registry: ServerRegistry = Arc::new(Mutex::new(HashMap::new()));

    let registry_for_central = Arc::clone(&registry);
    let central_tx_clone = central_tx.clone();

    let local_connections: ConnectionMap = Arc::new(Mutex::new(HashMap::new()));

    // TASK HUB
    let conns = Arc::clone(&local_connections);
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                IngressCommand::SendFrameToClient { from_ip, frame } => {
                    let buf = BytesMut::from(&frame[..]);
                    if buf.len() < 24 {
                        eprintln!("Error: El frame es demasiado corto para el encabezado de Knot");
                        return;
                    }
                    let decoded_frame = BinaryFrame::decode(buf);
                    let app_id = decoded_frame.app_id;


                    #[cfg(debug_assertions)]
                    println!("Entrante desde {} hasta appid: {}", from_ip, app_id);

                    let reg = registry_for_central.lock().await;
                    if let Some(&target_port) = reg.get(&app_id) {
                        println!("  -> Reenviando a aplicación local en puerto: {}", target_port);
                        send_to_local_app(Arc::clone(&conns), target_port, decoded_frame.payload).await;
                    } else {
                        println!("  -> AppID {} no está registrada.", app_id);
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
                    println!("[Ingress] new appname on hashmap: {} -> {}", app_id, port);
                    let mut reg = reg_handle.lock().await;
                    reg.insert(app_id, port);
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
                    #[cfg(debug_assertions)]
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
                                    let _ = tx_clone.send(CentralEvent::Register { app_id, port: req.port }).await;let _ = framed.send("Ok").await;
                                    
                                    let response_text = format!("OK: Registered ID {}", app_id);
                                    let _ = framed.send(response_text).await;
                                    return; 
                                },
                                // "getappname" => {
                                    
                                //     let _ = framed.send("Comando New enviado a Central").await;
                                //     return; 
                                // },
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

fn string_to_u64_rust(text: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut s = DefaultHasher::new();
    text.hash(&mut s);
    s.finish()
}

pub async fn send_to_local_app(
    connections: ConnectionMap,
    port: u16,
    data: Bytes
) {
    let mut lock = connections.lock().await;

    // 1. Intentar obtener el canal existente
    if let Some(tx) = lock.get(&port) {
        if tx.send(data.clone()).await.is_ok() {
            return; // Enviado con éxito
        }
        // Si el canal falló (socket cerrado), lo sacamos del mapa
        lock.remove(&port);
    }

    // 2. Si no hay conexión o falló, abrimos una nueva
    match TcpStream::connect(format!("127.0.0.1:{}", port)).await {
        Ok(mut stream) => {
            let (tx, mut rx) = mpsc::channel::<Bytes>(100);
            
            // Guardamos el canal en el mapa antes de lanzar el hilo
            lock.insert(port, tx.clone());
            
            // Tarea de fondo: Escribe en el socket mientras el canal reciba datos
            tokio::spawn(async move {
                while let Some(payload) = rx.recv().await {
                    if let Err(_) = stream.write_all(&payload).await {
                        break; // Error de red, cerramos esta tarea
                    }
                }
            });

            // Enviamos el primer paquete (el que disparó la conexión)
            let _ = lock.get(&port).unwrap().send(data).await;
        }
        Err(e) => eprintln!("[Error] No se pudo conectar al puerto {}: {}", port, e),
    }
}