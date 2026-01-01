use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio_rustls::rustls::pki_types::ServerName;

use crate::config;
use crate::tls;
use crate::{Error, Result};

type Buffer = Box<[u8]>;
const SIZE: usize = 40960;

pub async fn handle(id: u64, local: TcpStream, config: &config::HandlerConfig) {
    if let Err(e) = handle_inner(id, local, config).await {
        eprintln!("[{id:^5}] {e}");
    }
}
pub async fn handle_inner(
    id: u64,
    mut local: TcpStream,
    config: &config::HandlerConfig,
) -> Result<()> {
    eprintln!("[{id:^5}] Recv from {}", local.peer_addr().unwrap());

    let mut buf = Vec::with_capacity(SIZE);
    unsafe {
        buf.set_len(SIZE);
    }
    let mut buf = buf.into_boxed_slice();

    let n = match local.read(&mut buf).await {
        Ok(0) => return Ok(()),
        Err(_) => return Ok(()),
        Ok(n) => n,
    };
    if n < 3 {
        return Err(Error::InvalidRequest(
            format!("Too short: {:?}", &buf[..n]).into(),
        ));
    }
    if buf[0] == 0x05 {
        socks_recv(id, local, config, (buf, n)).await
    } else if str::from_utf8(&buf[..n.min(16)]).is_ok() {
        if buf.starts_with(b"CONNECT") {
            let addr = http_addr(&buf, 443)?;
            https_resolved(id, local, addr, config, (buf, n)).await
        } else {
            let addr = http_addr(&buf, 80)?;
            http_resolved(id, local, addr, config, (buf, n)).await
        }
    } else {
        Err(Error::InvalidRequest(
            format!("Unknown protocol: {:?}", &buf[..n]).into(),
        ))
    }
}
async fn http_resolved(
    id: u64,
    mut local: TcpStream,
    addr: String,
    config: &config::HandlerConfig,
    (buf, n): (Buffer, usize),
) -> Result<()> {
    eprintln!("[{id:^5}] Http  {addr}");

    let hosts = match lookup_host(&addr, config).await {
        Ok(hosts) => hosts,
        Err(e) => {
            let _ = local.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n").await;
            return Err(e);
        }
    };

    match connect(id, None, None, hosts, config).await {
        Ok((mut remote, local_addr, peer_addr)) => {
            remote.write_all(&buf[..n]).await?;
            eprintln!("[{id:^5}] {} ↔︎ {}", local_addr, peer_addr);
            tcp_relay(id, tls::Stream::new_direct(local), remote).await?;
        }
        Err(e) => {
            let _ = local
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n")
                .await;
            return Err(e);
        }
    }
    Ok(())
}
async fn https_resolved(
    id: u64,
    mut local: TcpStream,
    addr: String,
    config: &config::HandlerConfig,
    (_buf, _n): (Buffer, usize),
) -> Result<()> {
    eprintln!("[{id:^5}] Https {addr}");

    let hosts = match lookup_host(&addr, config).await {
        Ok(hosts) => hosts,
        Err(e) => {
            let _ = local.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n").await;
            return Err(e);
        }
    };

    let host_name = &addr[..addr.find(':').unwrap()];
    let mapped_name = config
        .sni_map
        .iter()
        .filter_map(|(src, dst)| {
            if host_name.ends_with(src) {
                eprintln!("[{id:^5}] {addr} → {dst:?}");
                Some(dst)
            } else {
                None
            }
        })
        .next();
    match connect(id, Some(host_name), mapped_name, hosts, config).await {
        Ok((remote, local_addr, peer_addr)) => {
            local
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
            local.flush().await?;
            let local = if matches!(remote, tls::Stream::Tls(_)) {
                eprintln!("[{id:^5}] {} ⇹ {}", local_addr, peer_addr);
                tls::Stream::new_server(local, host_name).await?
            } else {
                eprintln!("[{id:^5}] {} ↔︎ {}", local_addr, peer_addr);
                tls::Stream::new_direct(local)
            };
            tcp_relay(id, local, remote).await?;
        }
        Err(e) => {
            let _ = local
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n")
                .await;
            return Err(e);
        }
    }
    Ok(())
}
async fn socks_recv(
    id: u64,
    mut local: TcpStream,
    config: &config::HandlerConfig,
    (buf, n): (Buffer, usize),
) -> Result<()> {
    eprintln!("[{id:^5}] Socks5");
    if !buf[2..n].contains(&0x00) {
        let _ = local.write_all(&[0x05, 0xff]).await;
        Err(Error::InvalidRequest(
            format!("Invalid authentication: {:?}", &buf[..n]).into(),
        ))
    } else {
        local.write_all(&[0x05, 0x00]).await?;
        local.flush().await?;
        socks_handle_request(id, local, config, (buf, n)).await?;
        Ok(())
    }
}
async fn socks_handle_request(
    id: u64,
    mut local: TcpStream,
    config: &config::HandlerConfig,
    (mut buf, _n): (Buffer, usize),
) -> Result<()> {
    let n = local.read(&mut buf).await?;
    let (cmd, addr, _) = socks_prase_request(&buf[..n]).unwrap();
    match cmd {
        1 => {
            socks_tcp_resolved(id, local, addr, config, (buf, n)).await?;
        }
        3 => {
            socks_udp_resolved(id, local, addr, config, (buf, n)).await?;
        }
        _ => {}
    }
    Ok(())
}
async fn socks_tcp_resolved(
    id: u64,
    mut local: TcpStream,
    addr: String,
    config: &config::HandlerConfig,
    (mut buf, n): (Buffer, usize),
) -> Result<()> {
    eprintln!("[{id:^5}] Tcp -> {addr}");

    let hosts = match lookup_host(&addr, config).await {
        Ok(hosts) => hosts,
        Err(e) => {
            buf[1] = 0x04;
            let _ = local.write_all(&buf[..n]).await;
            return Err(e);
        }
    };
    match connect(id, None, None, hosts, config).await {
        Ok((remote, local_addr, peer_addr)) => {
            let n = build_socks_response(0, local_addr, &mut buf);
            let _ = local.write_all(&buf[..n]).await;
            eprintln!("[{id:^5}] {} ↔︎ {}", local_addr, peer_addr);
            tcp_relay(id, tls::Stream::new_direct(local), remote).await?;
        }
        Err(e) => {
            buf[1] = 0x04;
            let _ = local.write_all(&buf[..n]).await;
            return Err(e);
        }
    }
    Ok(())
}
async fn socks_udp_resolved(
    id: u64,
    mut local: TcpStream,
    _addr: String,
    config: &config::HandlerConfig,
    (mut buf, _n): (Buffer, usize),
) -> Result<()> {
    let socket = if local.peer_addr().unwrap().is_ipv6() {
        tokio::net::UdpSocket::bind((config.next_v6(), 0)).await?
    } else {
        tokio::net::UdpSocket::bind((config.next_v4(), 0)).await?
    };
    let remote_bind = socket.local_addr().unwrap();
    eprintln!("[{id:^5}] Udp ← {}", remote_bind);

    let n = build_socks_response(0, remote_bind, &mut buf);
    local.write_all(&buf[..n]).await?;
    socks_udp_relay(id, local, socket, (buf, n)).await?;
    Ok(())
}
async fn socks_udp_relay(
    id: u64,
    ctl: TcpStream,
    socket: UdpSocket,
    (mut buf, _n): (Buffer, usize),
) -> Result<()> {
    let mut local: Option<SocketAddr> = None;
    let ctl = ctl.into_std().unwrap();
    loop {
        match ctl.peek(&mut [0]) {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            _ => break,
        }
        let (n, src) = socket.recv_from(&mut buf).await?;
        if local.is_none() {
            local = Some(src);
            eprintln!("[{id:^5}] {} ↔︎ ...", src);
        }
        let local = local.unwrap();
        if src == local {
            if n < 10 || buf[2] != 0 {
                continue;
            }
            let (addr, d) = socks_prase_host(&buf[3..n]).unwrap();
            socket.send_to(&buf[d..n], &addr).await?;
        } else {
            socket
                .send_to(&build_socks_udp(src, &buf[..n]), local)
                .await?;
        }
    }
    eprintln!("[{id:^5}] Done");
    Ok(())
}

