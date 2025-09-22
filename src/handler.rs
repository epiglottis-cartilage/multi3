use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::UdpSocket;

use crate::Result;
use crate::config;

type Buffer = Box<[u8]>;
const SIZE: usize = 40960;
pub async fn handle(id: u64, mut local: TcpStream, config: &config::HandlerConfig) {
    eprintln!("[{id}] Recv from {}", local.peer_addr().unwrap());

    let mut buf = Vec::with_capacity(SIZE);
    unsafe {
        buf.set_len(SIZE);
    }
    let mut buf = buf.into_boxed_slice();

    let n = match local.read(&mut buf).await {
        Ok(0) => return,
        Err(_) => return,
        Ok(n) => n,
    };
    if n < 3 {
        eprintln!("[{id}] Too short: {:?}", &buf[..n]);
        return;
    }
    if let Err(e) = if buf[0] == 0x05 {
        socks_recv(id, local, config, (buf, n)).await
    } else if str::from_utf8(&buf[..n.min(16)]).is_ok() {
        let addr = http_addr(&buf).expect("No addr found");
        if buf.starts_with(b"CONNECT") {
            https_resolved(id, local, addr, config, (buf, n)).await
        } else {
            http_resolved(id, local, addr, config, (buf, n)).await
        }
    } else {
        eprintln!("[{id}] Unknown protocol: {:?}", &buf);
        return;
    } {
        eprintln!("[{id}] Inner error: {e}");
    }
}
async fn http_resolved(
    id: u64,
    mut local: TcpStream,
    addr: String,
    config: &config::HandlerConfig,
    (buf, n): (Buffer, usize),
) -> Result<()> {
    eprintln!("[{id}] Http  {addr}");

    let hosts = match lookup_host(&addr, config).await {
        Ok(hosts) => hosts,
        Err(e) => {
            let _ = local.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n").await;
            eprintln!("[{id}] DNS fails: {addr} {e}");
            return Ok(());
        }
    };

    match connect(hosts, config).await {
        Ok(mut remote) => {
            remote.write_all(&buf[..n]).await?;
            tcp_relay(id, local, remote).await?;
        }
        Err(e) => {
            let _ = local
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n")
                .await;
            eprintln!("[{id}] Failed to connect {addr}: {e}");
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
    eprintln!("[{id}] Https {addr}");

    let hosts = match lookup_host(&addr, config).await {
        Ok(hosts) => hosts,
        Err(e) => {
            let _ = local.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n").await;
            eprintln!("[{id}] DNS fails: {addr} {e}");
            return Ok(());
        }
    };

    match connect(hosts, config).await {
        Ok(remote) => {
            local
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
            local.flush().await?;
            tcp_relay(id, local, remote).await?;
        }
        Err(e) => {
            let _ = local
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n")
                .await;
            eprintln!("[{id}] Failed to connect {addr}: {e}");
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
    eprintln!("[{id}] Socks5");
    if !buf[2..n].contains(&0x00) {
        let _ = local.write_all(&[0x05, 0xff]).await;
        eprintln!("[{id}] Invalid Socks5 authentication {:?}", &buf[..n]);
    } else {
        local.write_all(&[0x05, 0x00]).await?;
        local.flush().await?;
        socks_handle_request(id, local, config, (buf, n)).await?;
    }
    Ok(())
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
    eprintln!("[{id}] Tcp -> {addr}");

    let hosts = match lookup_host(&addr, config).await {
        Ok(hosts) => hosts,
        Err(e) => {
            buf[1] = 0x04;
            let _ = local.write_all(&buf[..n]).await;
            eprintln!("[{id}] DNS fails: {addr} {e}");
            return Ok(());
        }
    };
    match connect(hosts, config).await {
        Ok(remote) => {
            let n = build_socks_response(0, remote.local_addr().unwrap(), &mut buf);
            let _ = local.write_all(&buf[..n]).await;
            tcp_relay(id, local, remote).await?;
        }
        Err(e) => {
            buf[1] = 0x04;
            let _ = local.write_all(&buf[..n]).await;
            eprintln!("[{id}] Failed to connect {addr}: {e}");
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
    eprintln!("[{id}] Udp <- {}", remote_bind);

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
            eprintln!("[{id}] {} <-> ...", src);
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
    eprintln!("[{id}] Done");
    Ok(())
}

async fn tcp_relay(id: u64, mut local: TcpStream, mut remote: TcpStream) -> Result<()> {
    eprintln!(
        "[{id}] {} <-> {}",
        remote.local_addr().unwrap(),
        remote.peer_addr().unwrap()
    );
    let _ = tokio::io::copy_bidirectional_with_sizes(&mut local, &mut remote, SIZE, SIZE).await;
    eprintln!("[{id}] Done",);
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
fn http_addr(buffer: &[u8]) -> Option<String> {
    let request = String::from_utf8_lossy(buffer);
    let mut request_split = request.split_ascii_whitespace();
    let path = request_split.nth(1);
    let mut addr = request_split
        .skip_while(|x| !x.eq_ignore_ascii_case("Host:"))
        .nth(1)
        .or(path)?
        .to_owned();
    if (addr.starts_with('[') && addr.ends_with(']')) || (!addr.contains(':')) {
        addr += ":80";
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
async fn lookup_host(
    addr: &str,
    config: &config::HandlerConfig,
) -> std::io::Result<Vec<SocketAddr>> {
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
        // TODO: error
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Empty result",
        ))
    } else {
        Ok(addrs)
    }
}
async fn connect(
    hosts: Vec<SocketAddr>,
    config: &config::HandlerConfig,
) -> std::io::Result<tokio::net::TcpStream> {
    for host in hosts {
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
        if let Ok(remote) = builder.connect(host).await {
            return Ok(remote);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::HostUnreachable,
        "All hosts are unreachable",
    ))
}
