pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, derive_more::From)]
#[allow(dead_code)]
pub enum Error {
    #[from]
    IoError(std::io::Error),
    #[from]
    ParseError(toml::de::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
impl std::error::Error for Error {}
// impl<E> From<std::sync::mpsc::SendError<E>> for Error {
//     fn from(_value: std::sync::mpsc::SendError<E>) -> Self {
//         Self::ChannelError
//     }
// }
