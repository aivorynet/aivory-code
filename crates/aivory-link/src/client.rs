//! The phone side of the link, for an FFI layer.
//!
//! [`LinkClient`] is blocking and has no generic types. Requests and messages are JSON
//! strings. Responses to [`LinkClient::call`] go to the caller; every other message (a
//! notification, or a response to a request sent with [`LinkClient::send`]) goes to the
//! [`MessageSink`], on a thread of the client.
//!
//! The client runs its own tokio runtime. Do not call its methods from inside another tokio
//! runtime; call them from a plain thread. The sink may call the client.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, PublicKey};
use serde_json::{Value, json};
use tokio::runtime::Runtime;
use tokio::time::timeout;

use crate::endpoint::{self, EndpointConfig, RelayChoice};
use crate::frame::{read_frame, write_frame};
use crate::hello::{HELLO_METHOD, Hello, HelloReply};
use crate::pairlink::PairLink;
use crate::{ALPN, LinkError, keys};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const HELLO_TIMEOUT: Duration = Duration::from_secs(20);

/// Receives what the daemon sends without being asked.
pub trait MessageSink: Send + Sync {
    /// One JSON-RPC message: a notification, or a response to a request from `send`.
    fn on_message(&self, json: String);
    /// The connection ended. No more messages follow.
    fn on_closed(&self, reason: String);
}

#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    /// An `aivory://pair?...` link, or the node id of the daemon (64 hex characters).
    pub target: String,
    /// The secret key of the device (64 hex characters, see [`keys::generate_secret_hex`]).
    /// Its public key is the identity of the device on the daemon.
    pub device_secret: String,
    pub device_name: String,
    pub os: String,
    /// The pairing code. `None`: take it from the link. A paired device needs no code.
    pub code: Option<String>,
    /// The relay URL. `None`: take it from the link. `"none"`: no relay.
    pub relay: Option<String>,
    /// Direct addresses of the daemon (`ip:port`), for a LAN or a test.
    pub direct_addrs: Vec<String>,
    /// `true`: the device has no LAN token yet. The daemon sends one in the hello reply
    /// ([`HelloReply::token`]).
    pub need_token: bool,
}

/// The kind of the network path that the connection uses now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// A direct IP path (LAN or hole punched).
    Direct,
    /// Through the relay.
    Relay,
    /// No path is selected (the connection is closed, or still opens).
    None,
}

impl PathKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Relay => "relay",
            Self::None => "none",
        }
    }
}

/// The selected path of the connection, from the connection info of iroh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathInfo {
    pub kind: PathKind,
    /// The remote address of the path: `ip:<addr>` or `relay:<url>`. Empty: no path.
    pub addr: String,
    /// The round-trip time estimate of the path.
    pub rtt_ms: u64,
}

type Pending = Arc<Mutex<HashMap<u64, mpsc::Sender<String>>>>;

pub struct LinkClient {
    runtime: Option<Runtime>,
    endpoint: Endpoint,
    conn: Connection,
    send: Arc<tokio::sync::Mutex<SendStream>>,
    pending: Pending,
    next_id: AtomicU64,
    closed: Arc<AtomicBool>,
    reply: HelloReply,
}

