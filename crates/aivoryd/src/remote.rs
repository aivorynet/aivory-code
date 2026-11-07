//! `aivoryd serve --remote <addr>`: the JSON-RPC surface over WebSocket for the phone app on
//! the LAN (plan step P4d). Each text frame is one JSON-RPC message, as one line is on the
//! pipe.
//!
//! Pairing: `devices/pair` gives a code (10 minutes) and an `aivory://pair` URL for the QR
//! code. The phone connects once with `?code=<code>&device=<name>`; the daemon answers with
//! the notification `device/paired {token, device}` and keeps only the SHA-256 of the token.
//! Later connections use `?token=<token>`.
//!
//! One pairing serves both links. The phone may add `&key=<Ed25519 public key, 64 hex>` to the
//! pairing query (or to a later `?token=` query of a phone without a key): the daemon stores the
//! key for the same device, so that the phone can use iroh without a new pairing. The other
//! way round, an iroh pairing ([`Pairings::issue_token`]) gives the phone a LAN token. A device
//! that has both is one entry in the device list.
//!
//! The caller of a request is the device of its connection ([`Caller`]). `devices/push` uses it.
//!
//! The LAN transport has no TLS. The iroh link (`iroh_link.rs`, `serve --iroh`) reaches the
//! phone outside the LAN. It shares `Pairings`: the same codes, `devices.json`, and
//! `devices/pair` and `devices/remove`. An iroh device is stored by its public key.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use aivory_link::PairLink;
use aivory_link::hello::Hello;
use aivory_protocol::{Device, Pairing, PushPlatform, UnixMs};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Digest;

use crate::outbox::{OUTBOX_CAPACITY, Out, Outbox};
use crate::rpc::Daemon;

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_FRAME: u64 = 8 * 1024 * 1024;
const PAIR_MINUTES: i64 = 10;

/// The iroh link of the daemon, as the pairing link names it.
#[derive(Debug, Clone, Default)]
struct IrohInfo {
    node: String,
    relay: Option<String>,
    /// The direct addresses of the endpoint (`ip:port`).
    direct: Vec<String>,
}

/// Who sends a request: the connection decides, never a parameter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Caller {
    /// The local pipe (the desktop app, the CLI). It is not a paired phone.
    #[default]
    Local,
    /// A paired phone, by device id. The LAN link knows it from the token, the iroh link from
    /// the key of the QUIC handshake.
    Device(String),
}

thread_local! {
    static CALLER: std::cell::RefCell<Caller> = const { std::cell::RefCell::new(Caller::Local) };
}

/// Run `f` for a connection of `caller`. Each connection reads and dispatches its requests on
/// one thread, so a thread-local holds the caller without a change in the dispatch signature.
pub fn as_caller<R>(caller: Caller, f: impl FnOnce() -> R) -> R {
    let before = CALLER.with(|c| c.replace(caller));
    let result = f();
    CALLER.with(|c| *c.borrow_mut() = before);
    result
}

/// The caller of the request that this thread handles. [`Caller::Local`] outside of
/// [`as_caller`].
pub fn current_caller() -> Caller {
    CALLER.with(|c| c.borrow().clone())
}

/// Where the daemon sends the sealed push of one device (`devices/push`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushTarget {
    /// The APNs hex token or the FCM registration token.
    pub token: String,
    pub platform: PushPlatform,
    /// The X25519 public key of the device (base64url).
    pub key: String,
}

/// Paired phones and open pairing codes. Saved as JSON next to the state database.
#[derive(Debug)]
pub struct Pairings {
    file: Option<PathBuf>,
    /// The address that the QR code names (`--remote`). `None`: no LAN link.
    public: Option<SocketAddr>,
    inner: Mutex<Inner>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Inner {
    /// SHA-256 (hex) of the device token -> device.
    devices: HashMap<String, Device>,
    /// Device public key (hex) -> device. These phones connect over iroh, which proves the key.
    #[serde(default)]
    keys: HashMap<String, Device>,
    /// Device id -> push target. Only devices that registered one are in the map.
    #[serde(default)]
    push: HashMap<String, PushTarget>,
    #[serde(skip)]
    codes: HashMap<String, UnixMs>,
    /// Open iroh connections by device key.
    #[serde(skip)]
    live: HashMap<String, u32>,
    #[serde(skip)]
    iroh: Option<IrohInfo>,
}

impl Inner {
    /// Use an open pairing code. `false`: unknown or expired.
    fn take_code(&mut self, code: &str, now: UnixMs) -> bool {
        self.codes
            .remove(code)
            .is_some_and(|expires| expires >= now)
    }
}

impl Pairings {
    pub fn load(file: Option<PathBuf>, public: SocketAddr) -> Self {
        Self::load_with(file, Some(public))
    }

