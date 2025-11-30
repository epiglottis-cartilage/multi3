use crate::Result;
use ouroboros::self_referencing;
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair};
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, Stream,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
};
use std::{
    cell::LazyCell,
    collections::BTreeMap,
    io::prelude::*,
    net::TcpStream,
    sync::{Arc, Mutex},
};

pub const TLS_CLIENT: LazyCell<Arc<ClientConfig>> = LazyCell::new(|| {
    let root_store = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    config.key_log = Arc::new(rustls::KeyLogFile::new());
    Arc::new(config)
});

const CA: CertificateDer = CertificateDer::from_slice(include_bytes!("../cert/root-crt.der"));
const ISSUER: LazyCell<Issuer<'static, KeyPair>> = LazyCell::new(|| {
    Issuer::from_ca_cert_der(
        &CA,
        KeyPair::from_der_and_sign_algo(
            &PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                include_bytes!("../cert/root-key.der").as_slice(),
            )),
            &rcgen::PKCS_RSA_SHA256,
        )
        .unwrap(),
    )
    .unwrap()
});
static CERT_CACHE: Mutex<BTreeMap<String, Arc<ServerConfig>>> = Mutex::new(BTreeMap::new());
fn generate_server_config(domain: String) -> Arc<ServerConfig> {
    use std::collections::btree_map::Entry as E;
    match CERT_CACHE.lock().unwrap().entry(domain.clone()) {
        E::Occupied(o) => o.get().clone(),
        E::Vacant(v) => {
            let mut params = CertificateParams::new(vec![domain]).unwrap();
            params.distinguished_name = DistinguishedName::new();
            params
                .distinguished_name
                .push(DnType::CommonName, "Multi3 Generated Certificate");

            let now = time::OffsetDateTime::now_utc();
            params.not_before = now.replace_year(now.year() - 1).unwrap();
            params.not_after = now.replace_year(now.year() + 5).unwrap();
            params.is_ca = rcgen::IsCa::NoCa;

            let key_pair = KeyPair::generate().unwrap();
            // 使用根CA的私钥进行签名
            let cert = params.signed_by(&key_pair, &*ISSUER).unwrap();

            let config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert.clone().into()],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der())),
                )
                .unwrap();
            let cfg = Arc::new(config);
            v.insert(cfg.clone());
            cfg
        }
    }
}

#[self_referencing]
#[derive(Debug)]
pub struct ServerStream {
    inner: TcpStream,
    conn: ServerConnection,
    #[borrows( mut conn,mut inner)]
    #[not_covariant]
    stream: Stream<'this, ServerConnection, TcpStream>,
}
#[self_referencing]
#[derive(Debug)]
pub struct ClientStream {
    inner: TcpStream,
    conn: ClientConnection,
    #[borrows(mut conn,mut inner)]
    #[not_covariant]
    stream: Stream<'this, ClientConnection, TcpStream>,
}

#[derive(Debug)]
pub enum WarpedStream {
    Direct(TcpStream),
    Server(ServerStream),
    Client(ClientStream),
}
impl From<TcpStream> for WarpedStream {
    fn from(value: TcpStream) -> Self {
        Self::Direct(value)
    }
}
impl WarpedStream {
    pub fn new(remote: TcpStream) -> Self {
        Self::Direct(remote)
    }
    pub fn new_server(remote: TcpStream, domain: String) -> Result<Self> {
        let conn = ServerConnection::new(generate_server_config(domain))?;
        Ok(WarpedStream::Server(ServerStream::try_new(
            remote,
            conn,
            |conn: &mut ServerConnection, sock: &mut TcpStream| {
                let mut s = Stream::new(conn, sock);
                s.flush()?;
                Ok::<_, std::io::Error>(s)
            },
        )?))
    }
    pub fn new_client(remote: TcpStream, name: ServerName<'static>) -> Result<Self> {
        let conn = ClientConnection::new(TLS_CLIENT.clone(), name).unwrap();
        Ok(WarpedStream::Client(ClientStream::try_new(
            remote,
            conn,
            |conn: &mut ClientConnection, sock: &mut TcpStream| {
                let mut s = Stream::new(conn, sock);
                s.flush()?;
                Ok::<_, std::io::Error>(s)
            },
        )?))
    }
}
impl Read for WarpedStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            WarpedStream::Direct(stream) => stream.read(buf),
            WarpedStream::Server(stream) => stream.with_stream_mut(|s| s.read(buf)),
            WarpedStream::Client(stream) => stream.with_stream_mut(|s| s.read(buf)),
        }
    }
}
impl Write for WarpedStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            WarpedStream::Direct(stream) => stream.write(buf),
            WarpedStream::Server(stream) => stream.with_stream_mut(|s| s.write(buf)),
            WarpedStream::Client(stream) => stream.with_stream_mut(|s| s.write(buf)),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            WarpedStream::Direct(stream) => stream.flush(),
            WarpedStream::Server(stream) => stream.with_stream_mut(|s| s.flush()),
            WarpedStream::Client(stream) => stream.with_stream_mut(|s| s.flush()),
        }
    }
}

pub struct ReadHalf {
    inner: Arc<Mutex<WarpedStream>>,
}
impl ReadHalf {
    pub fn inner(&mut self) -> std::sync::MutexGuard<'_, WarpedStream> {
        self.inner.lock().unwrap()
    }
}

pub struct WriteHalf {
    inner: Arc<Mutex<WarpedStream>>,
}
impl WriteHalf {
    pub fn inner(&mut self) -> std::sync::MutexGuard<'_, WarpedStream> {
        self.inner.lock().unwrap()
    }
}

impl WarpedStream {
    pub fn split(self) -> (ReadHalf, WriteHalf) {
        // let is_write_vectored = stream.is_write_vectored();

        let inner = Arc::new(Mutex::new(self));

        let rd = ReadHalf {
            inner: inner.clone(),
        };

        let wr = WriteHalf { inner };

        (rd, wr)
    }
}