async fn tcp_relay(id: u64, mut local: tls::Stream, mut remote: tls::Stream) -> Result<()> {
    tokio::io::copy_bidirectional_with_sizes(&mut local, &mut remote, SIZE, SIZE).await?;
    eprintln!("[{id:^5}] Done",);
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
fn http_addr(buffer: &[u8], default_port: u16) -> Result<String> {
    let request = String::from_utf8_lossy(buffer);
    let mut request_split = request.split_ascii_whitespace();
    let path = request_split.nth(1);
    let mut addr = request_split
        .skip_while(|x| !x.eq_ignore_ascii_case("Host:"))
        .nth(1)
        .or(path)
        .ok_or(Error::IoError(std::io::Error::new(
            std::io::ErrorKind::HostUnreachable,
            "DNS fails",
        )))?
        .to_owned();
    if (addr.starts_with('[') && addr.ends_with(']')) || (!addr.contains(':')) {
        addr.push(':');
        addr.push_str(&default_port.to_string());
    }
    Ok(addr)
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
async fn lookup_host(addr: &str, config: &config::HandlerConfig) -> Result<Vec<SocketAddr>> {
    let mut addrs = tokio::net::lookup_host(addr).await?.collect::<Vec<_>>();
    if let Some(ipv6_first) = config.ipv6_first {
        addrs.sort_by_key(|addr| {
            (
                !((config.has_ipv4 & addr.is_ipv4()) | (config.has_ipv6 & addr.is_ipv6())),
                addr.is_ipv6() == ipv6_first,
            )
        });
    }
    if addrs.is_empty() {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "DNS Empty result").into())
    } else {
        Ok(addrs)
    }
}
async fn connect(
    id: u64,
    host_name: Option<&str>,
    mapped_name: Option<&ServerName<'static>>,
    hosts: Vec<SocketAddr>,
    config: &config::HandlerConfig,
) -> Result<(tls::Stream, SocketAddr, SocketAddr)> {
    let get_builder = |host: &SocketAddr| {
        let builder;
        match host {
            SocketAddr::V4(_) => {
                builder = tokio::net::TcpSocket::new_v4()?;
                builder.bind((config.next_v4(), 0).into())?;
            }
            SocketAddr::V6(_) => {
                builder = tokio::net::TcpSocket::new_v6()?;
                builder.bind((config.next_v6(), 0).into())?;
            }
        }
        let local_addr = builder.local_addr().unwrap();
        Ok::<_, Error>((builder, local_addr, *host))
    };
    if let (Some(host_name), Some((mapped_name, boost))) = (
        host_name,
        match (mapped_name, config.boost) {
            (None, None) => None,
            (None, Some(boost)) => host_name.and_then(|host_name| {
                ServerName::try_from(host_name)
                    .ok()
                    .map(|mapped_name| (mapped_name, boost.get()))
            }),
            (Some(mapped_name), boost) => {
                Some((mapped_name.to_owned(), boost.map_or(1, |b| b.get())))
            }
        },
    ) {
        let mut futures = hosts
            .iter()
            .cycle()
            .take(boost as usize)
            .map(|host| {
                let (builder, local_addr, peer_addr) = get_builder(host).unwrap();
                let host = *host;
                let mapped = mapped_name.to_owned();
                (async move || {
                    let remote = builder.connect(host).await?;
                    let remote = tls::Stream::new_client(remote, mapped).await?;

                    Ok::<_, Error>((remote, local_addr, peer_addr))
                })()
            })
            .collect::<tokio::task::JoinSet<_>>();
        while let Some(f) = futures.join_next().await {
            match f {
                Ok(Ok(x)) => {
                    let start = std::time::Instant::now();
                    if cfg!(debug_assertions) {
                        tokio::spawn((async move || {
                            let _ = futures.join_all();
                            eprintln!("[{id:^5}] save {}ms", start.elapsed().as_millis_f32());
                        })());
                    }
                    return Ok(x);
                }
                Ok(Err(e)) => {
                    eprintln!("[{id:^5}] ⫤ {} fail {}", host_name, e);
                }
                Err(e) => {
                    unreachable!("Join Error {}", e);
                }
            }
        }
    } else {
        for host in hosts {
            let (builder, local_addr, peer_addr) = get_builder(&host)?;
            if let Ok(remote) = builder.connect(host).await {
                return Ok((tls::Stream::new_direct(remote), local_addr, peer_addr));
            }
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::HostUnreachable,
        "All hosts are unreachable",
    )
    .into())
}
