//! `aivoryd serve --iroh`: the JSON-RPC surface over iroh (QUIC with hole punching and a
//! relay) for the phone app outside the LAN (ADR 0015).
//!
//! The daemon is synchronous. The link runs in its own tokio runtime and bridges each
//! connection to [`crate::server::serve_connection`], the code that the pipe and the
//! WebSocket link use: one thread reads frames and calls the dispatch, one thread writes the
//! outbox. The adapters here turn the frames (4-byte length, then one JSON-RPC message) into
//! lines and back.
//!
//! Each connection has one bidirectional stream. The first frame is the request `link/hello`
//! (see [`aivory_link::hello`]). The QUIC handshake proves the key of the phone, so the
//! daemon compares the key in the hello with the key of the connection and looks it up in
//! `devices.json`. A wrong or removed device gets a JSON-RPC error and the daemon closes the
//! connection. `devices/remove` also closes a connection that is open.
//!
//! A new device, or a device whose hello has `needToken`, gets a LAN token in the reply: one
//! pairing serves both links. The requests of the connection run as the device
//! ([`crate::remote::Caller`]).

use std::io::{self, BufReader, Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aivory_link::endpoint::{self, EndpointConfig, RelayChoice};
use aivory_link::frame::{read_frame, write_frame};
use aivory_link::hello::{HELLO_METHOD, Hello, HelloReply, UNAUTHORIZED};
use aivory_link::iroh::endpoint::{Connection as QuicConnection, Incoming, RecvStream, SendStream};
use aivory_link::iroh::{Endpoint, Watcher};
use aivory_link::keys;
use serde_json::{Value, json};
use tokio::runtime::{Builder, Handle, Runtime};

use crate::endpoint::Connection;
use crate::remote::{Caller, Pairings, as_caller, lan_ip};
use crate::rpc::Daemon;
use crate::server::serve_connection;

const HELLO_TIMEOUT: Duration = Duration::from_secs(15);
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// How often an open connection checks that its device is still paired.
const RECHECK: Duration = Duration::from_secs(2);
const ONLINE_WAIT: Duration = Duration::from_secs(20);

/// QUIC close codes.
const CLOSE_BYE: u32 = 0;
const CLOSE_REJECTED: u32 = 1;
const CLOSE_REMOVED: u32 = 2;

/// Settings of the iroh link.
#[derive(Debug, Clone, Default)]
pub struct IrohOptions {
    /// The file of the secret key of the daemon. `None`: a new key for each run.
    pub secret_file: Option<PathBuf>,
    /// The setting `remote.relay`.
    pub relay: RelayChoice,
    /// Bind one IPv4 socket on this address. `None`: all interfaces.
    pub bind: Option<SocketAddr>,
}

/// The running link. Dropping it closes the endpoint.
#[derive(Debug)]
pub struct IrohLink {
    runtime: Option<Runtime>,
    endpoint: Endpoint,
    node: String,
}

struct Context {
    daemon: Arc<Daemon>,
    pairings: Arc<Pairings>,
    node: String,
}

impl IrohLink {
    /// Bind the endpoint and accept connections on a runtime of its own. The pairing links
    /// of `pairings` name the node id and the relay from now on.
    pub fn start(
        daemon: Arc<Daemon>,
        pairings: Arc<Pairings>,
        options: IrohOptions,
    ) -> io::Result<Self> {
        let secret = match &options.secret_file {
            Some(file) => keys::load_or_create_secret(file)?,
            None => {
                eprintln!("aivoryd: no data folder, the iroh key lasts for this run only");
                keys::parse_secret(&keys::generate_secret_hex()).map_err(io::Error::other)?
            }
        };
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("aivoryd-iroh")
            .enable_all()
            .build()?;
        let endpoint = runtime
            .block_on(endpoint::bind(EndpointConfig {
                secret,
                relay: options.relay.clone(),
                bind: options.bind,
                accept: true,
            }))
            .map_err(io::Error::other)?;
        let node = endpoint.id().to_string();
        pairings.set_iroh(&node, options.relay.url());
        pairings.set_direct(&dialable(&endpoint.bound_sockets()));
        if options.relay == RelayChoice::Default {
            runtime.spawn(publish_relay(endpoint.clone(), Arc::clone(&pairings)));
        }
        let ctx = Arc::new(Context {
            daemon,
            pairings,
            node: node.clone(),
        });
        runtime.spawn(accept_loop(endpoint.clone(), ctx));
        Ok(Self {
            runtime: Some(runtime),
            endpoint,
            node,
        })
    }

    /// The node id of the daemon (64 hex characters).
    pub fn node_id(&self) -> &str {
        &self.node
    }

    /// The local sockets of the endpoint. A phone in the same network can dial them.
    pub fn direct_addrs(&self) -> Vec<SocketAddr> {
        self.endpoint.bound_sockets()
    }

    /// The addresses that the pairing link names (`direct=`): the sockets of the endpoint, with
    /// the LAN address of this machine in place of a wildcard address.
    pub fn dialable_addrs(&self) -> Vec<SocketAddr> {
        dialable(&self.endpoint.bound_sockets())
    }
}

impl Drop for IrohLink {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            let endpoint = self.endpoint.clone();
            let _ = runtime
                .block_on(async { tokio::time::timeout(CLOSE_WAIT, endpoint.close()).await });
            runtime.shutdown_background();
        }
    }
}

