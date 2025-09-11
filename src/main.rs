#![feature(result_option_map_or_default)]
mod config;
mod drawer;
mod error;
mod event;
mod handle;
pub use error::*;
use std::{
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    thread,
};

fn main() {
    let (cfg, routings) = config::read_config("multi3.toml").unwrap();

    let (tx, rx) = mpsc::channel();

    let cfg = &*Box::leak(Box::new(cfg));
    let id = Arc::new(Mutex::new(0));
    for config::Routing { host, pool } in routings {
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
                        thread::spawn(move || handle::handle(id, stream, cfg, pool, tx));
                    }
                }
            });
        }
    }
    if cfg.tui {
        thread::spawn(move || drawer::drawer(rx));
        while tx.send((0, event::Event::Done())).is_ok() {
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
    println!("Shutting down");
}
