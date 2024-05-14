use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Mutex,
    time::Duration,
};

use anyhow::Context;

pub struct HostConfig {
    pub host: SocketAddr,
    pub tui: bool,
}
#[derive(Debug)]
pub struct HandlerConfig {
    ip4: Mutex<IpPool<Ipv4Addr>>,
    ip6: Mutex<IpPool<Ipv6Addr>>,
    pub connect_ttl: Duration,
    pub retry_ttl: Duration,
    pub io_ttl: Option<Duration>,
    pub ipv6_first: Option<bool>,
}
impl HandlerConfig {
    pub fn new(
        v4: Vec<Ipv4Addr>,
        v6: Vec<Ipv6Addr>,
        connect_ttl: Duration,
        retry_ttl: Duration,
        io_ttl: Option<Duration>,
        ipv6_first: Option<bool>,
    ) -> Self {
        Self {
            ip4: Mutex::new(IpPool::new(v4, Ipv4Addr::UNSPECIFIED)),
            ip6: Mutex::new(IpPool::new(v6, Ipv6Addr::UNSPECIFIED)),
            connect_ttl,
            retry_ttl,
            io_ttl,
            ipv6_first,
        }
    }
    pub fn next_ip4(&self) -> Ipv4Addr {
        self.ip4.lock().unwrap().next()
    }
    pub fn next_ip6(&self) -> Ipv6Addr {
        self.ip6.lock().unwrap().next()
    }
}

#[derive(Debug)]
struct IpPool<T> {
    val: Vec<T>,
    index: usize,
    default: T,
}
impl<T> IpPool<T>
where
    T: Clone,
{
    fn new(iter: Vec<T>, default: T) -> Self {
        Self {
            val: iter,
            index: 0,
            default,
        }
    }
    fn next(&mut self) -> T {
        if self.index >= self.val.len() {
            self.index = 0;
        }
        let ans = self.val.get(self.index).unwrap_or(&self.default).to_owned();
        self.index += 1;
        ans
    }
}

pub fn read_config(file_name: &str) -> anyhow::Result<(HostConfig, HandlerConfig)> {
    use std::{fs::File, io::prelude::*};
    let mut buf = String::new();
    let _ = File::open(file_name)
        .with_context(|| format!("Config file {} not found", file_name))?
        .read_to_string(&mut buf)?;
    let res: toml_file::Config = toml::from_str(&buf)?;
    Ok(res.into())
}

mod toml_file {
    // it sucks, but anyway it works
    use serde::Deserialize;
    use std::{net::SocketAddr, time::Duration};

    #[derive(Deserialize)]
    pub struct Config {
        host: SocketAddr,
        pool: Vec<std::net::IpAddr>,
        timeout: ConfigTimeout,
        tui: bool,
        ipv6_first: Option<bool>,
    }

    #[derive(Deserialize)]
    struct ConfigTimeout {
        connect: u64,
        retry: u64,
        io: Option<u64>,
    }
    impl From<Config> for (super::HostConfig, super::HandlerConfig) {
        fn from(val: Config) -> Self {
            let mut v4 = Vec::new();
            let mut v6 = Vec::new();
            for ip in val.pool {
                match ip {
                    std::net::IpAddr::V4(x) => v4.push(x),
                    std::net::IpAddr::V6(x) => v6.push(x),
                }
            }
            (
                super::HostConfig {
                    host: val.host,
                    tui: val.tui,
                },
                super::HandlerConfig::new(
                    v4,
                    v6,
                    Duration::from_millis(val.timeout.connect),
                    Duration::from_millis(val.timeout.retry),
                    val.timeout.io.map(Duration::from_secs),
                    val.ipv6_first,
                ),
            )
        }
    }
}
