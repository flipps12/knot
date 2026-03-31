mod ingress;

use ingress::socket::start;
    
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum KnotMessage {
    // De Ingress al Core: "Tengo datos de un cliente"
    ClientData(Vec<u8>),
    // Del Network al Core: "Recibí algo del P2P"
    NetworkData(Vec<u8>),
    // Comandos de control
    Shutdown,
    Log(String),
}

#[tokio::main]
async fn main() {
    println!("Hello, world!");

    // Main Channel 
    // Ingress -> Core <- Network
    let (hub_tx, mut hub_rx) = mpsc::channel::<KnotMessage>(100);

    // Secondary Channel
    // Core -> Network
    let (to_net_tx, to_net_rx) = mpsc::channel::<Vec<u8>>(100);
    // Core -> Ingress
    let (to_ing_tx, to_ing_rx) = mpsc::channel::<Vec<u8>>(100);

    // Clone rx - tx
    let ing_hub_tx = hub_tx.clone();
    let net_hub_tx = hub_tx.clone();

    // Spawn Workers
    tokio::spawn(start(to_ing_rx, ing_hub_tx));

    // For debug, Core is this
    loop {
        tokio::select! {
            Some(message) = hub_rx.recv() => {
                match message {
                    KnotMessage::ClientData(data) => {
                        println!("Main: Recibidos datos de Ingress, reenviando a Network...");
                        let _ = to_net_tx.send(data).await;
                    }
                    KnotMessage::Log(msg) => {
                        println!("[LOG GLOBAL]: {}", msg);
                    }
                    KnotMessage::Shutdown => {
                        println!("Apagando Knot...");
                        break;
                    }
                    _ => {}
                }
            }
            _ = tokio::signal::ctrl_c() => {
                break; 
            }
        }
    }

    println!("\n\n[Knot] Shutting down");
}
