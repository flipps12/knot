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

    let socket_result = connect_to_app(port).await;

    match socket_result {
        Ok(mut stream) => {
            let (tx, mut rx) = mpsc::channel::<Bytes>(100);
            lock.insert(port, tx.clone());

            // Tarea de fondo: Genérica para cualquier transporte
            tokio::spawn(async move {
                while let Some(payload) = rx.recv().await {
                    // Aquí payload es Bytes, el write_all es eficiente (Zero-copy al kernel)
                    if stream.write_all(&payload).await.is_err() {
                        break;
                    }
                }
            });

            let _ = tx.send(data).await;
        }
        Err(e) => eprintln!("[Error] Fallo de conexión a App (puerto/path {}): {}", port, e),
    }
}

async fn connect_to_app(port: u16) -> Result<Box<dyn tokio::io::AsyncWrite + Send + Unpin>, std::io::Error> {
    let force_tcp = std::env::var("KNOT_DEBUG_TCP").is_ok();

    #[cfg(unix)]
    {
        if !force_tcp {
            let path = format!("/tmp/knot_app_{}.sock", port);
            if std::path::Path::new(&path).exists() {
                use tokio::net::UnixStream;

                let stream = UnixStream::connect(path).await?;
                return Ok(Box::new(stream) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>);
            }
        }
    }

    // Fallback a TCP
    let stream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;
    Ok(Box::new(stream) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>)
}