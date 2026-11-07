//! `aivoryd serve --iroh` in one process, with relays off and direct localhost addresses:
//! pair with a code, read the war room, reconnect with the stored key, reject a removed or
//! unknown device. One pairing serves the LAN WebSocket link and iroh; `devices/push` binds to
//! the calling connection; a pairing link names the direct addresses; the client names its
//! path.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aivory_link::endpoint::RelayChoice;
use aivory_link::{ConnectOptions, LinkClient, LinkError, MessageSink, PairLink, PathKind, keys};
use aivory_protocol::WarRoom;
use aivory_sessions::StoreRoots;
use aivoryd::iroh_link::{IrohLink, IrohOptions};
use aivoryd::remote::Pairings;
use aivoryd::{Config, Daemon};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);

fn config() -> Config {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../aivory-sessions/tests/fixtures");
    Config {
        roots: StoreRoots {
            claude: base.join("claude"),
            codex: base.join("codex"),
            gemini: base.join("gemini"),
        },
        cache_file: None,
    }
}

/// A daemon on the demo data with the iroh link on localhost.
struct Fixture {
    dir: PathBuf,
    pairings: Arc<Pairings>,
    link: IrohLink,
}

impl Fixture {
    fn start(name: &str) -> Self {
        Self::start_with(name, None)
    }

    /// With the LAN WebSocket link on a free localhost port.
    fn start_lan(name: &str) -> (Self, SocketAddr) {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        (Self::start_with(name, Some(addr)), addr)
    }

