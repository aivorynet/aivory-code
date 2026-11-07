//! Local IPC endpoint: a Windows named pipe or a Unix domain socket, chosen by platform.

use std::io::{self, BufReader, Read, Write};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

/// A named local endpoint, for example `default` or `test-1234`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    name: String,
}

impl Endpoint {
    pub const DEFAULT_NAME: &'static str = "default";

    /// The name may contain ASCII letters, digits, `.`, `_`, and `-`.
    pub fn new(name: &str) -> io::Result<Self> {
        let ok = !name.is_empty()
            && name.len() <= 64
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !ok {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "endpoint name must be 1-64 characters of [A-Za-z0-9._-]",
            ));
        }
        Ok(Self {
            name: name.to_string(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The pipe path or the socket path, for display.
    pub fn address(&self) -> String {
        platform::address(&self.name)
    }
}

impl Default for Endpoint {
    fn default() -> Self {
        Self {
            name: Self::DEFAULT_NAME.to_string(),
        }
    }
}

/// An accepted or opened connection.
pub struct Connection {
    pub reader: BufReader<Box<dyn Read + Send>>,
    pub writer: Box<dyn Write + Send>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Connection")
    }
}

/// Accepts connections on an endpoint. The endpoint exists as soon as [`Listener::bind`] returns.
#[derive(Debug)]
pub struct Listener {
    inner: platform::Listener,
}

impl Listener {
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        Ok(Self {
            inner: platform::Listener::bind(&endpoint.name)?,
        })
    }

    /// Block until a client connects.
    pub fn accept(&mut self) -> io::Result<Connection> {
        self.inner.accept()
    }
}

/// Connect to a running daemon. Retries for a short time while the server is busy.
pub fn connect(endpoint: &Endpoint) -> io::Result<Connection> {
    platform::connect(&endpoint.name)
}
