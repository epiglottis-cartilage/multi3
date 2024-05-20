use crate::config;

use std::net::SocketAddr;
use std::num::NonZeroU16;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpSocket, TcpStream};
use tokio::time::timeout;

pub async fn handle(mut local: TcpStream, config: &config::HandlerConfig) {
    let error_code = match handle_inner(&mut local, config).await {
        Ok(code) => code,
        Err(_e) => {
            // println!("Inner error: {e}");
            NonZeroU16::new(500)
        }
    };

    if let Some(code) = error_code {
        let _ = local
            .write_all(format!("HTTP/1.1 {} {}\r\n\r\n", code, "Fucked").as_bytes())
            .await;
    }
}

async fn handle_inner(
    local: &mut TcpStream,
    config: &config::HandlerConfig,
) -> std::io::Result<Option<NonZeroU16>> {
    let time_start = std::time::Instant::now();
    let is_tls;
    let uri = {
        // This block will get the URI
        // And also consume "CONNECT" from https

        #[allow(invalid_value)]
        let mut buffer = unsafe { std::mem::MaybeUninit::<[u8; 10240]>::uninit().assume_init() };

        let n = local.peek(&mut buffer).await?;
        let request = String::from_utf8_lossy(&buffer[..n]);
        let mut request_split = request.split_ascii_whitespace();

        is_tls = request_split
            .next()
            .is_some_and(|head| head.eq_ignore_ascii_case("CONNECT"));

        let uri = request_split
            .skip_while(|x| !x.eq_ignore_ascii_case("Host:"))
            .nth(1);

        let mut uri = match uri {
            Some(x) => x,
            None => {
                println!(
                    "Host not found from {}=>\n[{}]",
                    local.local_addr()?,
                    String::from_utf8_lossy(&buffer[..n])
                );
                return Ok(NonZeroU16::new(400));
            }
        }
        .to_owned();

        if (uri.starts_with('[') && uri.ends_with(']')) || (!uri.contains(':')) {
            uri += ":80";
        };

        if is_tls {
            let _ = local.read(&mut buffer).await?;
        }

        uri
    };

    let mut remote = None;

    {
        // This block will connect to URI

        let hosts = match lookup_host(&uri).await {
            Ok(x) => x,
            Err(e) => {
                println!("{uri} cannot resolve: {e}");
                return Ok(NonZeroU16::new(404));
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

        for host in hosts {
            let local_socket: SocketAddr;
            let builder;
            match host {
                SocketAddr::V4(_) => {
                    local_socket = (config.next_ip4(), 0).into();
                    builder = TcpSocket::new_v4()?;
                }
                SocketAddr::V6(_) => {
                    local_socket = (config.next_ip6(), 0).into();
                    builder = TcpSocket::new_v6()?;
                }
            };

            if builder.bind(local_socket).is_err() {
                println!("{} is not bindable", local_socket.ip());
                continue;
            }
            match timeout(config.connect_ttl, builder.connect(host)).await {
                Ok(res) => match res {
                    Ok(stream) => {
                        remote = Some(stream);
                        break;
                    }
                    Err(e) => {
                        println!("{uri}({host}): {e}");
                        continue;
                    }
                },
                Err(_) => {
                    println!("{uri}({host}): Timeout when connect");
                    return Ok(NonZeroU16::new(504));
                }
            }
        }
    }
    let mut remote = match remote {
        None => return Ok(NonZeroU16::new(500)),
        Some(remote) => {
            if is_tls {
                local.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await?;
            }
            remote
        }
    };
    println!(
        "{}<->{} established in {}ms",
        remote.local_addr()?.ip(),
        uri,
        time_start.elapsed().as_millis(),
    );
    let (_up, _down) = tokio::io::copy_bidirectional(local, &mut remote).await?;
    println!(
        "{}<->{} end in {}s.",
        remote.local_addr()?.ip(),
        uri,
        time_start.elapsed().as_secs(),
    );

    Ok(None)
}
