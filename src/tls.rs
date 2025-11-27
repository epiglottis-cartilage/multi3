use crate::Result;
use rustls::{
    ClientConfig, ServerConfig, ServerConnection,
    pki_types::{CertificateDer, Der, PrivateKeyDer, pem::PemObject},
};
use std::{cell::LazyCell, net::TcpStream, sync::Arc};

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

pub struct TlsTcpStream {
    socket: TcpStream,
    tls_conn: ServerConnection,
}

pub fn attach(remote: TcpStream) -> Result<TlsTcpStream> {
    let conn: rustls::ServerConnection = rustls::ServerConnection::new(TLS_SERVER.clone())?;
    Ok(TlsTcpStream {
        socket: remote,
        tls_conn: conn,
    })
}