    /// Like [`Pairings::load`]; `public` is `None` when only the iroh link is on.
    pub fn load_with(file: Option<PathBuf>, public: Option<SocketAddr>) -> Self {
        let mut inner: Inner = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        // No connection is open after a start.
        for d in inner.devices.values_mut().chain(inner.keys.values_mut()) {
            d.online = false;
        }
        Self {
            file,
            public,
            inner: Mutex::new(inner),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The LAN address of the WebSocket link, when it is on.
    pub fn public(&self) -> Option<SocketAddr> {
        self.public
    }

    /// Name the iroh link in the pairing links: the node id and the relay URL, if known.
    pub fn set_iroh(&self, node: &str, relay: Option<&str>) {
        let mut inner = self.lock();
        let direct = inner.iroh.take().map(|i| i.direct).unwrap_or_default();
        inner.iroh = Some(IrohInfo {
            node: node.to_string(),
            relay: relay.map(str::to_string),
            direct,
        });
    }

    /// Name the direct addresses of the iroh endpoint in the pairing links (`direct=`), so that
    /// a phone reaches a daemon without a relay. Call it after [`Pairings::set_iroh`].
    pub fn set_direct(&self, addrs: &[SocketAddr]) {
        if let Some(iroh) = &mut self.lock().iroh {
            iroh.direct = addrs.iter().map(ToString::to_string).collect();
        }
    }

    /// A new pairing code for the QR code of Settings.
    pub fn pair(&self) -> Pairing {
        let code = format!("AIV-{}", random_hex(3).to_uppercase());
        let expires = now_ms() + PAIR_MINUTES * 60_000;
        let iroh = {
            let mut inner = self.lock();
            inner.codes.insert(code.clone(), expires);
            inner.iroh.clone()
        };
        let link = PairLink {
            node: iroh.as_ref().map(|i| i.node.clone()),
            direct: iroh.as_ref().map(|i| i.direct.clone()).unwrap_or_default(),
            relay: iroh.and_then(|i| i.relay),
            host: self.public.map(|a| a.to_string()),
            code: Some(code.clone()),
        };
        Pairing {
            url: link.to_url(),
            code,
            expires,
        }
    }

    /// The paired devices. A device with a token and a key is one entry.
    pub fn devices(&self) -> Vec<Device> {
        let inner = self.lock();
        let mut by_id: HashMap<&str, Device> = HashMap::new();
        for d in inner.devices.values() {
            by_id.insert(&d.id, d.clone());
        }
        for (key, d) in &inner.keys {
            let live = inner.live.get(key).is_some_and(|n| *n > 0);
            by_id
                .entry(&d.id)
                .and_modify(|e| {
                    e.online |= live;
                    e.last_seen = e.last_seen.max(d.last_seen);
                })
                .or_insert_with(|| Device {
                    online: live,
                    ..d.clone()
                });
        }
        let mut list: Vec<Device> = by_id.into_values().collect();
        list.sort_by(|a, b| a.paired.cmp(&b.paired).then_with(|| a.id.cmp(&b.id)));
        list
    }

    pub fn remove(&self, id: &str) -> bool {
        let removed = {
            let mut inner = self.lock();
            let before = inner.devices.len() + inner.keys.len();
            inner.devices.retain(|_, d| d.id != id);
            inner.keys.retain(|_, d| d.id != id);
            inner.push.remove(id);
            inner.devices.len() + inner.keys.len() != before
        };
        self.save();
        removed
    }

    /// Save the push target of a paired device. `false`: no such device.
    pub fn set_push(&self, id: &str, target: PushTarget) -> bool {
        let known = {
            let mut inner = self.lock();
            let known = inner
                .devices
                .values()
                .chain(inner.keys.values())
                .any(|d| d.id == id);
            if known {
                inner.push.insert(id.to_string(), target);
            }
            known
        };
        if known {
            self.save();
        }
        known
    }

    /// The push targets of the paired devices, by device id.
    pub fn push_targets(&self) -> Vec<(String, PushTarget)> {
        let mut list: Vec<_> = self
            .lock()
            .push
            .iter()
            .map(|(id, t)| (id.clone(), t.clone()))
            .collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }

    /// Drop the push target of a device (the gateway said the token is not valid). The device
    /// stays paired. The target goes only when it still has this `token`: the phone may have
    /// registered a new one meanwhile. `false`: nothing dropped.
    pub fn drop_push(&self, id: &str, token: &str) -> bool {
        let dropped = {
            let mut inner = self.lock();
            if inner.push.get(id).is_some_and(|t| t.token == token) {
                inner.push.remove(id).is_some()
            } else {
                false
            }
        };
        if dropped {
            self.save();
        }
        dropped
    }

    /// Store the device key of a device that paired over the LAN, so that it can use iroh too.
    /// `false`: the key is not valid, or it belongs to another device.
    fn register_key(&self, device: &Device, key: &str) -> bool {
        let key = key.trim().to_ascii_lowercase();
        if key.len() != 64 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
        {
            let mut inner = self.lock();
            if inner.keys.get(&key).is_some_and(|d| d.id != device.id) {
                return false;
            }
            // One key for each device: a new key replaces the old one.
            inner.keys.retain(|_, d| d.id != device.id);
            inner.keys.insert(key, device.clone());
        }
        self.save();
        true
    }

    /// Give a paired device a new LAN token (it replaces an older one). The phone sends it as
    /// `?token=` on the WebSocket link. `None`: no such device.
    pub fn issue_token(&self, id: &str) -> Option<String> {
        let token = random_hex(32);
        {
            let mut inner = self.lock();
            let device = inner
                .devices
                .values()
                .chain(inner.keys.values())
                .find(|d| d.id == id)?
                .clone();
            inner.devices.retain(|_, d| d.id != id);
            inner.devices.insert(
                hash(&token),
                Device {
                    online: false,
                    ..device
                },
            );
        }
        self.save();
        Some(token)
    }

    /// Check an iroh connection. `peer` is the key that the QUIC handshake proved (hex). A
    /// known key is admitted. An unknown key needs an open pairing code and becomes a new
    /// device. The second value is `true` for a new device.
    pub fn admit_key(&self, peer: &str, hello: &Hello) -> Result<(Device, bool), String> {
        if hello.key != peer {
            return Err("the device key does not match the connection".into());
        }
        let now = now_ms();
        let (device, new) = {
            let mut inner = self.lock();
            if let Some(device) = inner.keys.get_mut(peer) {
                device.last_seen = now;
                (device.clone(), false)
            } else {
                let code = hello.code.as_deref().unwrap_or_default();
                if !inner.take_code(code, now) {
                    return Err("unknown device: pair it with a valid code".into());
                }
                let device = Device {
                    id: format!("d{}", random_hex(4)),
                    name: if hello.device.is_empty() {
                        "Phone".into()
                    } else {
                        hello.device.clone()
                    },
                    os: hello.os.clone(),
                    paired: now,
                    last_seen: now,
                    online: true,
                };
                inner.keys.insert(peer.to_string(), device.clone());
                (device, true)
            }
        };
        if new {
            self.save();
        }
        *self.lock().live.entry(peer.to_string()).or_default() += 1;
        Ok((device, new))
    }

    /// `false` once the device with this key is removed. The iroh link closes such a
    /// connection.
    pub fn key_allowed(&self, peer: &str) -> bool {
        self.lock().keys.contains_key(peer)
    }

    /// An iroh connection of the device ended.
    pub fn key_offline(&self, peer: &str) {
        let mut inner = self.lock();
        if let Some(n) = inner.live.get_mut(peer) {
            *n = n.saturating_sub(1);
        }
        if let Some(d) = inner.keys.get_mut(peer) {
            d.last_seen = now_ms();
        }
    }

    /// Check a connection: a known token, or an open code that becomes a new device (its
    /// token comes back once).
    fn admit(&self, query: &HashMap<String, String>) -> Option<(Device, Option<String>)> {
        let now = now_ms();
        if let Some(token) = query.get("token") {
            let device = {
                let mut inner = self.lock();
                let device = inner.devices.get_mut(&hash(token))?;
                device.last_seen = now;
                device.online = true;
                device.clone()
            };
            // A phone that paired before it had a key can add it now.
            if let Some(key) = query.get("key") {
                self.register_key(&device, key);
            }
            return Some((device, None));
        }
        let code = query.get("code")?;
        let token = {
            let mut inner = self.lock();
            if !inner.take_code(code, now) {
                return None;
            }
            let token = random_hex(32);
            let device = Device {
                id: format!("d{}", random_hex(4)),
                name: query
                    .get("device")
                    .cloned()
                    .unwrap_or_else(|| "Phone".into()),
                os: query.get("os").cloned().unwrap_or_default(),
                paired: now,
                last_seen: now,
                online: true,
            };
            inner.devices.insert(hash(&token), device);
            token
        };
        self.save();
        let device = self.lock().devices.get(&hash(&token)).cloned()?;
        if let Some(key) = query.get("key") {
            self.register_key(&device, key);
        }
        Some((device, Some(token)))
    }

    fn offline(&self, id: &str) {
        if let Some(d) = self.lock().devices.values_mut().find(|d| d.id == id) {
            d.online = false;
        }
        self.save();
    }

    fn save(&self) {
        let Some(file) = &self.file else {
            return;
        };
        let text = serde_json::to_string_pretty(&*self.lock()).unwrap_or_default();
        if let Err(e) = std::fs::write(file, text) {
            eprintln!("aivoryd: cannot save the paired devices: {e}");
        }
    }
}

/// Listen on `addr` and serve each WebSocket connection on its own thread.
pub fn serve(daemon: Arc<Daemon>, pairings: Arc<Pairings>, addr: SocketAddr) -> io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    eprintln!(
        "aivoryd remote endpoint on ws://{}",
        pairings.public.unwrap_or(addr)
    );
    thread::Builder::new()
        .name("aivoryd-remote".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let daemon = Arc::clone(&daemon);
                let pairings = Arc::clone(&pairings);
                thread::spawn(move || {
                    if let Err(e) = connection(&daemon, &pairings, stream) {
                        eprintln!("aivoryd: remote connection ended: {e}");
                    }
                });
            }
        })?;
    Ok(())
}

