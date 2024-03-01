use std::{
    net::TcpListener,
    sync::{mpsc, Arc},
    thread,
};

mod config;
mod drawer;
mod event;
mod handle;
fn main() {
    let (host, cfg) = config::read_config("multi3.toml").unwrap();

    let listener = TcpListener::bind(host.host).unwrap();
    println!("Listening {}", host.host);

    let (tx, rx) = mpsc::channel();
    let tx_ = tx.clone();

    thread::spawn(move || {
        let cfg = Arc::new(cfg);
        let mut id = 1;
        for stream in listener.incoming().flatten() {
            let config = cfg.clone();
            let tx__ = tx_.clone();
            thread::spawn(move || handle::handle(id, stream, config, &tx__));
            id += 1;
        }
    });

    if host.tui {
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
