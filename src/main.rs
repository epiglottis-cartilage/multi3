mod config;
mod handle;
// mod logger;
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let (host, cfg) = config::read_config("multi3.toml").unwrap();

    let cfg = &*Box::leak(Box::new(cfg));
    let listener = tokio::net::TcpListener::bind(host.host).await?;

    loop {
        let (stream, _) = listener.accept().await?;
        let cfg = &*cfg;
        tokio::spawn(async move { handle::handle(stream, cfg).await });
    }
    // println!("Shutting down");
}
