mod config;
mod drawer;
mod error;
mod event;
mod handler;
pub use error::*;
use std::{
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    thread,
};

fn main() {
    let (cfg, pool) = config::read_config("multi3.toml").unwrap();

    let (tx, rx) = mpsc::channel();

    let cfg = &*Box::leak(Box::new(cfg));
    let id = Arc::new(Mutex::new(0));
    let pool = Arc::new(pool);
    let pool = pool.clone();
    {
        let tx = tx.clone();
        let id = id.clone();
        thread::spawn(move || {
            println!("Listening on: {}", &cfg.host);
            let listener = match TcpListener::bind(&cfg.host) {
                Ok(listener) => listener,
                Err(e) => {
                    tx.send((
                        0,
                        event::Event::Error(
                            format!("Failed to bind to {}: {}", cfg.host, e).into(),
                        ),
                    ))
                    .unwrap();
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
                    thread::spawn(move || handler::handle(id, stream, &(cfg, pool), &tx));
                }
            }
        });
    }

    if cfg.tui {
        thread::spawn(move || drawer::drawer(rx));
        while tx.send((0, event::Event::None)).is_ok() {
            thread::sleep(drawer::FRAME_INTERVAL)
        }
    } else {
        while let Ok((id, x)) = rx.recv() {
            match x {
                event::Event::Upload(_) | event::Event::Download(_) => continue,
                _ => {
                    println!("[{:<4}] {:?}", id, x);
                }
            }
        }
    }
}
