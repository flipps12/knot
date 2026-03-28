// src/ingress/socket.rs

use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::codec::{Framed, FramedRead, LinesCodec};
use futures::{StreamExt, SinkExt}; // Para el .next() del stream
use serde::{Deserialize, Serialize};

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

pub async fn startMainSocket()-> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    println!("Servidor escuchando en el puerto 8080...");

    loop {
        // 2. Esperamos una nueva conexión sin bloquear el hilo del sistema
        let (mut socket, addr) = listener.accept().await?;
        println!("Nueva conexión desde: {}", addr);

        // 3. Usamos 'tokio::spawn' para manejar cada cliente en una "tarea" ligera
        tokio::spawn(async move {
            let mut framed = Framed::new(socket, LinesCodec::new());

            // Loop interno para leer paquetes de este cliente específico
            while let Some(result) = framed.next().await {
                match result {
                    Ok(linea) => {
                        if let Ok(req) = serde_json::from_str::<Message>(&linea) {
                            println!("Petición recibida: {:?}", req);

                            let (cmd, val) = match req.command.as_str() {
                                "newchannel" => ("TCP".to_string(), "127.0.0.1:1234".to_string()),
                                "status" => ("REPORT".to_string(), "All systems GO".to_string()),
                                "echo"   => ("ECHO".to_string(), req.value), 
                                _        => ("UNKNOWN".to_string(), "Comando inválido".to_string()),
                            };

                            let resp = ResponseTcp {
                                command: cmd,
                                value: val,
                            };

                            // return response
                            if let Ok(json_resp) = serde_json::to_string(&resp) {
                                // .send() envía el string y añade el '\n' automáticamente por el LinesCodec
                                if let Err(e) = framed.send(json_resp).await {
                                    eprintln!("Error al enviar respuesta: {}", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error de conexión: {}", e);
                        break;
                    }
                }
            }
        });
    }
}
