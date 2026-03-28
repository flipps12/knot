mod ingress;

use ingress::socket::start;

#[tokio::main]
async fn main() {
    println!("Hello, world!");
    start().await;
}
