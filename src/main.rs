#![feature(duration_millis_float)]
mod config;
mod error;
mod handler;
mod tls;
use error::{Error, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let (host, cfg) = config::read_config("multi3.toml")?;

    let cfg = &*Box::leak(Box::new(cfg));
    let listener = tokio::net::TcpListener::bind(host.host).await?;

    let mut id = 0;
    loop {
        id += 1;
        let (stream, _) = listener.accept().await?;
        let _join_handle = tokio::spawn(handler::handle(id, stream, cfg));
    }
}
