//! Unix domain socket: `$XDG_RUNTIME_DIR/aivory/<name>.sock`, else `<tmp>/aivory-<user>/<name>.sock`.
//! The directory has mode 0700 and the socket mode 0600.

use std::fs;
use std::io::{self, BufReader};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::Connection;

const CONNECT_RETRY: Duration = Duration::from_secs(3);

fn socket_path(name: &str) -> PathBuf {
    let dir = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => PathBuf::from(d).join("aivory"),
        None => {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            std::env::temp_dir().join(format!("aivory-{user}"))
        }
    };
    dir.join(format!("{name}.sock"))
}

pub fn address(name: &str) -> String {
    socket_path(name).display().to_string()
}

#[derive(Debug)]
pub struct Listener {
    inner: UnixListener,
    path: PathBuf,
}

impl Listener {
    pub fn bind(name: &str) -> io::Result<Self> {
        let path = socket_path(name);
        if let Some(dir) = path.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another daemon listens on {}", path.display()),
                ));
            }
            fs::remove_file(&path)?; // stale socket of a dead daemon
        }
        let inner = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(Self { inner, path })
    }

    pub fn accept(&mut self) -> io::Result<Connection> {
        let (stream, _) = self.inner.accept()?;
        let writer = stream.try_clone()?;
        Ok(Connection {
            reader: BufReader::new(Box::new(stream)),
            writer: Box::new(writer),
        })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn connect(name: &str) -> io::Result<Connection> {
    let path = socket_path(name);
    let started = Instant::now();
    loop {
        match UnixStream::connect(&path) {
            Ok(stream) => {
                let writer = stream.try_clone()?;
                return Ok(Connection {
                    reader: BufReader::new(Box::new(stream)),
                    writer: Box::new(writer),
                });
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) && started.elapsed() < CONNECT_RETRY =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}