/// The LAN address of this machine with the port of `bind` (for the QR code).
pub fn public_addr(bind: SocketAddr) -> SocketAddr {
    if !bind.ip().is_unspecified() {
        return bind;
    }
    SocketAddr::new(lan_ip().unwrap_or(bind.ip()), bind.port())
}

/// The LAN address of this machine (IPv4), when it has one.
pub fn lan_ip() -> Option<std::net::IpAddr> {
    // A UDP "connect" picks the outgoing interface; it sends nothing.
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|()| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip())
        .ok()
        .filter(|ip| !ip.is_unspecified())
}

fn connection(daemon: &Daemon, pairings: &Pairings, stream: TcpStream) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let request = read_request(&mut reader)?;
    let Some(key) = request.headers.get("sec-websocket-key") else {
        writer.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    };
    let Some((device, token)) = pairings.admit(&request.query) else {
        writer.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    };
    write!(
        writer,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(key)
    )?;
    writer.flush()?;

    let (outbox, rx) = Outbox::new(OUTBOX_CAPACITY);
    let frames = writer.try_clone()?;
    let writer_thread = thread::spawn(move || write_loop(frames, &rx));
    if let Some(token) = token {
        outbox.send(json!({
            "jsonrpc": "2.0",
            "method": "device/paired",
            "params": { "token": token, "device": device },
        }))?;
    }
    let caller = Caller::Device(device.id.clone());
    let result = as_caller(caller, || {
        read_loop(daemon, &mut reader, &mut writer, &outbox)
    });
    outbox.close();
    drop(outbox);
    let _ = writer_thread.join();
    pairings.offline(&device.id);
    result
}