    fn start_with(name: &str, lan: Option<SocketAddr>) -> Self {
        let dir = std::env::temp_dir().join(format!("aivory-iroh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pairings = Arc::new(Pairings::load_with(Some(dir.join("devices.json")), lan));
        let daemon = Arc::new(Daemon::with_demo(config()).with_pairings(Arc::clone(&pairings)));
        if let Some(addr) = lan {
            aivoryd::remote::serve(Arc::clone(&daemon), Arc::clone(&pairings), addr)
                .expect("the LAN link starts");
        }
        let link = IrohLink::start(
            daemon,
            Arc::clone(&pairings),
            IrohOptions {
                secret_file: Some(dir.join("iroh.key")),
                relay: RelayChoice::Disabled,
                bind: Some("127.0.0.1:0".parse().unwrap()),
            },
        )
        .expect("the iroh link starts");
        Self {
            dir,
            pairings,
            link,
        }
    }

    fn options(&self, target: &str, secret: &str, code: Option<String>) -> ConnectOptions {
        ConnectOptions {
            target: target.to_string(),
            device_secret: secret.to_string(),
            device_name: "Test phone".into(),
            os: "android".into(),
            code,
            relay: Some("none".into()),
            direct_addrs: self
                .link
                .direct_addrs()
                .iter()
                .map(ToString::to_string)
                .collect(),
            need_token: false,
        }
    }

    fn connect(&self, target: &str, secret: &str, code: Option<String>) -> Connected {
        let (tx, messages) = mpsc::channel();
        let (closed_tx, closed) = mpsc::channel();
        let sink = Arc::new(Collect {
            messages: Mutex::new(tx),
            closed: Mutex::new(closed_tx),
        });
        let client = LinkClient::connect(&self.options(target, secret, code), sink);
        Connected {
            client,
            messages,
            closed,
        }
    }

    fn pair_url(&self) -> String {
        self.pairings.pair().url
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Collect {
    messages: Mutex<Sender<String>>,
    closed: Mutex<Sender<String>>,
}

impl MessageSink for Collect {
    fn on_message(&self, json: String) {
        let _ = self.messages.lock().unwrap().send(json);
    }
    fn on_closed(&self, reason: String) {
        let _ = self.closed.lock().unwrap().send(reason);
    }
}

struct Connected {
    client: Result<LinkClient, LinkError>,
    messages: Receiver<String>,
    closed: Receiver<String>,
}

impl Connected {
    fn ok(&self) -> &LinkClient {
        self.client.as_ref().expect("the connection is accepted")
    }
    fn rejection(&self) -> String {
        match &self.client {
            Err(LinkError::Rejected(message)) => message.clone(),
            other => panic!("expected a rejection, got {other:?}"),
        }
    }
}

fn call(c: &Connected, method: &str, params: Value) -> Value {
    let text = c
        .ok()
        .call(method, &params.to_string(), 20_000)
        .expect("the daemon answers");
    serde_json::from_str(&text).unwrap()
}

#[test]
fn pair_read_reconnect_and_remove() {
    let fx = Fixture::start("flow");
    let secret = keys::generate_secret_hex();
    let public = keys::public_hex(&secret).unwrap();

    // The pairing link names the node and the code (no relay and no LAN host here).
    let url = fx.pair_url();
    let link = PairLink::parse(&url).unwrap();
    assert_eq!(link.node.as_deref(), Some(fx.link.node_id()));
    assert!(link.relay.is_none() && link.host.is_none());
    assert!(link.code.unwrap().starts_with("AIV-"));

    // First connect: the code pairs the device.
    let first = fx.connect(&url, &secret, None);
    let reply = first.ok().hello_reply().clone();
    assert!(reply.newly_paired);
    assert_eq!(reply.node, fx.link.node_id());
    assert_eq!(first.ok().node_id(), fx.link.node_id());

    let response = call(&first, "warroom/snapshot", json!({}));
    let snapshot: WarRoom = serde_json::from_value(response["result"].clone()).unwrap();
    assert!(!snapshot.sessions.is_empty(), "the demo has sessions");

    // The device list shows the phone; devices/pair works over the link.
    let settings = call(&first, "settings/get", json!({}));
    assert!(settings.get("result").is_some(), "{settings}");
    let pair = call(&first, "devices/pair", json!({}));
    assert!(pair["result"]["url"].as_str().unwrap().contains("node="));
    let devices = fx.pairings.devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Test phone");
    assert!(devices[0].online);

    // A message that goes through `send` and a frame with raw line breaks reach the sink.
    first
        .ok()
        .send("{\n \"jsonrpc\": \"2.0\",\n \"id\": 77,\n \"method\": \"daemon/info\"\n}")
        .unwrap();
    let echoed = first.messages.recv_timeout(WAIT).expect("a response");
    assert!(echoed.contains("\"id\":77"), "{echoed}");
    drop(first);

    // Reconnect with the stored key and the node id only: no code, not new.
    let again = fx.connect(&link_node(&url), &secret, None);
    assert!(!again.ok().hello_reply().newly_paired);
    assert_eq!(fx.pairings.devices().len(), 1);
    assert!(fx.pairings.key_allowed(&public));
    let id = again.ok().hello_reply().device_id.clone();

    // `devices/remove` closes the open connection and the device cannot return.
    let removed = call(&again, "devices/remove", json!({ "id": id }));
    assert_eq!(removed["result"]["ok"], true, "{removed}");
    again
        .closed
        .recv_timeout(WAIT)
        .expect("the daemon closes the connection of a removed device");
    drop(again);
    let denied = fx.connect(&link_node(&url), &secret, None);
    assert!(denied.rejection().contains("unknown device"));
    assert!(fx.pairings.devices().is_empty());
}

fn link_node(url: &str) -> String {
    PairLink::parse(url).unwrap().node.unwrap()
}

#[test]
fn unknown_devices_and_bad_codes_are_rejected() {
    let fx = Fixture::start("reject");
    let node = fx.link.node_id().to_string();
    let secret = keys::generate_secret_hex();

    // No code.
    assert!(
        fx.connect(&node, &secret, None)
            .rejection()
            .contains("unknown device")
    );
    // A wrong code.
    let wrong = fx.connect(&node, &secret, Some("AIV-000000".into()));
    assert!(wrong.rejection().contains("unknown device"));
    // A code works once: a second device with the same code fails.
    let url = fx.pair_url();
    let code = PairLink::parse(&url).unwrap().code;
    assert!(fx.connect(&node, &secret, code.clone()).client.is_ok());
    let other = keys::generate_secret_hex();
    assert!(fx.connect(&node, &other, code).client.is_err());
    assert_eq!(fx.pairings.devices().len(), 1);
}

// ---- one pairing for both links ----------------------------------------------------------

/// A minimal WebSocket client of the LAN link (RFC 6455, text frames only).
struct Ws {
    stream: TcpStream,
    id: u64,
}

impl Ws {
    /// Connect with a query. `Err(status line)` when the daemon refuses.
    fn connect(addr: SocketAddr, query: &str) -> Result<Self, String> {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        write!(
            stream,
            "GET /?{query} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).map_err(|e| e.to_string())?;
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        if head.starts_with("HTTP/1.1 101") {
            Ok(Self { stream, id: 0 })
        } else {
            Err(head.lines().next().unwrap_or_default().to_string())
        }
    }

    fn send(&mut self, text: &str) {
        let payload = text.as_bytes();
        let mask = [7u8, 8, 9, 10];
        let mut frame = vec![0x81];
        match payload.len() {
            n if n < 126 => frame.push(0x80 | n as u8),
            n => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
            }
        }
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.stream.write_all(&frame).unwrap();
    }

    fn recv(&mut self) -> Value {
        let mut head = [0u8; 2];
        self.stream.read_exact(&mut head).unwrap();
        let len = match head[1] & 0x7F {
            126 => {
                let mut b = [0u8; 2];
                self.stream.read_exact(&mut b).unwrap();
                usize::from(u16::from_be_bytes(b))
            }
            127 => {
                let mut b = [0u8; 8];
                self.stream.read_exact(&mut b).unwrap();
                u64::from_be_bytes(b) as usize
            }
            n => usize::from(n),
        };
        let mut payload = vec![0u8; len];
        self.stream.read_exact(&mut payload).unwrap();
        serde_json::from_slice(&payload).unwrap()
    }

    /// Send a request and return its response (other messages are skipped).
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let id = self.id;
        self.send(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string(),
        );
        loop {
            let message = self.recv();
            if message["id"] == json!(id) {
                return message;
            }
        }
    }
}

fn push_params() -> Value {
    let (_, public) = aivory_link::push::generate_keypair();
    json!({ "token": "fcm-token", "platform": "fcm", "pushKey": aivory_link::push::to_base64url(&public) })
}

#[test]
fn one_pairing_serves_the_lan_link_and_iroh() {
    let (fx, lan) = Fixture::start_lan("both");

    // The pairing link names everything a phone needs, on both links.
    let url = fx.pair_url();
    let link = PairLink::parse(&url).unwrap();
    assert_eq!(link.host.as_deref(), Some(lan.to_string().as_str()));
    assert_eq!(link.node.as_deref(), Some(fx.link.node_id()));
    assert!(!link.direct.is_empty(), "{url}");

    // 1. Pair over the LAN with the device key; the token comes back once.
    let secret = keys::generate_secret_hex();
    let public = keys::public_hex(&secret).unwrap();
    let code = link.code.clone().unwrap();
    let mut ws = Ws::connect(lan, &format!("code={code}&device=Pixel&key={public}")).unwrap();
    let paired = ws.recv();
    assert_eq!(paired["method"], "device/paired");
    let token = paired["params"]["token"].as_str().unwrap().to_string();
    let id = paired["params"]["device"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(fx.pairings.key_allowed(&public));

    // 2. The same phone connects over iroh with no code: not new, same device.
    let iroh = fx.connect(&link_node(&url), &secret, None);
    let reply = iroh.ok().hello_reply().clone();
    assert!(
        !reply.newly_paired,
        "the LAN pairing already registered the key"
    );
    assert_eq!(reply.device_id, id);
    assert!(reply.token.is_none(), "no token unless the phone needs one");
    assert_eq!(fx.pairings.devices().len(), 1, "one device, two links");
    assert!(fx.pairings.devices()[0].online);

    // 3. `devices/push` binds to the caller on both links; the id of another device fails.
    let r = call(&iroh, "devices/push", push_params());
    assert_eq!(r["result"]["ok"], true, "{r}");
    let mut other = push_params();
    other["id"] = json!("dother");
    let r = call(&iroh, "devices/push", other.clone());
    assert_eq!(r["error"]["code"], -32602, "{r}");
    let r = ws.call("devices/push", other);
    assert_eq!(r["error"]["code"], -32602, "{r}");
    let r = ws.call("devices/push", push_params());
    assert_eq!(r["result"]["ok"], true, "{r}");
    let targets = fx.pairings.push_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].0, id);
    drop(ws);
    drop(iroh);

    // 4. The token of the LAN pairing works later, without the code.
    let mut again = Ws::connect(lan, &format!("token={token}")).unwrap();
    assert_eq!(
        again.call("daemon/info", json!({}))["result"]["name"],
        "aivoryd"
    );
    drop(again);

    // 5. An iroh pairing gets a LAN token when the phone asks for one.
    let secret2 = keys::generate_secret_hex();
    let code2 = PairLink::parse(&fx.pair_url()).unwrap().code;
    let mut options = fx.options(&link_node(&url), &secret2, code2);
    options.need_token = true;
    let (tx, _messages) = mpsc::channel();
    let (closed_tx, _closed) = mpsc::channel();
    let sink = Arc::new(Collect {
        messages: Mutex::new(tx),
        closed: Mutex::new(closed_tx),
    });
    let second = LinkClient::connect(&options, sink).expect("the second phone pairs");
    let reply2 = second.hello_reply().clone();
    assert!(reply2.newly_paired);
    let token2 = reply2.token.expect("an iroh pairing issues the LAN token");
    let mut lan2 = Ws::connect(lan, &format!("token={token2}")).unwrap();
    // It is the same device on the LAN: its push target binds to the device of the iroh pairing.
    let r = lan2.call("devices/push", push_params());
    assert_eq!(r["result"]["ok"], true, "{r}");
    assert!(
        fx.pairings
            .push_targets()
            .iter()
            .any(|(device, _)| *device == reply2.device_id)
    );
    assert_eq!(
        fx.pairings.devices().len(),
        2,
        "two phones, not four entries"
    );
    // A made-up token is refused.
    assert!(Ws::connect(lan, "token=nope").is_err());
}

#[test]
fn a_lan_phone_without_a_key_adds_it_with_its_token() {
    let (fx, lan) = Fixture::start_lan("late-key");
    let url = fx.pair_url();
    let code = PairLink::parse(&url).unwrap().code.unwrap();
    // An old phone: no key in the pairing query.
    let mut ws = Ws::connect(lan, &format!("code={code}&device=Old")).unwrap();
    let token = ws.recv()["params"]["token"].as_str().unwrap().to_string();
    drop(ws);
    let secret = keys::generate_secret_hex();
    let public = keys::public_hex(&secret).unwrap();
    assert!(!fx.pairings.key_allowed(&public));
    // It reconnects with its token and the new key.
    let ws = Ws::connect(lan, &format!("token={token}&key={public}")).unwrap();
    drop(ws);
    assert!(fx.pairings.key_allowed(&public));
    let iroh = fx.connect(&link_node(&url), &secret, None);
    assert!(!iroh.ok().hello_reply().newly_paired);
    assert_eq!(fx.pairings.devices().len(), 1);
    // A bad key is ignored and does not break the LAN link.
    let ws = Ws::connect(lan, &format!("token={token}&key=zz")).unwrap();
    drop(ws);
    assert!(fx.pairings.key_allowed(&public));
}

#[test]
fn the_pair_link_names_direct_addresses_and_the_path_is_direct() {
    let fx = Fixture::start("direct");
    let url = fx.pair_url();
    let link = PairLink::parse(&url).unwrap();
    assert!(link.relay.is_none(), "this daemon has no relay");
    let direct: Vec<String> = fx
        .link
        .dialable_addrs()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(!direct.is_empty());
    assert_eq!(link.direct, direct);

    // The phone has the link and no other address: it reaches the daemon through `direct=`.
    let secret = keys::generate_secret_hex();
    let mut options = fx.options(&url, &secret, None);
    options.direct_addrs.clear();
    let (tx, _messages) = mpsc::channel();
    let (closed_tx, _closed) = mpsc::channel();
    let sink = Arc::new(Collect {
        messages: Mutex::new(tx),
        closed: Mutex::new(closed_tx),
    });
    let client = LinkClient::connect(&options, sink).expect("the direct address works");
    assert!(client.hello_reply().newly_paired);

    // The path getter reports the selected path: direct, on localhost.
    let started = std::time::Instant::now();
    let mut path = client.path();
    while path.kind != PathKind::Direct && started.elapsed() < WAIT {
        std::thread::sleep(Duration::from_millis(100));
        path = client.path();
    }
    assert_eq!(path.kind, PathKind::Direct, "{path:?}");
    assert!(path.addr.starts_with("ip:127.0.0.1:"), "{path:?}");
    assert_eq!(path.kind.as_str(), "direct");
    client.close();
    assert_eq!(client.path().kind, PathKind::None);
}