/// Turn the bound sockets into addresses that a phone can dial. A wildcard IPv4 address takes
/// the LAN address; a wildcard IPv6 address is left out.
fn dialable(bound: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for addr in bound {
        let ok = if addr.ip().is_unspecified() {
            lan_ip()
                .filter(|ip| ip.is_ipv4() == addr.is_ipv4())
                .map(|ip| SocketAddr::new(ip, addr.port()))
        } else {
            Some(*addr)
        };
        if let Some(addr) = ok
            && !out.contains(&addr)
        {
            out.push(addr);
        }
    }
    out
}

/// Wait for the home relay, then name it in the pairing links.
async fn publish_relay(endpoint: Endpoint, pairings: Arc<Pairings>) {
    if tokio::time::timeout(ONLINE_WAIT, endpoint.online())
        .await
        .is_err()
    {
        eprintln!("aivoryd: the iroh endpoint found no relay; pairing links name no relay");
        return;
    }
    let addr = endpoint.watch_addr().get();
    let relay = addr.relay_urls().next().map(ToString::to_string);
    pairings.set_iroh(&endpoint.id().to_string(), relay.as_deref());
}

async fn accept_loop(endpoint: Endpoint, ctx: Arc<Context>) {
    while let Some(incoming) = endpoint.accept().await {
        let ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            if let Err(e) = connection(incoming, &ctx).await {
                eprintln!("aivoryd: iroh connection ended: {e}");
            }
        });
    }
}

