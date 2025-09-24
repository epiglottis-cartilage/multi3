use std::{
    borrow::Cow,
    net::{IpAddr, SocketAddr},
};
#[derive(Debug)]
pub enum Event {
    Received(IpAddr),
    Recognized(Protocol),
    Resolved(String),
    Connected(IpAddr, SocketAddr),
    Done(),
    Upload(usize),
    Download(usize),
    Retry(),
    Error(Cow<'static, str>),
}

#[derive(Debug)]
pub enum Protocol {
    Http,
    Https,
    Socks5Tcp,
    Socks5Udp,
}
impl Protocol {
    pub fn display(&self) -> &str {
        match self {
            Protocol::Http => "H.",
            Protocol::Https => "Hs",
            Protocol::Socks5Tcp => "St",
            Protocol::Socks5Udp => "Su",
        }
    }
}
