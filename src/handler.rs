use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio_rustls::rustls::pki_types::ServerName;

use crate::config;
use crate::tls;
use crate::tracker::{Protocol, Tracker};
use crate::{Error, Result};

type Buffer = Box<[u8]>;
const SIZE: usize = 40960;

struct ByteCounter<S> {
    inner: S,
    read_count: Arc<AtomicU64>,
    write_count: Arc<AtomicU64>,
}

impl<S: AsyncRead + Unpin> AsyncRead for ByteCounter<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        if matches!(result, Poll::Ready(Ok(()))) {
            let after = buf.filled().len();
            let read = after - before;
            this.read_count.fetch_add(read as u64, Ordering::Relaxed);
        }
        result
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ByteCounter<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &result {
            this.write_count.fetch_add(*n as u64, Ordering::Relaxed);
        }
        result
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

pub async fn handle(id: u64, local: TcpStream, config: &config::HandlerConfig, tracker: Tracker) {
    let peer_addr = match local.peer_addr() {
        Ok(addr) => addr,
        Err(_) => return,
    };
    log::info!("[{id:^5}] Recv from {}", peer_addr);
    tracker.add(id, String::new(), String::new(), Protocol::Http);

    match handle_inner(id, local, config, &tracker).await {
        Ok(()) => {
            tracker.set_completed(id, None);
        }
        Err(e) => {
            log::error!("[{id:^5}] {e}");
            tracker.set_completed(id, Some(e.to_string()));
        }
    }
}

pub async fn handle_inner(
    id: u64,
    mut local: TcpStream,
    config: &config::HandlerConfig,
    tracker: &Tracker,
) -> Result<()> {
    let local_addr = local.peer_addr().unwrap();
    log::info!("[{id:^5}] Recv from {}", local_addr);

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
        socks_recv(id, local, config, (buf, n), tracker).await
    } else if str::from_utf8(&buf[..n.min(16)]).is_ok() {
        if buf.starts_with(b"CONNECT") {
            let addr = http_addr(&buf, 443)?;
            tracker.update_info(id, addr.clone(), Protocol::Https);
            https_resolved(id, local, addr, config, (buf, n), tracker).await
        } else {
            let addr = http_addr(&buf, 80)?;
            tracker.update_info(id, addr.clone(), Protocol::Http);
            http_resolved(id, local, addr, config, (buf, n), tracker).await
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
    tracker: &Tracker,
) -> Result<()> {
    log::info!("[{id:^5}] Http  {addr}");

    let hosts = match lookup_host(&addr, config).await {
        Ok(hosts) => hosts,
        Err(e) => {
            let _ = local.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n").await;
            return Err(e);
        }
    };

    match connect(id, None, None, hosts, config, tracker).await {
        Ok((mut remote, local_addr, peer_addr)) => {
            remote.write_all(&buf[..n]).await?;
            tracker.set_connected(id);
            log::info!("[{id:^5}] {} \u{2194}\u{fe0e} {}", local_addr, peer_addr);
            tcp_relay(id, tls::Stream::new_direct(local), remote, tracker).await?;
            Ok(())
        }
        Err(e) => {
            let _ = local
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n")
                .await;
            Err(e)
        }
    }
}

async fn https_resolved(
    id: u64,
    mut local: TcpStream,
    addr: String,
    config: &config::HandlerConfig,
    (_buf, _n): (Buffer, usize),
    tracker: &Tracker,
) -> Result<()> {
    log::info!("[{id:^5}] Https {addr}");

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
                log::info!("[{id:^5}] {addr} \u{2192} {dst:?}");
                Some(dst)
            } else {
                None
            }
        })
        .next();
    match connect(id, Some(host_name), mapped_name, hosts, config, tracker).await {
        Ok((remote, local_addr, peer_addr)) => {
            local
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
            local.flush().await?;
            tracker.set_connected(id);
            let local = if matches!(remote, tls::Stream::Tls(_)) {
                log::info!("[{id:^5}] {} \u{21f9} {}", local_addr, peer_addr);
                tls::Stream::new_server(local, host_name).await?
            } else {
                log::info!("[{id:^5}] {} \u{2194}\u{fe0e} {}", local_addr, peer_addr);
                tls::Stream::new_direct(local)
            };
            tcp_relay(id, local, remote, tracker).await?;
            Ok(())
        }
        Err(e) => {
            let _ = local
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n")
                .await;
            Err(e)
        }
    }
}

