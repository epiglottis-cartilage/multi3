use std::{borrow::Cow, net::IpAddr};
#[derive(Debug)]
pub enum Event {
    Received(IpAddr),
    Resolved(String),
    Connected(IpAddr, IpAddr),
    Done(),
    Upload(usize),
    Download(usize),
    Retry(),
    Error(Cow<'static, str>),
}
