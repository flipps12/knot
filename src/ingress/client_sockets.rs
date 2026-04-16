// src/ingress/client_sockets.rs

use bytes::Bytes;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::ingress::socket::ConnectionMap;

pub async fn send_to_local_app(connections: ConnectionMap, port: u16, data: Bytes) {
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
                        break;
                    }
                }
            });

            // Enviamos el primer paquete (el que disparó la conexión)
            let _ = lock.get(&port).unwrap().send(data).await;
        }
        Err(e) => eprintln!("[Error] No se pudo conectar al puerto {}: {}", port, e),
    }
}
