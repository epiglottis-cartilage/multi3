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
    None,
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
            Protocol::Http =>      "    http://",
            Protocol::Https =>     "   https://",
            Protocol::Socks5Tcp => "T socks5://",
            Protocol::Socks5Udp => "U socks5://",
        }
    }
}
