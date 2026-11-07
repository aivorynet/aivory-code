/// Errors of the link. The messages never contain a secret key.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("cannot connect: {0}")]
    Connect(String),
    /// The daemon answered the hello with a JSON-RPC error.
    #[error("the daemon rejected the device: {0}")]
    Rejected(String),
    #[error("the request timed out")]
    Timeout,
    #[error("the connection is closed")]
    Closed,
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}