impl fmt::Debug for LinkClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinkClient")
            .field("node", &self.reply.node)
            .field("device_id", &self.reply.device_id)
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl LinkClient {
    /// Connect, send the hello, and wait for the answer. A rejected device gives
    /// [`LinkError::Rejected`].
    pub fn connect(
        options: &ConnectOptions,
        sink: Arc<dyn MessageSink>,
    ) -> Result<Self, LinkError> {
        let secret = keys::parse_secret(&options.device_secret)?;
        let (addr, relay, code) = target(options)?;
        let hello = Hello {
            code,
            device: options.device_name.clone(),
            os: options.os.clone(),
            key: secret.public().to_string(),
            need_token: options.need_token,
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("aivory-link")
            .enable_all()
            .build()?;
        let setup = runtime.block_on(async {
            let endpoint = endpoint::bind(EndpointConfig {
                secret,
                relay,
                bind: None,
                accept: false,
            })
            .await?;
            match handshake(&endpoint, addr, &hello).await {
                Ok(parts) => Ok((endpoint, parts)),
                Err(e) => {
                    endpoint.close().await;
                    Err(e)
                }
            }
        });
        let (endpoint, (conn, send, recv, reply)) = match setup {
            Ok(parts) => parts,
            Err(e) => {
                runtime.shutdown_background();
                return Err(e);
            }
        };

        let pending = Pending::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Event>();
        // The sink runs on its own thread, so that it may call the client.
        std::thread::Builder::new()
            .name("aivory-link-sink".into())
            .spawn(move || {
                for event in rx {
                    match event {
                        Event::Message(json) => sink.on_message(json),
                        Event::Closed(reason) => sink.on_closed(reason),
                    }
                }
            })?;
        runtime.spawn(read_task(
            recv,
            Arc::clone(&pending),
            Arc::clone(&closed),
            tx,
        ));
        Ok(Self {
            runtime: Some(runtime),
            endpoint,
            conn,
            send: Arc::new(tokio::sync::Mutex::new(send)),
            pending,
            next_id: AtomicU64::new(1),
            closed,
            reply,
        })
    }

    /// The answer of the daemon to the hello: the device id, and whether this connection
    /// paired the device.
    pub fn hello_reply(&self) -> &HelloReply {
        &self.reply
    }

    /// The node id of the daemon (64 hex characters). Store it with the relay URL for later
    /// connections.
    pub fn node_id(&self) -> &str {
        &self.reply.node
    }

    /// The path that the connection uses now. A relay path can change to a direct path after
    /// hole punching, so ask again when you show it.
    pub fn path(&self) -> PathInfo {
        if self.is_closed() {
            return PathInfo::none();
        }
        let paths = self.conn.paths();
        let selected = paths.iter().find(|p| p.is_selected());
        match selected {
            Some(p) => PathInfo {
                kind: if p.is_relay() {
                    PathKind::Relay
                } else {
                    PathKind::Direct
                },
                addr: p.remote_addr().to_string(),
                rtt_ms: u64::try_from(p.rtt().as_millis()).unwrap_or(u64::MAX),
            },
            None => PathInfo::none(),
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Send one raw JSON-RPC message (a request, a notification, or a response).
    pub fn send(&self, json: &str) -> Result<(), LinkError> {
        if self.is_closed() {
            return Err(LinkError::Closed);
        }
        let send = Arc::clone(&self.send);
        let bytes = json.as_bytes();
        self.rt()
            .block_on(async move { write_frame(&mut *send.lock().await, bytes).await })
            .map_err(|_| LinkError::Closed)
    }

    /// Send a request and wait for its response. `params_json` is a JSON value (empty:
    /// `null`). The result is the complete response message (`result` or `error`).
    pub fn call(
        &self,
        method: &str,
        params_json: &str,
        timeout_ms: u64,
    ) -> Result<String, LinkError> {
        let params: Value = if params_json.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(params_json)
                .map_err(|e| LinkError::Invalid(format!("params are not JSON: {e}")))?
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let (tx, rx) = mpsc::channel();
        lock(&self.pending).insert(id, tx);
        if let Err(e) = self.send(&request.to_string()) {
            lock(&self.pending).remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
            Ok(response) => Ok(response),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                lock(&self.pending).remove(&id);
                Err(LinkError::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(LinkError::Closed),
        }
    }

    /// Close the connection. The sink gets `on_closed`.
    pub fn close(&self) {
        self.conn.close(0u32.into(), b"bye");
        self.closed.store(true, Ordering::Relaxed);
    }

    fn rt(&self) -> &Runtime {
        self.runtime.as_ref().expect("the runtime lives until drop")
    }
}

impl Drop for LinkClient {
    fn drop(&mut self) {
        self.close();
        if let Some(runtime) = self.runtime.take() {
            let endpoint = self.endpoint.clone();
            let _ =
                runtime.block_on(async { timeout(Duration::from_secs(2), endpoint.close()).await });
            runtime.shutdown_background();
        }
    }
}

impl PathInfo {
    fn none() -> Self {
        Self {
            kind: PathKind::None,
            addr: String::new(),
            rtt_ms: 0,
        }
    }
}

enum Event {
    Message(String),
    Closed(String),
}

type Handshake = (Connection, SendStream, RecvStream, HelloReply);

async fn handshake(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    hello: &Hello,
) -> Result<Handshake, LinkError> {
    let conn = timeout(CONNECT_TIMEOUT, endpoint.connect(addr, ALPN))
        .await
        .map_err(|_| LinkError::Connect("timeout".into()))?
        .map_err(|e| LinkError::Connect(e.to_string()))?;
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| LinkError::Connect(e.to_string()))?;
    let request = json!({
        "jsonrpc": "2.0", "id": 0, "method": HELLO_METHOD,
        "params": serde_json::to_value(hello).map_err(|e| LinkError::Invalid(e.to_string()))?,
    });
    write_frame(&mut send, request.to_string().as_bytes()).await?;
    let frame = timeout(HELLO_TIMEOUT, read_frame(&mut recv))
        .await
        .map_err(|_| LinkError::Connect("no answer to the hello".into()))?
        .map_err(|e| LinkError::Connect(e.to_string()))?
        .ok_or(LinkError::Closed)?;
    let response: Value = serde_json::from_slice(&frame)
        .map_err(|e| LinkError::Connect(format!("bad hello answer: {e}")))?;
    if let Some(error) = response.get("error") {
        let message = error["message"].as_str().unwrap_or("rejected");
        return Err(LinkError::Rejected(message.to_string()));
    }
    let reply: HelloReply = serde_json::from_value(response["result"].clone())
        .map_err(|e| LinkError::Connect(format!("bad hello answer: {e}")))?;
    Ok((conn, send, recv, reply))
}

async fn read_task(
    mut recv: RecvStream,
    pending: Pending,
    closed: Arc<AtomicBool>,
    events: mpsc::Sender<Event>,
) {
    let reason = loop {
        match read_frame(&mut recv).await {
            Ok(Some(bytes)) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                if let Some(text) = route(&pending, text) {
                    let _ = events.send(Event::Message(text));
                }
            }
            Ok(None) => break "the daemon closed the stream".to_string(),
            Err(e) => break e.to_string(),
        }
    };
    closed.store(true, Ordering::Relaxed);
    // Dropping the senders wakes the waiting callers.
    lock(&pending).clear();
    let _ = events.send(Event::Closed(reason));
}

/// Give a response to the caller of `call`. Return the message when it is for the sink.
fn route(pending: &Pending, text: String) -> Option<String> {
    let id = serde_json::from_str::<Value>(&text).ok().and_then(|v| {
        let is_response = v.get("method").is_none();
        is_response
            .then(|| v.get("id").and_then(Value::as_u64))
            .flatten()
    });
    let Some(id) = id else {
        return Some(text);
    };
    match lock(pending).remove(&id) {
        Some(tx) => {
            let _ = tx.send(text);
            None
        }
        None => Some(text),
    }
}

/// The address to dial, the relay choice, and the pairing code.
fn target(o: &ConnectOptions) -> Result<(EndpointAddr, RelayChoice, Option<String>), LinkError> {
    let link = if o.target.starts_with("aivory://") {
        PairLink::parse(&o.target)?
    } else {
        PairLink {
            node: Some(o.target.clone()),
            ..PairLink::default()
        }
    };
    let node = link
        .node
        .ok_or_else(|| LinkError::Invalid("the link has no node id".into()))?;
    let id = PublicKey::from_str(node.trim())
        .map_err(|_| LinkError::Invalid("the node id is not 64 hex characters".into()))?;
    let relay_text = o.relay.clone().or(link.relay);
    let relay = RelayChoice::from_setting(relay_text.as_deref());
    let mut addr = EndpointAddr::new(id);
    if let RelayChoice::Custom(url) = &relay {
        addr = addr.with_relay_url(endpoint::parse_relay(url)?);
    }
    // The addresses of the link come first; the options add to them.
    for direct in link.direct.iter().chain(&o.direct_addrs) {
        let parsed = direct
            .parse()
            .map_err(|_| LinkError::Invalid(format!("'{direct}' is not an ip:port address")))?;
        addr = addr.with_ip_addr(parsed);
    }
    Ok((addr, relay, o.code.clone().or(link.code)))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