fn read_loop(
    daemon: &Daemon,
    reader: &mut impl Read,
    control: &mut TcpStream,
    outbox: &Outbox,
) -> io::Result<()> {
    let mut message = Vec::new();
    loop {
        let frame = match read_frame(reader) {
            Ok(frame) => frame,
            // The phone dropped the socket without a close frame (sleep, network change).
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        match frame.opcode {
            0x0 | 0x1 => {
                message.extend_from_slice(&frame.payload);
                if message.len() as u64 > MAX_FRAME {
                    return Err(io::Error::other("message too large"));
                }
                if !frame.fin {
                    continue;
                }
                let text = String::from_utf8_lossy(&message).into_owned();
                message.clear();
                if let Some(response) = daemon.handle_line(&text, outbox) {
                    outbox.send(response)?;
                }
            }
            0x8 => {
                // Close: echo it and stop. The writer thread may still flush queued frames.
                let _ = control.write_all(&encode_frame(0x8, &[]));
                return Ok(());
            }
            0x9 => {
                let _ = control.write_all(&encode_frame(0xA, &frame.payload));
            }
            _ => {}
        }
    }
}

fn write_loop(mut stream: TcpStream, rx: &Receiver<Out>) -> io::Result<()> {
    for out in rx {
        match out {
            Out::Message(v) => {
                let bytes = serde_json::to_vec(&v).map_err(io::Error::other)?;
                stream.write_all(&encode_frame(0x1, &bytes))?;
            }
            Out::Close => break,
        }
    }
    Ok(())
}

#[derive(Debug)]
struct Request {
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
}

fn read_request(reader: &mut impl BufRead) -> io::Result<Request> {
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let target = first.split_whitespace().nth(1).unwrap_or("/");
    let query = target
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), percent_decode(v)))
        .collect();
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
        if headers.len() > 64 {
            break;
        }
    }
    Ok(Request { query, headers })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(b) => {
                    out.push(b);
                    i += 3;
                    continue;
                }
                Err(_) => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

