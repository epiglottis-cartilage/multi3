use std::hint::unreachable_unchecked;
use std::io::{ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::net::TcpStream;
use std::net::{Ipv4Addr, UdpSocket};
use std::net::{Ipv6Addr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::mpsc;

use rustls::pki_types::ServerName;

use crate::config;
use crate::event::{Event, Protocol};
use crate::tls::WarpedStream;
use crate::utils::{ReadHalf, WriteHalf, split};
use crate::{Error, Result};

type Buffer = Box<[u8]>;
const SIZE: usize = 40960;
pub fn handle(
    id: usize,
    mut local: TcpStream,
    cfg: &(&config::Config, Arc<config::IpPool>),
    reporter: &mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    // eprintln!("[{id}] Recv from {}", local.peer_addr().unwrap());
    local.set_read_timeout(Some(cfg.0.io_timeout)).unwrap();
    let mut buf = Vec::with_capacity(SIZE);
    unsafe {
        buf.set_len(SIZE);
    }
    let mut buf = buf.into_boxed_slice();

    if let Ok(n @ 1..) = local.read(&mut buf) {
        reporter.send((id, Event::Received(local.peer_addr().unwrap().ip())))?;
        if n < 3 {
            // eprintln!("[{id}] Too short: {:?}", &buf[..n]);
            reporter.send((
                id,
                Event::Error(Error::InvalidRequest(
                    format!("Too short: {:?}", &buf[..n]).into(),
                )),
            ))?;
            return Ok(());
        }
        if let Err(e) = if buf[0] == 0x05 {
            socks_recv(id, local, cfg, (buf, n), reporter)
        } else if str::from_utf8(&buf[..n.min(16)]).is_ok() {
            if buf.starts_with(b"CONNECT") {
                let addr = http_addr(&buf, 443).expect("No addr found");
                https_resolved(id, local, addr, cfg, (buf, n), reporter)
            } else {
                let addr = http_addr(&buf, 80).expect("No addr found");
                http_resolved(id, local, addr, cfg, (buf, n), reporter)
            }
        } else {
            // eprintln!("[{id}] Unknown protocol: {:?}", &buf[..n]);
            reporter.send((
                id,
                Event::Error(Error::InvalidRequest(
                    format!("Unknown protocol: {:?}", &buf[..n]).into(),
                )),
            ))?;
            return Ok(());
        } {
            // eprintln!("[{id}] Inner error: {e}");
            let _ = reporter.send((id, Event::Error(e.into())));
        }
    }
    Ok(())
}
fn http_resolved(
    id: usize,
    mut local: TcpStream,
    addr: String,
    cfg: &(&config::Config, Arc<config::IpPool>),
    (buf, n): (Buffer, usize),
    reporter: &mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    // eprintln!("[{id}] Http  {addr}");
    reporter.send((id, Event::Recognized(Protocol::Http)))?;
    reporter.send((id, Event::Resolved(addr.clone())))?;

    let hosts = match lookup_host(&addr, cfg) {
        Ok(hosts) => hosts,
        Err(e) => {
            let _ = local.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n");
            // eprintln!("[{id}] DNS fails: {addr} {e}");
            reporter.send((id, Event::Error(e.into())))?;
            return Ok(());
        }
    };

    match connect(None, hosts, cfg, || {
        reporter.send((id, Event::Retry())).map_err(Into::into)
    }) {
        Ok(mut remote) => {
            remote.write_all(&buf[..n])?;
            reporter.send((
                id,
                Event::Connected(remote.local_addr().ip(), remote.peer_addr()),
            ))?;
            let (lr, lw) = split(WarpedStream::new(local));
            let (rr, rw) = split(remote);
            tcp_relay(id, lr, lw, rr, rw, buf.clone(), buf, reporter)?;
        }
        Err(e) => {
            let _ = local.write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n");
            // eprintln!("[{id}] Failed to connect {addr}: {e}");
            reporter.send((id, Event::Error(e.into())))?;
        }
    }
    Ok(())
}
fn https_resolved(
    id: usize,
    mut local: TcpStream,
    addr: String,
    cfg: &(&config::Config, Arc<config::IpPool>),
    (buf, _n): (Buffer, usize),
    reporter: &mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    // eprintln!("[{id}] Https {addr}");
    reporter.send((id, Event::Recognized(Protocol::Https)))?;
    reporter.send((id, Event::Resolved(addr.clone())))?;

    let hosts = match lookup_host(&addr, cfg) {
        Ok(hosts) => hosts,
        Err(e) => {
            let _ = local.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n");
            // eprintln!("[{id}] DNS fails: {addr} {e}");
            reporter.send((id, Event::Error(e.into())))?;
            return Ok(());
        }
    };
    let host_name = &addr[..*addr.find(':').iter().last().unwrap()];
    match connect(Some(host_name), hosts, cfg, || {
        reporter.send((id, Event::Retry())).map_err(Into::into)
    }) {
        Ok(remote) => {
            local.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
            local.flush()?;
            let local = WarpedStream::new_server(local)?;

            reporter.send((
                id,
                Event::Connected(remote.local_addr().ip(), remote.peer_addr()),
            ))?;
            let (lr, lw) = split(local);
            let (rr, rw) = split(remote);
            tcp_relay(id, lr, lw, rr, rw, buf.clone(), buf, reporter)?;
        }
        Err(e) => {
            let _ = local.write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n");
            // eprintln!("[{id}] Failed to connect {addr}: {e}");
            reporter.send((id, Event::Error(e.into())))?;
        }
    }
    Ok(())
}
fn socks_recv(
    id: usize,
    mut local: TcpStream,
    cfg: &(&config::Config, Arc<config::IpPool>),
    (buf, n): (Buffer, usize),
    reporter: &mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    // eprintln!("[{id}] Socks5");

    if !buf[2..n].contains(&0x00) {
        let _ = local.write_all(&[0x05, 0xff]);
        // eprintln!("[{id}] Invalid Socks5 authentication {:?}", &buf[..n]);
        reporter.send((
            id,
            Event::Error(Error::InvalidRequest(
                format!("Invalid authentication: {:?}", &buf[..n]).into(),
            )),
        ))?;
    } else {
        local.write_all(&[0x05, 0x00])?;
        local.flush()?;
        socks_handle_request(id, local, cfg, (buf, n), reporter)?;
    }
    Ok(())
}
fn socks_handle_request(
    id: usize,
    mut local: TcpStream,
    cfg: &(&config::Config, Arc<config::IpPool>),
    (mut buf, _n): (Buffer, usize),
    reporter: &mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    let n = local.read(&mut buf)?;
    let (cmd, addr, _) = socks_prase_request(&buf[..n])
        .ok_or_else(|| Error::InvalidRequest("No host in request".into()))?;
    match cmd {
        1 => {
            socks_tcp_resolved(id, local, addr, cfg, (buf, n), reporter)?;
        }
        3 => {
            socks_udp_resolved(id, local, addr, cfg, (buf, n), reporter)?;
        }
        _ => {}
    }
    Ok(())
}
fn socks_tcp_resolved(
    id: usize,
    mut local: TcpStream,
    addr: String,
    cfg: &(&config::Config, Arc<config::IpPool>),
    (mut buf, n): (Buffer, usize),
    reporter: &mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    // eprintln!("[{id}] Tcp -> {addr}");
    reporter.send((id, Event::Recognized(Protocol::Socks5Tcp)))?;
    reporter.send((id, Event::Resolved(addr.clone())))?;

    let hosts = match lookup_host(&addr, cfg) {
        Ok(hosts) => hosts,
        Err(e) => {
            buf[1] = 0x04;
            let _ = local.write_all(&buf[..n]);
            // eprintln!("[{id}] DNS fails: {addr} {e}");
            reporter.send((id, Event::Error(e.into())))?;
            return Ok(());
        }
    };
    match connect(None, hosts, cfg, || {
        reporter.send((id, Event::Retry())).map_err(Into::into)
    }) {
        Ok(remote) => {
            let n = build_socks_response(0, remote.local_addr(), &mut buf);
            let _ = local.write_all(&buf[..n]);
            let (lr, lw) = split(WarpedStream::new(local));
            let (rr, rw) = split(remote);
            tcp_relay(id, lr, lw, rr, rw, buf.clone(), buf, reporter)?;
        }
        Err(e) => {
            buf[1] = 0x04;
            let _ = local.write_all(&buf[..n]);
            // eprintln!("[{id}] Failed to connect {addr}: {e}");
            reporter.send((id, Event::Error(e.into())))?;
        }
    }
    Ok(())
}
fn socks_udp_resolved(
    id: usize,
    mut local: TcpStream,
    _addr: String,
    cfg: &(&config::Config, Arc<config::IpPool>),
    (mut buf, _n): (Buffer, usize),
    reporter: &mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    let socket = if local.peer_addr().unwrap().is_ipv6() {
        UdpSocket::bind((cfg.1.next_v6(), 0))?
    } else {
        UdpSocket::bind((cfg.1.next_v4(), 0))?
    };
    let remote_bind = socket.local_addr()?;
    // eprintln!("[{id}] Udp <- {}", remote_bind);
    reporter.send((id, Event::Recognized(Protocol::Socks5Udp)))?;

    let n = build_socks_response(0, remote_bind, &mut buf);
    local.write_all(&buf[..n])?;
    socks_udp_relay(id, local, socket, (buf, n), reporter)?;
    Ok(())
}
fn socks_udp_relay(
    id: usize,
    ctl: TcpStream,
    socket: UdpSocket,
    (mut buf, _n): (Buffer, usize),
    reporter: &mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    let mut local: Option<SocketAddr> = None;
    ctl.set_nonblocking(true)?;
    // socket.set_read_timeout()
    loop {
        match ctl.peek(&mut [0]) {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            _ => break,
        }
        let (n, src) = socket.recv_from(&mut buf)?;
        let local = match local {
            Some(x) => x,
            ref mut l @ None => {
                *l = Some(src);
                src
            }
        };
        if src == local {
            if n < 10 || buf[2] != 0 {
                continue;
            }
            let (addr, d) = socks_prase_host(&buf[3..n])
                .ok_or_else(|| Error::InvalidRequest("No host in request".into()))?;
            socket.send_to(&buf[d..n], &addr)?;
        } else {
            socket.send_to(&build_socks_udp(src, &buf[..n]), local)?;
        }
    }
    // eprintln!("[{id}] Done");
    reporter.send((id, Event::Done()))?;
    Ok(())
}

fn tcp_relay(
    id: usize,
    local_r: ReadHalf,
    local_w: WriteHalf,
    remote_r: ReadHalf,
    remote_w: WriteHalf,
    buf_up: Buffer,
    buf_down: Buffer,
    reporter: &mpsc::Sender<(usize, Event)>,
) -> Result<()> {
    // eprintln!(
    //     "[{id}] {} <-> {}",
    //     remote.local_addr().unwrap(),
    //     remote.peer_addr().unwrap()
    // );

    let reporter_ = reporter.clone();
    let handle = std::thread::spawn(move || {
        copy(local_r, remote_w, buf_up, |x| {
            reporter_.send((id, Event::Upload(x))).map_err(Into::into)
        })
    });

    copy(remote_r, local_w, buf_down, move |x| {
        reporter.send((id, Event::Download(x))).map_err(Into::into)
    })?;
    handle.join().unwrap()?;

    eprintln!("[{id}] Done",);
    reporter.send((id, Event::Done()))?;
    Ok(())
}
fn copy(
    mut from: ReadHalf,
    mut to: WriteHalf,
    mut buf: Buffer,
    reporter: impl Fn(usize) -> Result<()>,
) -> Result<()> {
    loop {
        match from.inner().read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                to.inner().write_all(&buf[..n])?;
                reporter(n)?;
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            e => {
                e?;
                unsafe { unreachable_unchecked() };
            }
        }
    }
    // let _ = to.shutdown(std::net::Shutdown::Both);
    Ok(())
}
fn build_socks_response(cmd: u8, addr: SocketAddr, buf: &mut Buffer) -> usize {
    buf[0] = 0x05;
    buf[1] = cmd;
    buf[2] = 0x00;
    match addr {
        SocketAddr::V4(addr) => {
            buf[3] = 0x01;
            buf[4..8].copy_from_slice(&addr.ip().octets());
            buf[8..10].copy_from_slice(&addr.port().to_be_bytes());
            10
        }
        SocketAddr::V6(addr) => {
            buf[3] = 0x04;
            buf[4..20].copy_from_slice(&addr.ip().octets());
            buf[20..22].copy_from_slice(&addr.port().to_be_bytes());
            22
        }
    }
}
fn build_socks_udp(addr: SocketAddr, data: &[u8]) -> Box<[u8]> {
    let mut pack = Vec::with_capacity(data.len() + if addr.is_ipv6() { 22 } else { 10 });
    unsafe { pack.set_len(pack.capacity()) };
    let mut pack = pack.into_boxed_slice();
    pack[0] = 0x00;
    pack[1] = 0x00;
    pack[2] = 0x00;
    match addr {
        SocketAddr::V4(addr) => {
            pack[3] = 0x01;
            pack[4..8].copy_from_slice(&addr.ip().octets());
            pack[8..10].copy_from_slice(&addr.port().to_be_bytes());
            pack[10..].copy_from_slice(data);
        }
        SocketAddr::V6(addr) => {
            pack[3] = 0x04;
            pack[4..20].copy_from_slice(&addr.ip().octets());
            pack[20..22].copy_from_slice(&addr.port().to_be_bytes());
            pack[22..].copy_from_slice(data);
        }
    }
    pack
}
fn http_addr(buffer: &[u8], default_port: u16) -> Option<String> {
    let request = String::from_utf8_lossy(buffer);
    let mut request_split = request.split_ascii_whitespace();
    let path = request_split.nth(1);
    let mut addr = request_split
        .skip_while(|x| !x.eq_ignore_ascii_case("Host:"))
        .nth(1)
        .or(path)?
        .to_owned();
    if (addr.starts_with('[') && addr.ends_with(']')) || (!addr.contains(':')) {
        addr += &format!(":{default_port}");
    }
    Some(addr)
}
fn socks_prase_request(buffer: &[u8]) -> Option<(u8, String, usize)> {
    let cmd = buffer[1];
    socks_prase_host(&buffer[3..]).map(|(x, y)| (cmd, x, y))
}
fn socks_prase_host(buffer: &[u8]) -> Option<(String, usize)> {
    match buffer[0] {
        1 => {
            let addr = Ipv4Addr::from_octets(*buffer[1..5].first_chunk()?);
            let port = u16::from_be_bytes(*buffer[5..7].first_chunk()?);
            Some((format!("{}:{}", addr, port), 7))
        }
        4 => {
            let addr = Ipv6Addr::from_octets(*buffer[1..17].first_chunk()?);
            let port = u16::from_be_bytes(*buffer[17..19].first_chunk()?);
            Some((format!("{}:{}", addr, port), 19))
        }
        3 => {
            let len = buffer[1] as usize;
            let addr = String::from_utf8(buffer[2..2 + len].to_vec()).ok()?;
            let port = u16::from_be_bytes(*buffer[2 + len..2 + len + 2].first_chunk()?);
            Some((format!("{}:{}", addr, port), 2 + len + 2))
        }
        _ => None,
    }
}
fn lookup_host(
    addr: &str,
    cfg: &(&config::Config, Arc<config::IpPool>),
) -> std::io::Result<Vec<SocketAddr>> {
    let mut addrs = addr.to_socket_addrs()?.collect::<Vec<_>>();
    if let Some(ipv6_first) = cfg.0.ipv6_first {
        addrs.sort_by_key(|addr| {
            (
                !((cfg.1.have_v4 & addr.is_ipv4()) | (cfg.1.have_v6 & addr.is_ipv6())),
                addr.is_ipv6() == ipv6_first,
            )
        });
    }
    if addrs.is_empty() {
        // TODO: error
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Empty result",
        ))
    } else {
        Ok(addrs)
    }
}

