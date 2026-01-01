use std::borrow::Cow;

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, derive_more::From)]
pub enum Error {
    #[from]
    IoError(std::io::Error),
    #[from]
    ParseError(toml::de::Error),
    #[from]
    TlsError(tokio_rustls::rustls::Error),
    InvalidRequest(Cow<'static, str>),
}
impl std::error::Error for Error {}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(error) => write!(f, "IO error: {}", error.kind()),
            Self::TlsError(error) => write!(f, "ILS error: {}", error),
            Self::ParseError(error) => write!(f, "Parse error: {}", error),
            Self::InvalidRequest(s) => write!(f, "Invalid request in {s}"),
        }
    }
}
