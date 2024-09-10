mod config;
mod drawer;
mod error;
mod event;
mod handle;
mod summary;
pub use error::*;
use std::{
    net::TcpListener,
    sync::{mpsc, Arc, Mutex},
    thread,
};
fn main() -> Result<()> {
    let (cfg, routings) = config::read_config("multi3.toml").unwrap();

    let (tx, rx) = mpsc::channel();

    let cfg = &*Box::leak(Box::new(cfg));
    let id = Arc::new(Mutex::new(0));
    for config::Routing { group, host, pool } in routings {
        let pool = Arc::new(pool);
        for socket in host {
            let pool = pool.clone();
            let tx = tx.clone();
            let id = id.clone();
            thread::spawn(move || {
                println!("Listening on: {}", socket);
                let listener = match TcpListener::bind(&socket) {
                    Ok(listener) => listener,
                    Err(e) => {
                        println!("Failed to bind to {}: {}", socket, e);
                        return;
                    }
                };
                for stream in listener.incoming() {
                    let pool = pool.clone();
                    let tx = tx.clone();
                    if let Ok(stream) = stream {
                        let mut id = id.lock().unwrap();
                        *id += 1;
                        let id = id.clone();
                        thread::spawn(move || handle::handle(id, group, stream, cfg, pool, tx));
                    }
                }
            });
        }
    }
    drawer::init(rx, cfg.tui)?;
    if let Some(socket) = cfg.lookup {
        thread::spawn(move || summary::start_summary_server(socket));
    }
    while tx.send((0, 114514, event::Event::Done())).is_ok() {
        thread::sleep(drawer::FRAME_INTERVAL);
    }
    println!("Shutting down");
    Ok(())
}
