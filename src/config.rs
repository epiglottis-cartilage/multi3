use crate::Result;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Mutex,
    time::Duration,
};
// pub type Error = Box<dyn std::error::Error>;

pub struct Routing {
    pub host: Box<[SocketAddr]>,
    pub pool: IpPool,
}

pub struct Config {
    pub connect_ttl: Duration,
    pub retry_ttl: Duration,
    pub io_ttl: Duration,
    pub ipv6_first: Option<bool>,
    pub tui: bool,
}
pub struct Pool<T: Clone> {
    pool: Box<[T]>,
    index: Mutex<usize>,
}
impl<T: Clone> Pool<T> {
    pub fn new(pool: Box<[T]>) -> Self {
        Self {
            pool,
            index: Mutex::new(0),
        }
    }
    pub fn next(&self) -> Option<T> {
        if self.pool.is_empty() {
            return None;
        }
        let mut index = self.index.lock().unwrap();
        let item = unsafe { self.pool.get_unchecked(*index) };
        *index += 1;
        if *index >= self.pool.len() {
            *index = 0;
        }
        Some(item.to_owned())
    }
}
pub struct IpPool {
    pub pool_v4: Pool<Ipv4Addr>,
    pub pool_v6: Pool<Ipv6Addr>,
}
impl IpPool {
    pub fn new(pool: Vec<IpAddr>) -> Self {
        let mut v4 = Vec::new();
        let mut v6 = Vec::new();
        for ip in pool {
            match ip {
                IpAddr::V4(ip) => v4.push(ip),
                IpAddr::V6(ip) => v6.push(ip),
            }
        }
        Self {
            pool_v4: Pool::new(v4.into_boxed_slice()),
            pool_v6: Pool::new(v6.into_boxed_slice()),
        }
    }
}

pub fn read_config(file_name: &str) -> Result<(Config, Vec<Routing>)> {
    use std::{fs::File, io::prelude::*};
    let mut buf = String::new();
    let _ = File::open(file_name)?.read_to_string(&mut buf)?;
    let res: toml_file::Config = toml::from_str(&buf)?;
    let config = Config {
        connect_ttl: Duration::from_millis(res.timeout.connect),
        retry_ttl: Duration::from_millis(res.timeout.retry),
        io_ttl: Duration::from_millis(res.timeout.io),
        ipv6_first: res.ipv6_first,
        tui: res.tui,
    };
    let routing = res
        .routing
        .into_iter()
        .map(|r| Routing {
            host: r.host.into_boxed_slice(),
            pool: IpPool::new(r.pool),
        })
        .collect::<Vec<_>>();
    return Ok((config, routing));
}
mod toml_file {
    // it sucks, but anyway it works
    use serde::Deserialize;
    use std::net::{IpAddr, SocketAddr};

    #[derive(Deserialize)]
    pub struct Routing {
        pub host: Vec<SocketAddr>,
        pub pool: Vec<IpAddr>,
    }

    #[derive(Deserialize)]
    pub struct Config {
        pub routing: Vec<Routing>,
        pub timeout: Timeout,
        pub tui: bool,
        pub ipv6_first: Option<bool>,
    }

    #[derive(Deserialize)]
    pub struct Timeout {
        pub connect: u64,
        pub retry: u64,
        pub io: u64,
    }
}
