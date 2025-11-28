use crate::Result;
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair};
use std::collections::BTreeMap;
use std::{cell::LazyCell, sync::Arc};
use tokio::{io::AsyncWriteExt, net::TcpStream, sync::Mutex};
use tokio_rustls::{
    TlsAcceptor, TlsConnector, TlsStream,
    rustls::{
        self, ClientConfig, RootCertStore, ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject},
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

const CERT: CertificateDer = CertificateDer::from_slice(include_bytes!("../cert/root-crt.der"));
const ISSUER: LazyCell<Issuer<'static, KeyPair>> = LazyCell::new(|| {
    Issuer::from_ca_cert_der(
        &CERT,
        KeyPair::from_pem(include_str!("../cert/root-key.pem")).unwrap(),
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
            // 设置证书有效期（例如，24小时）
            params.not_before = rcgen::date_time_ymd(2000, 1, 1);
            params.not_after = rcgen::date_time_ymd(2100, 1, 1);
            // 使用根CA的私钥进行签名
            let cert = params.signed_by(ISSUER.key(), &*ISSUER).unwrap();
            let config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert.clone().into(), CERT],
                    PrivateKeyDer::from_pem_slice(include_str!("../cert/root-key.pem").as_bytes())
                        .unwrap(),
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
        Ok(Self::Tls(TlsStream::Server(stream.into())))
    }
    pub async fn new_client(remote: TcpStream, name: ServerName<'static>) -> Result<Self> {
        let connector = TlsConnector::from(TLS_CLIENT.clone());
        let stream = connector.connect(name, remote).await?;
        Ok(Self::Tls(TlsStream::Client(stream.into())))
    }
    pub fn new_direct(remote: TcpStream) -> Self {
        Self::Direct(remote)
    }
    pub async fn write_all(&mut self, buf: &[u8]) -> std::result::Result<(), std::io::Error> {
        match self {
            Stream::Direct(stream) => stream.write_all(buf).await,
            Stream::Tls(stream) => stream.write_all(buf).await,
        }
    }
}