async fn socks_recv(
    id: u64,
    mut local: TcpStream,
    config: &config::HandlerConfig,
    (buf, n): (Buffer, usize),
    tracker: &Tracker,
) -> Result<()> {
    log::info!("[{id:^5}] Socks5");
    if !buf[2..n].contains(&0x00) {
        let _ = local.write_all(&[0x05, 0xff]).await;
        Err(Error::InvalidRequest(
            format!("Invalid authentication: {:?}", &buf[..n]).into(),
        ))
    } else {
        local.write_all(&[0x05, 0x00]).await?;
        local.flush().await?;
        socks_handle_request(id, local, config, (buf, n), tracker).await
    }
}

async fn socks_handle_request(
    id: u64,
    mut local: TcpStream,
    config: &config::HandlerConfig,
    (mut buf, _n): (Buffer, usize),
    tracker: &Tracker,
) -> Result<()> {
    let n = local.read(&mut buf).await?;
    let (cmd, addr, _) = socks_prase_request(&buf[..n]).unwrap();
    match cmd {
        1 => {
            tracker.update_info(id, addr.clone(), Protocol::Socks5Tcp);
            socks_tcp_resolved(id, local, addr, config, (buf, n), tracker).await
        }
        3 => {
            tracker.update_info(id, addr.clone(), Protocol::Socks5Udp);
            socks_udp_resolved(id, local, addr, config, (buf, n), tracker).await
        }
        _ => Ok(()),
    }
}

async fn socks_tcp_resolved(
    id: u64,
    mut local: TcpStream,
    addr: String,
    config: &config::HandlerConfig,
    (mut buf, n): (Buffer, usize),
    tracker: &Tracker,
) -> Result<()> {
    log::info!("[{id:^5}] Tcp -> {addr}");

    let hosts = match lookup_host(&addr, config).await {
        Ok(hosts) => hosts,
        Err(e) => {
            buf[1] = 0x04;
            let _ = local.write_all(&buf[..n]).await;
            return Err(e);
        }
    };
    match connect(id, None, None, hosts, config, tracker).await {
        Ok((remote, local_addr, peer_addr)) => {
            let n = build_socks_response(0, local_addr, &mut buf);
            let _ = local.write_all(&buf[..n]).await;
            tracker.set_connected(id);
            log::info!("[{id:^5}] {} \u{2194}\u{fe0e} {}", local_addr, peer_addr);
            tcp_relay(id, tls::Stream::new_direct(local), remote, tracker).await?;
            Ok(())
        }
        Err(e) => {
            buf[1] = 0x04;
            let _ = local.write_all(&buf[..n]).await;
            Err(e)
        }
    }
}

async fn socks_udp_resolved(
    id: u64,
    mut local: TcpStream,
    _addr: String,
    config: &config::HandlerConfig,
    (mut buf, _n): (Buffer, usize),
    tracker: &Tracker,
) -> Result<()> {
    let socket = if local.peer_addr().unwrap().is_ipv6() {
        tokio::net::UdpSocket::bind((config.next_v6(), 0)).await?
    } else {
        tokio::net::UdpSocket::bind((config.next_v4(), 0)).await?
    };
    let remote_bind = socket.local_addr().unwrap();
    tracker.update_local_addr(id, remote_bind.ip().to_string());
    log::info!("[{id:^5}] Udp \u{2190} {}", remote_bind);

    let n = build_socks_response(0, remote_bind, &mut buf);
    local.write_all(&buf[..n]).await?;
    socks_udp_relay(id, local, socket, (buf, n), tracker).await
}