fn connect(
    host_name: Option<&str>,
    hosts: Vec<SocketAddr>,
    cfg: &(&config::Config, Arc<config::IpPool>),
    reporter: impl Fn() -> Result<()>,
) -> Result<WarpedStream> {
    use socket2::{Domain, Protocol, Socket, Type};

    if let Some(host_name) = host_name
        && cfg.0.boost > 1
    {
        let (rx, wx) = std::sync::mpsc::channel();
        let connect_timeout = cfg.0.connect_timeout;
        let io_timeout = cfg.0.io_timeout;
        hosts
            .iter()
            .cycle()
            .take(cfg.0.boost as usize)
            .for_each(|host| {
                let rx = rx.clone();
                let host_name = host_name.to_string();
                let builder;
                match host {
                    SocketAddr::V4(_) => {
                        builder =
                            Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
                        builder
                            .bind(&SocketAddr::new(cfg.1.next_v4().into(), 0).into())
                            .unwrap();
                    }
                    SocketAddr::V6(_) => {
                        builder =
                            Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP)).unwrap();
                        builder
                            .bind(&SocketAddr::new(cfg.1.next_v6().into(), 0).into())
                            .unwrap();
                    }
                }
                let host = (*host).into();
                std::thread::spawn(move || {
                    if let Ok(()) = builder.connect_timeout(&host, connect_timeout) {
                        let remote: TcpStream = builder.into();
                        remote.set_read_timeout(Some(io_timeout)).unwrap();
                        let server_name = ServerName::try_from(host_name).unwrap();
                        if let Ok(x) = WarpedStream::new_client(remote, server_name.to_owned()) {
                            rx.send(x).unwrap();
                        }
                    }
                });
            });
        if let Ok(remote) = wx.recv_timeout(connect_timeout + io_timeout) {
            return Ok(remote);
        }
    } else {
        for host in hosts {
            let builder;
            match host {
                SocketAddr::V4(_) => {
                    builder = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
                    builder.bind(&SocketAddr::new(cfg.1.next_v4().into(), 0).into())?;
                }
                SocketAddr::V6(_) => {
                    builder = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
                    builder.bind(&SocketAddr::new(cfg.1.next_v6().into(), 0).into())?;
                }
            }
            if let Ok(()) = builder.connect_timeout(&host.into(), cfg.0.connect_timeout) {
                let remote: TcpStream = builder.into();
                remote.set_read_timeout(Some(cfg.0.io_timeout))?;
                return Ok(WarpedStream::new(remote));
            } else {
                reporter()?;
            }
        }
    };
    Err(std::io::Error::new(
        std::io::ErrorKind::HostUnreachable,
        "All hosts are unreachable",
    )
    .into())
}
