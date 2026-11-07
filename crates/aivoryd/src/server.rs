use std::io::{self, BufRead, Read, Write};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread;

use serde_json::Value;

use crate::endpoint::{self, Connection, Endpoint, Listener};
use crate::outbox::{OUTBOX_CAPACITY, Out, Outbox};
use crate::rpc::Daemon;

/// Longest accepted request line. A longer line closes the connection.
const MAX_LINE_BYTES: u64 = 4 * 1024 * 1024;

/// The accept loop. One thread per connection.
#[derive(Debug)]
pub struct Server {
    daemon: Arc<Daemon>,
    endpoint: Endpoint,
    listener: Listener,
}

impl Server {
    /// Create the endpoint. Clients can connect as soon as this returns.
    pub fn bind(daemon: Arc<Daemon>, endpoint: Endpoint) -> io::Result<Self> {
        let listener = Listener::bind(&endpoint)?;
        Ok(Self {
            daemon,
            endpoint,
            listener,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Serve until a client calls `daemon/shutdown`.
    pub fn run(mut self) -> io::Result<()> {
        loop {
            let conn = self.listener.accept()?;
            if self.daemon.is_shutdown_requested() {
                return Ok(());
            }
            let daemon = Arc::clone(&self.daemon);
            let endpoint = self.endpoint.clone();
            thread::spawn(move || {
                let _ = serve_connection(&daemon, conn);
                if daemon.is_shutdown_requested() {
                    // Wake the accept loop so that it sees the flag.
                    let _ = endpoint::connect(&endpoint);
                }
            });
        }
    }
}

/// Read request lines until the client closes the connection. A writer thread sends what the
/// requests and the feed subscriptions put into the outbox.
pub fn serve_connection(daemon: &Daemon, conn: Connection) -> io::Result<()> {
    let Connection { mut reader, writer } = conn;
    let (outbox, rx) = Outbox::new(OUTBOX_CAPACITY);
    let writer = thread::spawn(move || write_loop(writer, &rx));
    let result = read_loop(daemon, &mut reader, &outbox);
    outbox.close();
    drop(outbox);
    let written = writer.join().unwrap_or(Ok(()));
    result.and(written)
}

fn read_loop(daemon: &Daemon, reader: &mut impl BufRead, outbox: &Outbox) -> io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.take(MAX_LINE_BYTES).read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = daemon.handle_line(&line, outbox) {
            outbox.send(response)?;
        }
        if daemon.is_shutdown_requested() {
            return Ok(());
        }
    }
}

fn write_loop(mut writer: Box<dyn Write + Send>, rx: &Receiver<Out>) -> io::Result<()> {
    for out in rx {
        match out {
            Out::Message(v) => write_json_line(&mut writer, &v)?,
            Out::Close => break,
        }
    }
    Ok(())
}

fn write_json_line(w: &mut dyn Write, v: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(v).map_err(io::Error::other)?;
    bytes.push(b'\n');
    w.write_all(&bytes)?;
    w.flush()
}
