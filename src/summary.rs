use crate::drawer::SUMMARY;
use crate::Result;
use std::io::{self, prelude::*};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;

pub fn start_summary_server(socket: SocketAddr) -> io::Result<()> {
    let listener = TcpListener::bind(socket)?;
    println!("Listening on port {}", socket);
    for stream in listener.incoming() {
        if let Ok(stream) = stream {
            thread::spawn(move || handle(stream));
        }
    }
    Ok(())
}
pub fn handle(mut stream: TcpStream) -> Result<()> {
    let mut buffer = [0; 512];
    let n = stream.read(&mut buffer)?;
    let group: i32 = String::from_utf8_lossy(&buffer[..n]).parse()?;
    let summary = SUMMARY.lock().unwrap();
    let (ul, dl) = summary.lookup_group(group).unwrap_or((0, 0));
    stream.write(format!("{{\"ul\":{ul},\"dl\":{dl}}}",).as_bytes())?;
    Ok(())
}
