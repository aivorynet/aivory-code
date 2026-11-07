//! The error type of the index.

/// Errors of the index layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("syntax layer: {0}")]
    Syntax(#[from] aivory_syntax::Error),
    /// A stub or a table does not encode or decode.
    #[error("encoding error: {0}")]
    Codec(String),
    /// A segment file is damaged or has an unknown format.
    #[error("corrupt segment: {0}")]
    Corrupt(String),
    #[error("git: {0}")]
    Git(String),
    #[error("unknown snapshot `{0}`")]
    UnknownSnapshot(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<postcard::Error> for Error {
    fn from(e: postcard::Error) -> Self {
        Error::Codec(e.to_string())
    }
}

impl From<fst::Error> for Error {
    fn from(e: fst::Error) -> Self {
        Error::Corrupt(e.to_string())
    }
}
