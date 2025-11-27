use crate::Result;
use rustls::{
    ClientConfig, ClientConnection, ServerConfig, ServerConnection, Stream,
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject},
};
use std::{
    cell::LazyCell,
    io::prelude::*,
    net::{SocketAddr, TcpStream},
    ops::{Deref, DerefMut},
    sync::Arc,
};

pub const TLS_CLIENT: LazyCell<Arc<ClientConfig>> = LazyCell::new(|| {
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    config.key_log = Arc::new(rustls::KeyLogFile::new());
    Arc::new(config)
});

const CERT: CertificateDer = CertificateDer::from_slice(include_bytes!("../cert/root-crt.der"));

pub const TLS_SERVER: LazyCell<Arc<ServerConfig>> = LazyCell::new(|| {
    #[allow(unused_mut)]
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CERT],
            PrivateKeyDer::from_pem_slice(include_str!("../cert/root-key.pem").as_bytes()).unwrap(),
        )
        .unwrap();
    Arc::new(config)
});

#[derive(Debug)]
pub enum WarpedStream {
    Direct(TcpStream),
    Server(TcpStream, ServerConnection),
    Client(TcpStream, ClientConnection),
}
impl From<TcpStream> for WarpedStream {
    fn from(value: TcpStream) -> Self {
        Self::Direct(value)
    }
}
impl WarpedStream {
    pub fn new(stream: TcpStream) -> Self {
        Self::Direct(stream)
    }
    pub fn new_server(remote: TcpStream) -> Result<Self> {
        let conn = ServerConnection::new(TLS_SERVER.clone())?;
        Ok(WarpedStream::Server(remote, conn))
    }
    pub fn new_client(remote: TcpStream, name: ServerName<'static>) -> Result<Self> {
        let conn = ClientConnection::new(TLS_CLIENT.clone(), name).unwrap();
        Ok(WarpedStream::Client(remote, conn))
    }
    pub fn local_addr(&self) -> SocketAddr {
        match self {
            WarpedStream::Direct(stream)
            | WarpedStream::Server(stream, _)
            | WarpedStream::Client(stream, _) => stream.local_addr(),
        }
        .unwrap()
    }
    pub fn peer_addr(&self) -> SocketAddr {
        match self {
            WarpedStream::Direct(stream)
            | WarpedStream::Server(stream, _)
            | WarpedStream::Client(stream, _) => stream.peer_addr(),
        }
        .unwrap()
    }
}
impl Read for WarpedStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            WarpedStream::Direct(stream) => stream.read(buf),
            WarpedStream::Server(stream, conn) => Stream::new(conn, stream).read(buf),
            WarpedStream::Client(stream, conn) => Stream::new(conn, stream).read(buf),
        }
    }
}
impl Write for WarpedStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            WarpedStream::Direct(stream) => stream.write(buf),
            WarpedStream::Server(stream, conn) => Stream::new(conn, stream).write(buf),
            WarpedStream::Client(stream, conn) => Stream::new(conn, stream).write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            WarpedStream::Direct(stream) => stream.flush(),
            WarpedStream::Server(stream, conn) => Stream::new(conn, stream).flush(),
            WarpedStream::Client(stream, conn) => Stream::new(conn, stream).flush(),
        }
    }
}
impl Deref for WarpedStream {
    type Target = TcpStream;
    fn deref(&self) -> &Self::Target {
        match self {
            WarpedStream::Direct(stream)
            | WarpedStream::Server(stream, _)
            | WarpedStream::Client(stream, _) => stream,
        }
    }
}
impl DerefMut for WarpedStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            WarpedStream::Direct(stream)
            | WarpedStream::Server(stream, _)
            | WarpedStream::Client(stream, _) => stream,
        }
    }
}
