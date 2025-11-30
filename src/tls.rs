use crate::Result;
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair};
use std::collections::BTreeMap;
use std::{cell::LazyCell, sync::Arc};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    sync::Mutex,
};
use tokio_rustls::{
    TlsAcceptor, TlsConnector, TlsStream,
    rustls::{
        self, ClientConfig, RootCertStore, ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
    },
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

static CERT_CACHE: Mutex<BTreeMap<String, Arc<ServerConfig>>> = Mutex::const_new(BTreeMap::new());
async fn generate_server_config(domain: String) -> Arc<ServerConfig> {
    use std::collections::btree_map::Entry as E;
    match CERT_CACHE.lock().await.entry(domain.clone()) {
        E::Occupied(o) => o.get().clone(),
        E::Vacant(v) => {
            let mut params = CertificateParams::new(vec![domain.to_string()]).unwrap();
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
pub enum Stream {
    Direct(TcpStream),
    Tls(TlsStream<TcpStream>),
}
impl Stream {
    pub async fn new_server(remote: TcpStream, host_name: &str) -> Result<Self> {
        let connector = TlsAcceptor::from(generate_server_config(host_name.to_string()).await);
        let stream = connector.accept(remote).await?;
        Ok(Self::Tls(TlsStream::Server(stream)))
    }
    pub async fn new_client(remote: TcpStream, name: ServerName<'static>) -> Result<Self> {
        let connector = TlsConnector::from(TLS_CLIENT.clone());
        let stream = connector.connect(name, remote).await?;
        Ok(Self::Tls(TlsStream::Client(stream)))
    }
    pub fn new_direct(remote: TcpStream) -> Self {
        Self::Direct(remote)
    }
}
use std::pin::Pin;
use std::task::{Context, Poll};
impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Direct(x) => Pin::new(x).poll_read(cx, buf),
            Self::Tls(x) => Pin::new(x).poll_read(cx, buf),
        }
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::result::Result<usize, std::io::Error>> {
        match self.get_mut() {
            Self::Direct(x) => Pin::new(x).poll_write(cx, buf),
            Self::Tls(x) => Pin::new(x).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), std::io::Error>> {
        match self.get_mut() {
            Self::Direct(x) => Pin::new(x).poll_flush(cx),
            Self::Tls(x) => Pin::new(x).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), std::io::Error>> {
        match self.get_mut() {
            Self::Direct(x) => Pin::new(x).poll_shutdown(cx),
            Self::Tls(x) => Pin::new(x).poll_shutdown(cx),
        }
    }
}
