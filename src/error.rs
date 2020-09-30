use std::io;
use std::fmt::{Debug, Display, Formatter};
use trust_dns_server::authority::LookupError;
use futures_util::io::ErrorKind;

#[derive(Debug)]
pub(crate) struct Error<E>(pub(crate) E);

impl Error<io::Error> {
    pub(crate) fn msg(kind: io::ErrorKind, msg: &str) -> Self {
        let error: Box<dyn std::error::Error + Send + Sync> = From::from(msg);
        Error(io::Error::new(kind, error))
    }
}

impl <E: 'static + Into<Box<dyn std::error::Error + Send + Sync>>> Error<E> {
    pub(crate) fn io(self) -> Error<std::io::Error> {
        Error(io::Error::new(io::ErrorKind::Other, self.0))
    }
}

impl <E: 'static + std::error::Error> std::error::Error for Error<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl <E: Display> Display for Error<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        self.0.fmt(f)
    }
}

impl From<io::ErrorKind> for Error<io::Error> {
    fn from(kind: ErrorKind) -> Self {
        Error(From::from(kind))
    }
}

impl From<&str> for Error<io::Error> {
    fn from(msg: &str) -> Self {
        Error::msg(io::ErrorKind::Other, msg)
    }
}

impl Into<acme_lib::Error> for Error<io::Error> {
    fn into(self) -> acme_lib::Error {
        acme_lib::Error::from(self.0)
    }
}

impl Into<LookupError> for Error<io::Error> {
    fn into(self) -> LookupError {
        LookupError::Io(self.0)
    }
}

impl Default for Error<io::Error> {
    fn default() -> Self {
        From::from(io::ErrorKind::Other)
    }
}
