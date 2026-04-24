#![feature(duration_millis_float)]
mod config;
mod error;
mod handler;
mod tls;
mod tracker;
mod ui;

use error::{Error, Result};

#[tokio::main]
async fn main() {
    if let Err(e) = main_wrapper().await {
        eprintln!("{}", e);
    }
}

async fn main_wrapper() -> Result<()> {
    let (host, cfg, tui) = config::read_config("multi3.toml")?;

    let cfg = &*Box::leak(Box::new(cfg));
    let tracker = tracker::Tracker::new();

    if tui {
        let tracker_clone = tracker.clone();
        let _ui_handle = tokio::task::spawn_blocking(move || {
            if let Err(e) = ui::run_ui(tracker_clone) {
                eprintln!("UI error: {}", e);
            }
            std::process::exit(0);
        });
    } else {
        env_logger::init();
    }

    log::info!("Listening on {}", host.host);
    let listener = tokio::net::TcpListener::bind(host.host).await?;

    let mut id = 0;
    loop {
        id += 1;
        let (stream, _) = listener.accept().await?;
        let tracker = tracker.clone();
        let _join_handle = tokio::spawn(handler::handle(id, stream, cfg, tracker));
    }
}
