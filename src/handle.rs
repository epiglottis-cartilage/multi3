use super::config;
use super::event::Event;
use anyhow::Result;
use std::{
    io::{self, prelude::*},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::{mpsc, Arc},
    thread,
};

const DEFAULT_BUF_SIZE: usize = 40960;
const HTTPS_HEADER: &str = "CONNECT";

pub fn handle(
    id: usize,
    local: TcpStream,
    config: Arc<config::HandlerConfig>,
    reporter: &mpsc::Sender<(usize, Event)>,
) {
    match inner_handle(id, local, config, reporter) {
        Err(e) => {
            let _ = reporter.send((id, Event::Error(e.to_string().into())));
        }
        Ok(()) => {}
    }
}

fn inner_handle(
    id: usize,
    mut local: TcpStream,
    config: Arc<config::HandlerConfig>,
    reporter: &mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    reporter.send((id, Event::Received(local.peer_addr()?.ip())))?;
    local.set_read_timeout(Some(config.io_ttl))?;
    local.set_write_timeout(Some(config.io_ttl))?;

    let mut is_https = false;

    let uri = {
        let mut buffer = [0u8; DEFAULT_BUF_SIZE];
        let n = local.peek(&mut buffer)?;
        let request = String::from_utf8_lossy(&buffer[..n]);
        let mut request_split = request.split_ascii_whitespace();

        let head = request_split.next();
        let uri = request_split
            .skip_while(|x| !x.eq_ignore_ascii_case("Host:"))
            .nth(1);
        let mut uri = match uri {
            None => {
                reporter.send((id, Event::Error(format!("No host in {}", request).into())))?;
                local.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")?;

                return Ok(());
            }
            Some(x) => x,
        }
        .to_owned();

        if (uri.starts_with('[') && uri.ends_with(']')) || (!uri.contains(':')) {
            uri += ":80";
        }

        if head.unwrap().eq_ignore_ascii_case(HTTPS_HEADER) {
            // consume the CONNECT package of https request.
            let _ = local.read(&mut buffer)?;
            is_https = true;
        }

        uri
    };

    reporter.send((id, Event::Resolved(uri.clone())))?;

    let remote = {
        let hosts = match uri.to_socket_addrs() {
            Ok(x) => x,
            Err(e) => {
                reporter.send((id, Event::Error(format!("DNS fail:{}", e).into())))?;
                local.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n")?;
                return Ok(());
            }
        };
        let hosts = match config.ipv6_first {
            None => hosts,
            Some(ipv6_first) => {
                let mut hosts: Vec<_> = hosts.collect::<Vec<_>>();
                hosts.sort_by(|a, b| {
                    if ipv6_first {
                        a.is_ipv4().cmp(&b.is_ipv4())
                    } else {
                        a.is_ipv6().cmp(&b.is_ipv6())
                    }
                });
                hosts.into_iter()
            }
        };

        let mut remote = None;
        for host in hosts {
            use socket2::{Domain, Protocol, Socket, Type};

            let (to_bind, builder) = match host {
                SocketAddr::V4(_) => {
                    let local: SocketAddr = (config.next_ip4(), 0).into();
                    let builder = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
                    (local, builder)
                }
                SocketAddr::V6(_) => {
                    let local: SocketAddr = (config.next_ip6(), 0).into();
                    let builder = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
                    (local, builder)
                }
            };

            if builder.bind(&to_bind.into()).is_err() {
                reporter.send((id, Event::Retry()))?;
                continue;
            }

            match builder.connect_timeout(&host.into(), config.connect_ttl) {
                Ok(()) => {
                    remote = Some(builder);
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    reporter.send((id, Event::Error("Timeout".into())))?;
                    local.write_all(b"HTTP/1.1 504 Gateway Time-out\r\n\r\n")?;
                    return Ok(());
                }
                Err(_) => {
                    reporter.send((id, Event::Retry()))?;
                }
            }
        }
        match remote {
            None => {
                reporter.send((id, Event::Error("Fail to connect".into())))?;
                local.write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n")?;
                return Ok(());
            }
            Some(x) => x,
        }
    };

    reporter.send((
        id,
        Event::Connected(
            remote.local_addr()?.as_socket().unwrap().ip(),
            remote.peer_addr()?.as_socket().unwrap().ip(),
        ),
    ))?;

    if is_https {
        // answer to CONNECT
        local.write_all(b"HTTP/1.1 200 OK\r\n\r\n")?;
    }

    remote.set_read_timeout(Some(config.io_ttl))?;
    remote.set_write_timeout(Some(config.io_ttl))?;

    {
        let reporter_up = reporter.clone();
        let local_ = local.try_clone()?;
        let remote_ = remote.try_clone()?;
        let up = thread::spawn(move || copy_up(id, local_, remote_, reporter_up));

        let reporter_down = reporter.clone();
        let down = thread::spawn(move || copy_down(id, remote, local, reporter_down));

        match up.join().and(down.join()).unwrap() {
            Ok(()) => reporter.send((id, Event::Done()))?,
            Err(e) => return Err(e),
        };
    }
    Ok(())
}
fn copy_up(
    id: usize,
    mut from: TcpStream,
    mut to: socket2::Socket,
    reporter: mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    let mut buffer = [0u8; DEFAULT_BUF_SIZE];
    loop {
        match from.read(&mut buffer) {
            Ok(0) => {
                return Ok(());
            }
            Ok(n) => {
                reporter.send((id, Event::Upload(n)))?;
                to.write_all(&buffer[..n])?;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e)
                if e.kind() == io::ErrorKind::TimedOut || e.kind() == io::ErrorKind::WouldBlock =>
            {
                // reporter.send((id, Event::Error("IO timeout".into())))?;
                return Ok(());
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }
}

fn copy_down(
    id: usize,
    mut from: socket2::Socket,
    mut to: TcpStream,
    reporter: mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    let mut buffer = [0u8; DEFAULT_BUF_SIZE];
    loop {
        match from.read(&mut buffer) {
            Ok(0) => {
                return Ok(());
            }
            Ok(n) => {
                reporter.send((id, Event::Download(n)))?;
                to.write_all(&buffer[..n])?;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e)
                if e.kind() == io::ErrorKind::TimedOut || e.kind() == io::ErrorKind::WouldBlock =>
            {
                reporter.send((id, Event::Error("IO timeout".into())))?;
                return Ok(());
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }
}
