use crate::Result;
use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Mutex,
};
pub struct HostConfig {
    pub host: SocketAddr,
    pub tui: bool,
}
#[derive(Debug)]
pub struct HandlerConfig {
    ip4: Mutex<IpPool<Ipv4Addr>>,
    ip6: Mutex<IpPool<Ipv6Addr>>,
    pub ipv6_first: Option<bool>,
    pub has_ipv4: bool,
    pub has_ipv6: bool,
    pub host: SocketAddr,
}
impl HandlerConfig {
    pub fn new(
        v4: Vec<Ipv4Addr>,
        v6: Vec<Ipv6Addr>,
        ipv6_first: Option<bool>,
        host: SocketAddr,
    ) -> Self {
        Self {
            has_ipv4: !v4.is_empty(),
            has_ipv6: !v6.is_empty(),
            ip4: Mutex::new(IpPool::new(v4, Ipv4Addr::UNSPECIFIED)),
            ip6: Mutex::new(IpPool::new(v6, Ipv6Addr::UNSPECIFIED)),
            ipv6_first,
            host,
        }
    }
    pub fn next_v4(&self) -> Ipv4Addr {
        self.ip4.lock().unwrap().next()
    }
    pub fn next_v6(&self) -> Ipv6Addr {
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

pub fn read_config(file_name: &str) -> Result<(HostConfig, HandlerConfig)> {
    use std::{fs::File, io::prelude::*};
    let mut buf = String::new();
    let _ = File::open(file_name)?.read_to_string(&mut buf)?;
    let res: toml_file::Config = toml::from_str(&buf).unwrap();
    Ok(res.into())
}

mod toml_file {
    // it sucks, but anyway it works
    use serde::Deserialize;
    use std::net::SocketAddr;

    #[derive(Deserialize)]
    pub struct Config {
        host: SocketAddr,
        pool: Vec<std::net::IpAddr>,
        tui: bool,
        ipv6_first: Option<bool>,
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
                super::HandlerConfig::new(v4, v6, val.ipv6_first, val.host),
            )
        }
    }
}