fn read_frame(r: &mut impl Read) -> io::Result<Frame> {
    let mut head = [0u8; 2];
    r.read_exact(&mut head)?;
    let fin = head[0] & 0x80 != 0;
    let opcode = head[0] & 0x0F;
    let masked = head[1] & 0x80 != 0;
    let len = match head[1] & 0x7F {
        126 => {
            let mut b = [0u8; 2];
            r.read_exact(&mut b)?;
            u64::from(u16::from_be_bytes(b))
        }
        127 => {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            u64::from_be_bytes(b)
        }
        n => u64::from(n),
    };
    if len > MAX_FRAME {
        return Err(io::Error::other("frame too large"));
    }
    let mut mask = [0u8; 4];
    if masked {
        r.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(Frame {
        fin,
        opcode,
        payload,
    })
}

/// A server frame (not masked), FIN set.
fn encode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 10);
    out.push(0x80 | opcode);
    match payload.len() {
        n if n < 126 => out.push(n as u8),
        n if n <= u16::MAX as usize => {
            out.push(126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(payload);
    out
}

fn accept_key(key: &str) -> String {
    let mut sha = sha1_smol::Sha1::new();
    sha.update(key.trim().as_bytes());
    sha.update(GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(sha.digest().bytes())
}

fn hash(token: &str) -> String {
    hex(&sha2::Sha256::digest(token.as_bytes()))
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the OS random source works");
    hex(&buf)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_ms() -> UnixMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as UnixMs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_key_matches_rfc_6455() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn frames_round_trip_with_a_client_mask() {
        for len in [5usize, 300, 70_000] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let mut frame = encode_frame(0x1, &payload);
            // Turn it into a masked client frame.
            let mask = [1u8, 2, 3, 4];
            let header_len = frame.len() - payload.len();
            frame[1] |= 0x80;
            let mut client = frame[..header_len].to_vec();
            client.extend_from_slice(&mask);
            client.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
            let back = read_frame(&mut &client[..]).unwrap();
            assert!(back.fin);
            assert_eq!(back.opcode, 0x1);
            assert_eq!(back.payload, payload);
        }
    }

    #[test]
    fn a_code_admits_once_and_the_token_admits_later() {
        let p = Pairings::load(None, "127.0.0.1:7420".parse().unwrap());
        let pairing = p.pair();
        assert!(pairing.url.contains(&pairing.code));
        let mut q = HashMap::new();
        q.insert("code".to_string(), pairing.code.clone());
        q.insert("device".to_string(), "Pixel".to_string());
        let (device, token) = p.admit(&q).expect("the code admits");
        let token = token.expect("a new device gets a token");
        assert_eq!(device.name, "Pixel");
        assert!(p.admit(&q).is_none(), "a code works once");
        let mut t = HashMap::new();
        t.insert("token".to_string(), token);
        assert_eq!(p.admit(&t).unwrap().0.id, device.id);
        assert!(p.remove(&device.id));
        assert!(p.admit(&t).is_none());
    }

    #[test]
    fn a_device_key_admits_with_a_code_and_later_without() {
        let p = Pairings::load_with(None, None);
        let hello = |key: &str, code: Option<&str>| Hello {
            code: code.map(str::to_string),
            device: "Pixel".into(),
            os: "android".into(),
            key: key.into(),
            need_token: false,
        };
        // No LAN host without --remote; the link names the node once set.
        p.set_iroh("nodeid", Some("https://relay.example.com/"));
        let pairing = p.pair();
        let link = PairLink::parse(&pairing.url).unwrap();
        assert_eq!(link.node.as_deref(), Some("nodeid"));
        assert_eq!(link.relay.as_deref(), Some("https://relay.example.com/"));
        assert!(link.host.is_none());
        // The key in the hello must be the key of the connection.
        assert!(
            p.admit_key("aa", &hello("bb", Some(&pairing.code)))
                .is_err()
        );
        // The failed try did not use the code.
        let (device, new) = p
            .admit_key("aa", &hello("aa", Some(&pairing.code)))
            .unwrap();
        assert!(new);
        assert!(
            p.admit_key("cc", &hello("cc", Some(&pairing.code)))
                .is_err()
        );
        let (again, new) = p.admit_key("aa", &hello("aa", None)).unwrap();
        assert!(!new);
        assert_eq!(again.id, device.id);
        assert!(p.devices()[0].online);
        p.key_offline("aa");
        p.key_offline("aa");
        assert!(!p.devices()[0].online);
        assert!(p.remove(&device.id));
        assert!(!p.key_allowed("aa"));
        assert!(p.admit_key("aa", &hello("aa", None)).is_err());
    }

    #[test]
    fn a_device_with_a_token_and_a_key_is_one_entry() {
        let p = Pairings::load_with(None, Some("127.0.0.1:7420".parse().unwrap()));
        let key = "ab".repeat(32);
        let mut q = HashMap::new();
        q.insert("code".to_string(), p.pair().code);
        q.insert("key".to_string(), key.to_uppercase());
        let (device, token) = p.admit(&q).expect("the code admits");
        assert!(token.is_some());
        assert!(p.key_allowed(&key), "the key is stored in lower case");
        assert_eq!(p.devices().len(), 1);
        // The key joins the device of the token: the iroh hello is not a new device.
        let hello = Hello {
            code: None,
            device: String::new(),
            os: String::new(),
            key: key.clone(),
            need_token: false,
        };
        let (again, new) = p.admit_key(&key, &hello).unwrap();
        assert!(!new);
        assert_eq!(again.id, device.id);
        assert!(p.devices()[0].online);
        // A new token replaces the old one.
        let old = token.unwrap();
        let fresh = p.issue_token(&device.id).unwrap();
        let mut t = HashMap::new();
        t.insert("token".to_string(), old);
        assert!(p.admit(&t).is_none());
        t.insert("token".to_string(), fresh);
        assert_eq!(p.admit(&t).unwrap().0.id, device.id);
        assert_eq!(p.devices().len(), 1);
        assert!(p.issue_token("dnone").is_none());
        // Bad keys and the key of another device are refused.
        assert!(!p.register_key(&device, "xyz"));
        let other = Device {
            id: "dother".into(),
            ..device.clone()
        };
        assert!(!p.register_key(&other, &key));
        // The device goes with both its entries.
        assert!(p.remove(&device.id));
        assert!(p.devices().is_empty());
        assert!(!p.key_allowed(&key));
    }

    #[test]
    fn the_caller_is_local_unless_a_connection_sets_it() {
        assert_eq!(current_caller(), Caller::Local);
        let inside = as_caller(Caller::Device("d1".into()), current_caller);
        assert_eq!(inside, Caller::Device("d1".into()));
        assert_eq!(current_caller(), Caller::Local);
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("My%20Phone+X"), "My Phone X");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