async fn socks_udp_relay(
    id: u64,
    ctl: TcpStream,
    socket: UdpSocket,
    (mut buf, _n): (Buffer, usize),
    tracker: &Tracker,
) -> Result<()> {
    let mut local: Option<SocketAddr> = None;
    let ctl = ctl.into_std().unwrap();
    tracker.set_connected(id);

    let conns = tracker.get_connections();
    let conn = conns.iter().find(|c| c.id == id);
    let (upload_atomic, download_atomic) = if let Some(conn) = conn {
        (conn.upload_bytes.clone(), conn.download_bytes.clone())
    } else {
        (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)))
    };

    loop {
        match ctl.peek(&mut [0]) {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            _ => break,
        }
        let (n, src) = socket.recv_from(&mut buf).await?;
        if local.is_none() {
            local = Some(src);
            log::info!("[{id:^5}] {} \u{2194}\u{fe0e} ...", src);
        }
        let local = local.unwrap();
        if src == local {
            if n < 10 || buf[2] != 0 {
                continue;
            }
            let (addr, d) = socks_prase_host(&buf[3..n]).unwrap();
            let sent = socket.send_to(&buf[d..n], &addr).await?;
            upload_atomic.fetch_add(sent as u64, Ordering::Relaxed);
        } else {
            let sent = socket
                .send_to(&build_socks_udp(src, &buf[..n]), local)
                .await?;
            download_atomic.fetch_add(sent as u64, Ordering::Relaxed);
        }
    }
    log::info!("[{id:^5}] Done");
    Ok(())
}

async fn tcp_relay(
    id: u64,
    local: tls::Stream,
    remote: tls::Stream,
    tracker: &Tracker,
) -> Result<()> {
    let conns = tracker.get_connections();
    let conn = conns.iter().find(|c| c.id == id);
    let (upload, download) = if let Some(conn) = conn {
        (conn.upload_bytes.clone(), conn.download_bytes.clone())
    } else {
        (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)))
    };

    let mut remote = ByteCounter {
        inner: remote,
        read_count: download.clone(),
        write_count: upload.clone(),
    };
    let mut local = local;

    tokio::io::copy_bidirectional_with_sizes(&mut local, &mut remote, SIZE, SIZE).await?;
    log::info!("[{id:^5}] Done");
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
    tracker: &Tracker,
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
        Ok::<_, Error>((builder, *host))
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
                let (builder, peer_addr) = get_builder(host).unwrap();
                let host = *host;
                let mapped = mapped_name.to_owned();
                (async move || {
                    let remote = builder.connect(host).await?;
                    let local_addr = remote.local_addr().unwrap();
                    let remote = tls::Stream::new_client(remote, mapped).await?;

                    Ok::<_, Error>((remote, local_addr, peer_addr))
                })()
            })
            .collect::<tokio::task::JoinSet<_>>();
        while let Some(f) = futures.join_next().await {
            match f {
                Ok(Ok((remote, local_addr, peer_addr))) => {
                    let start = std::time::Instant::now();
                    if cfg!(debug_assertions) {
                        tokio::spawn((async move || {
                            let _ = futures.join_all();
                            log::debug!("[{id:^5}] save {}ms", start.elapsed().as_millis_f32());
                        })());
                    }
                    tracker.update_local_addr(id, local_addr.ip().to_string());
                    return Ok((remote, local_addr, peer_addr));
                }
                Ok(Err(e)) => {
                    tracker.add_retry(id, format!("{} fail {}", host_name, e));
                    log::warn!("[{id:^5}] \u{2ae4} {} fail {}", host_name, e);
                }
                Err(e) => {
                    unreachable!("Join Error {}", e);
                }
            }
        }
    } else {
        for host in hosts {
            let (builder, peer_addr) = get_builder(&host)?;
            if let Ok(remote) = builder.connect(host).await {
                let local_addr = remote.local_addr().unwrap();
                tracker.update_local_addr(id, local_addr.ip().to_string());
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
