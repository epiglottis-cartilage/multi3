use std::borrow::Cow;

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, derive_more::From)]
pub enum Error {
    #[from]
    IoError(std::io::Error),
    #[from]
    ParseError(toml::de::Error),
    #[from]
    TlsError(rustls::Error),
    ChannelError,
    PoolEmpty,
    InvalidRequest(Cow<'static, str>),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(error) => write!(f, "IO error: {}", error.kind()),
            Self::ParseError(error) => write!(f, "Parse error: {}", error),
            Self::TlsError(error) => write!(f, "TLS error: {}", error),
            Self::ChannelError => write!(f, "Channel error"),
            Self::PoolEmpty => write!(f, "Connection pool is empty"),
            Self::InvalidRequest(s) => write!(f, "Invalid request in {s}"),
        }
    }
}
impl std::error::Error for Error {}
impl<E> From<std::sync::mpsc::SendError<E>> for Error {
    fn from(_error: std::sync::mpsc::SendError<E>) -> Self {
        Self::ChannelError
    }
}
