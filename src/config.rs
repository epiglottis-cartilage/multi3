use crate::Result;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Mutex,
    time::Duration,
};

pub struct Config {
    pub host: SocketAddr,
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
    pub ipv6_first: Option<bool>,
    pub tui: bool,
}
struct Pool<T: Clone> {
    default: T,
    pool: Box<[T]>,
    index: Mutex<usize>,
}
impl<T: Clone> Pool<T> {
    pub fn new(pool: Box<[T]>, default: T) -> Self {
        Self {
            default,
            pool,
            index: Mutex::new(0),
        }
    }
    pub fn next(&self) -> T {
        if self.pool.is_empty() {
            return self.default.clone();
        }
        let mut index = self.index.lock().unwrap();
        let item = unsafe { self.pool.get_unchecked(*index) };
        *index += 1;
        if *index >= self.pool.len() {
            *index = 0;
        }
        item.to_owned()
    }
}
pub struct IpPool {
    pool_v4: Pool<Ipv4Addr>,
    pool_v6: Pool<Ipv6Addr>,
    pub have_v4: bool,
    pub have_v6: bool,
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
            have_v4: !v4.is_empty(),
            have_v6: !v6.is_empty(),
            pool_v4: Pool::new(v4.into_boxed_slice(), Ipv4Addr::UNSPECIFIED),
            pool_v6: Pool::new(v6.into_boxed_slice(), Ipv6Addr::UNSPECIFIED),
        }
    }
    #[inline]
    pub fn next_v4(&self) -> Ipv4Addr {
        self.pool_v4.next()
    }
    #[inline]
    pub fn next_v6(&self) -> Ipv6Addr {
        self.pool_v6.next()
    }
}

pub fn read_config(file_name: &str) -> Result<(Config, IpPool)> {
    use std::{fs::File, io::prelude::*};
    let mut buf = String::new();
    let _ = File::open(file_name)?.read_to_string(&mut buf)?;
    let res: toml_file::Config = toml::from_str(&buf)?;
    let config = Config {
        host: res.host,
        connect_timeout: Duration::from_millis(res.timeout.connect),
        io_timeout: Duration::from_millis(res.timeout.io),
        ipv6_first: res.ipv6_first,
        tui: res.tui,
    };
    let pool = IpPool::new(res.pool);
    return Ok((config, pool));
}
mod toml_file {
    // it sucks, but anyway it works
    use serde::Deserialize;
    use std::net::{IpAddr, SocketAddr};

    #[derive(Deserialize)]
    pub struct Config {
        pub host: SocketAddr,
        pub pool: Vec<IpAddr>,
        pub timeout: Timeout,
        pub tui: bool,
        pub ipv6_first: Option<bool>,
    }

    #[derive(Deserialize)]
    pub struct Timeout {
        pub connect: u64,
        pub io: u64,
    }
}