async fn connection(incoming: Incoming, ctx: &Context) -> io::Result<()> {
    let conn = incoming.await.map_err(io::Error::other)?;
    let peer = conn.remote_id().to_string();
    let (mut send, mut recv) = conn.accept_bi().await.map_err(io::Error::other)?;
    let first = tokio::time::timeout(HELLO_TIMEOUT, read_frame(&mut recv))
        .await
        .map_err(|_| io::Error::other("no hello"))??
        .ok_or_else(|| io::Error::other("the phone closed before the hello"))?;
    let request: Value = serde_json::from_slice(&first).unwrap_or(Value::Null);
    let id = request.get("id").cloned().unwrap_or(Value::Null);

    let mut need_token = false;
    let admitted = if request["method"] != HELLO_METHOD {
        Err(format!("the first message must be {HELLO_METHOD}"))
    } else {
        serde_json::from_value::<Hello>(request["params"].clone())
            .map_err(|e| format!("bad hello: {e}"))
            .and_then(|hello| {
                need_token = hello.need_token;
                ctx.pairings.admit_key(&peer, &hello)
            })
    };
    let (device, new) = match admitted {
        Ok(ok) => ok,
        Err(message) => {
            let error = json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": UNAUTHORIZED, "message": message },
            });
            let _ = write_frame(&mut send, error.to_string().as_bytes()).await;
            finish(&mut send).await;
            conn.close(CLOSE_REJECTED.into(), b"rejected");
            return Ok(());
        }
    };
    let reply = HelloReply {
        device_id: device.id.clone(),
        name: device.name.clone(),
        newly_paired: new,
        node: ctx.node.clone(),
        token: (new || need_token)
            .then(|| ctx.pairings.issue_token(&device.id))
            .flatten(),
    };
    let ok = json!({ "jsonrpc": "2.0", "id": id, "result": reply });
    if let Err(e) = write_frame(&mut send, ok.to_string().as_bytes()).await {
        ctx.pairings.key_offline(&peer);
        return Err(e);
    }
    eprintln!("aivoryd: iroh device '{}' connected", device.name);

    // The synchronous dispatch runs on its own threads.
    let handle = Handle::current();
    let (done_tx, mut done) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new().name("aivoryd-iroh-conn".into());
    let daemon = Arc::clone(&ctx.daemon);
    let caller = Caller::Device(device.id.clone());
    let reader = FrameReader::new(recv, handle.clone());
    let writer = FrameWriter {
        send: Some(send),
        conn: conn.clone(),
        buf: Vec::new(),
        handle,
    };
    thread.spawn(move || {
        let conn = Connection {
            reader: BufReader::new(Box::new(reader)),
            writer: Box::new(writer),
        };
        let _ = done_tx.send(as_caller(caller, || serve_connection(&daemon, conn)));
    })?;
    let result = loop {
        tokio::select! {
            result = &mut done => break result.unwrap_or(Ok(())),
            () = tokio::time::sleep(RECHECK) => {
                if !ctx.pairings.key_allowed(&peer) {
                    conn.close(CLOSE_REMOVED.into(), b"device removed");
                }
            }
        }
    };
    ctx.pairings.key_offline(&peer);
    result
}

/// Finish the stream and wait until the peer has all data, so that a close does not cut it.
async fn finish(send: &mut SendStream) {
    if send.finish().is_ok() {
        let _ = tokio::time::timeout(CLOSE_WAIT, send.stopped()).await;
    }
}

/// Reads frames as lines: each frame becomes one line, so the line reader of the pipe server
/// reads it. A cut or reset stream is an end of input, as on the WebSocket link.
struct FrameReader {
    recv: RecvStream,
    handle: Handle,
    line: Vec<u8>,
    pos: usize,
}

impl FrameReader {
    fn new(recv: RecvStream, handle: Handle) -> Self {
        Self {
            recv,
            handle,
            line: Vec::new(),
            pos: 0,
        }
    }
}

impl Read for FrameReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.line.len() {
            let frame = match self.handle.block_on(read_frame(&mut self.recv)) {
                Ok(Some(frame)) => frame,
                Ok(None) => return Ok(0),
                Err(e) if e.kind() == io::ErrorKind::InvalidData => return Err(e),
                Err(_) => return Ok(0),
            };
            // A JSON message needs no raw line break; a pretty-printed frame stays one line.
            self.line = String::from_utf8_lossy(&frame)
                .replace(['\n', '\r'], " ")
                .into_bytes();
            self.line.push(b'\n');
            self.pos = 0;
        }
        let n = out.len().min(self.line.len() - self.pos);
        out[..n].copy_from_slice(&self.line[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Writes lines as frames: a flush sends the buffered line as one frame. Dropping it
/// finishes the stream, waits for the phone, and closes the connection.
struct FrameWriter {
    send: Option<SendStream>,
    conn: QuicConnection,
    buf: Vec<u8>,
    handle: Handle,
}

impl Write for FrameWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let Some(send) = &mut self.send else {
            return Err(io::ErrorKind::BrokenPipe.into());
        };
        if self.buf.last() == Some(&b'\n') {
            self.buf.pop();
        }
        let result = self.handle.block_on(write_frame(send, &self.buf));
        self.buf.clear();
        result
    }
}

impl Drop for FrameWriter {
    fn drop(&mut self) {
        if let Some(mut send) = self.send.take() {
            self.handle.block_on(finish(&mut send));
        }
        self.conn.close(CLOSE_BYE.into(), b"bye");
    }
}
