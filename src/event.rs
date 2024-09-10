use std::{borrow::Cow, net::IpAddr};
#[derive(Debug)]
pub enum Event {
    Received(i32, IpAddr),
    Resolved(String),
    Connected(IpAddr, IpAddr),
    Done(),
    Upload(usize),
    Download(usize),
    Retry(),
    Error(Cow<'static, str>),
}
