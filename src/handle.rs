use crate::config;
use crate::event::Event;
use crate::Result;
use std::{
    io::{self, prelude::*},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs},
    sync::{mpsc, Arc},
    thread,
};

const BUFFER_SIZE: usize = 40960;
const HTTPS_HEADER: &str = "CONNECT";

pub fn handle(
    id: usize,
    local: TcpStream,
    config: &config::Config,
    pool: Arc<config::IpPool>,
    reporter: mpsc::Sender<(usize, Event)>,
) {
    match inner_handle(id, local, config, pool, reporter.clone()) {
        Err(e) => {
            let _ = reporter.send((id, Event::Error(e.to_string().into())));
        }
        Ok(()) => {}
    }
}

fn inner_handle(
    id: usize,
    mut local: TcpStream,
    config: &config::Config,
    pool: Arc<config::IpPool>,
    reporter: mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    reporter.send((id, Event::Received(local.peer_addr()?.ip())))?;
    local.set_read_timeout(Some(config.io_ttl))?;
    local.set_write_timeout(Some(config.io_ttl))?;

    let is_https;

    let uri = {
        #[allow(invalid_value)]
        let mut buffer =
            unsafe { std::mem::MaybeUninit::<[u8; BUFFER_SIZE]>::uninit().assume_init() };
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
        } else {
            is_https = false;
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
        let hosts: Vec<_> = match config.ipv6_first {
            None => hosts.collect(),
            Some(ipv6_first) => {
                let mut v6 = Vec::new();
                let mut v4 = Vec::new();
                hosts.for_each(|socket| match socket {
                    SocketAddr::V4(_) => v4.push(socket),
                    SocketAddr::V6(_) => v6.push(socket),
                });
                if ipv6_first {
                    v6.into_iter().chain(v4.into_iter())
                } else {
                    v4.into_iter().chain(v6.into_iter())
                }
                .collect()
            }
        };
        let time_start = std::time::Instant::now();
        let mut remote = None;
        for host in hosts {
            use socket2::{Domain, Protocol, Socket, Type};
            let local_socket: SocketAddr;
            let builder;
            match host {
                SocketAddr::V4(_) => {
                    local_socket = (pool.pool_v4.next().unwrap_or(Ipv4Addr::UNSPECIFIED), 0).into();
                    builder = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
                }
                SocketAddr::V6(_) => {
                    local_socket = (pool.pool_v6.next().unwrap_or(Ipv6Addr::UNSPECIFIED), 0).into();
                    builder = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
                }
            };

            if builder.bind(&local_socket.into()).is_err() {
                reporter.send((id, Event::Retry()))?;
                continue;
            }

            match builder.connect_timeout(&host.into(), config.connect_ttl) {
                Ok(()) => {
                    remote = Some(builder);
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    if time_start.elapsed() > config.retry_ttl {
                        reporter.send((id, Event::Error("Timeout".into())))?;
                        local.write_all(b"HTTP/1.1 504 Gateway Time-out\r\n\r\n")?;
                        return Ok(());
                    } else {
                        reporter.send((id, Event::Retry()))?;
                    }
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
            remote.local_addr().unwrap().as_socket().unwrap().ip(),
            remote.peer_addr().unwrap().as_socket().unwrap().ip(),
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
        let up = thread::spawn(move || copy(id, Box::new(local_), Box::new(remote_), reporter_up));

        let reporter_down = reporter.clone();
        let down =
            thread::spawn(move || copy(id, Box::new(remote), Box::new(local), reporter_down));

        match up.join().and(down.join()).unwrap() {
            Ok(()) => reporter.send((id, Event::Done()))?,
            Err(e) => return Err(e),
        };
    }
    Ok(())
}
fn copy(
    id: usize,
    mut from: Box<dyn Read>,
    mut to: Box<dyn Write>,
    reporter: mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    #[allow(invalid_value)]
    let mut buffer = unsafe { std::mem::MaybeUninit::<[u8; BUFFER_SIZE]>::uninit().assume_init() };
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
