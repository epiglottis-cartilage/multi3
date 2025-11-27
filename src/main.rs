#![feature(can_vector)]
mod config;
mod drawer;
mod error;
mod event;
mod handler;
mod tls;
mod utils;
pub use error::*;
use std::{
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    thread,
};

fn main() {
    let (cfg, pool) = match config::read_config("multi3.toml") {
        Ok(x) => x,
        Err(e) => {
            println!("Failed to read config: {}", e);
            return;
        }
    };

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
                    tx.send((0, event::Event::Error(e.into()))).unwrap();
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
                    thread::spawn(move || {
                        if let Err(e) = handler::handle(id, stream, &(cfg, pool), &tx) {
                            println!("{}", e);
                        }
                    });
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
