// src/ingress/socket.rs

use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::codec::{Framed, FramedRead, LinesCodec};
use futures::{StreamExt, SinkExt}; // Para el .next() del stream
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

// Mensajes que el SERVIDOR envía a la CENTRAL
#[derive(Debug)]
enum CentralEvent {
    Register { port: u16, tx: mpsc::Sender<ServerCommand> },
    Alert { port: u16, msg: String },
    New { address: String, port: u16 },
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

pub async fn start() -> Result<(), Box<dyn std::error::Error>> {
    let (central_tx, mut central_rx) = mpsc::channel::<CentralEvent>(100);
    let registry: ServerRegistry = Arc::new(Mutex::new(HashMap::new()));


    let registry_for_central = Arc::clone(&registry);
    let central_tx_clone = central_tx.clone();

    // TASK CENTRAL: El controlador
    tokio::spawn(async move {
        while let Some(event) = central_rx.recv().await {
            match event {
                CentralEvent::Register { port, tx } => {
                    println!("🧠 Central: Registrando servidor en puerto {}", port);
                    let mut reg = registry_for_central.lock().await;
                    reg.insert(port, tx);
                }
                CentralEvent::Alert { port, msg } => {
                    println!("📢 Alerta de [{}]: {}", port, msg);
                    
                    // EJEMPLO DE VUELTA: Si recibimos una alerta, enviamos un mensaje de confirmación
                    let reg = registry_for_central.lock().await;
                    if let Some(server_tx) = reg.get(&port) {
                        let _ = server_tx.send(ServerCommand::SendMessage {
                            text: "Recibido, buen trabajo.".to_string()
                        }).await;
                    }
                }
                CentralEvent::New { address, port } => {
                    println!("New channel {} -> 127.0.0.1:{}", address, port);

                    let tx = central_tx_clone.clone();
                    let reg = Arc::clone(&registry_for_central);
                    tokio::spawn(async move {
                        if let Err(e) = start_managed_server(tx, reg).await {
                            eprintln!("Error en servidor hijo: {}", e);
                        }
                    });
                }
            }
        }
    });

    // Lanzar los 20 servidores pasándoles el registro y el canal central
    for _ in 0..20 {
        let tx = central_tx.clone();
        let reg = Arc::clone(&registry);
        tokio::spawn(async move {
            if let Err(e) = start_managed_server(tx, reg).await {
                eprintln!("Error en servidor: {}", e);
            }
        });
    }

    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn start_managed_server(
    central_tx: mpsc::Sender<CentralEvent>,
    registry: ServerRegistry,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = registry;
    
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    // Creamos el canal por el cual este servidor recibirá órdenes
    let (server_tx, mut server_rx) = mpsc::channel::<ServerCommand>(10);

    // 1. REGISTRO: Enviamos nuestro canal de retorno a la Central
    central_tx.send(CentralEvent::Register { 
        port, 
        tx: server_tx 
    }).await?;

    // 2. TASK DE ESCUCHA (Vueltas de la Central)
    // Este bloque procesa lo que el "Cerebro" nos mande
    tokio::spawn(async move {
        while let Some(command) = server_rx.recv().await {
            match command {
                ServerCommand::SendMessage { text } => {
                    println!("[Servidor {}] La central dice: {}", port, text);
                    // Aquí podrías enviar esto a los clientes conectados si quisieras
                }
            }
        }
    });

    // 3. LOOP DE ACEPTACIÓN (Igual que antes)
    loop {
        let (socket, addr) = listener.accept().await?;
        let tx_clone = central_tx.clone();
        
        tokio::spawn(async move {
            let _ = tx_clone.send(CentralEvent::Alert { 
                port, 
                msg: format!("Cliente {} conectado", addr) 
            }).await;
            
            let mut framed = Framed::new(socket, LinesCodec::new());
            

            while let Some(result) = framed.next().await {
                match result {
                    Ok(linea) => {
                        println!("[Servidor {}] Datos crudos: {}", port, linea);

                        if let Ok(req) = serde_json::from_str::<Message>(&linea) {
                            let (cmd, val) = match req.command.as_str() {
                                "status"     => ("REPORT".to_string(), "OK".to_string()),
                                "echo"       => ("ECHO".to_string(), req.value), 
                                "newchannel" => {
                                    let _ = tx_clone.send(CentralEvent::New {address: req.value, port}).await;
                                    let _ = framed.send("Creando nuevo servidor dinámico...").await;
                                    return ()
                                },
                                _            => ("UNKNOWN".to_string(), "Comando inválido".to_string()),
                            };

                            let resp = ResponseTcp { command: cmd.to_string(), value: val.to_string() };

                            if let Ok(json_resp) = serde_json::to_string(&resp) {
                                if let Err(e) = framed.send(json_resp).await {
                                    eprintln!("Error al enviar: {}", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Conexión perdida con {}: {}", addr, e);
                        break;
                    }
                }
            }
        });
    }
}