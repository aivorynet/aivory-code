//! The push worker over a fake war-room feed and a fake sender, the `devices/push` RPC, and
//! the HTTPS sender against a local gateway stand-in.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use aivory_link::hello::Hello;
use aivory_link::push as sealed;
use aivory_protocol::demo::Demo;
use aivory_protocol::rpc::{Delta, DeltaNote, Subscription};
use aivory_protocol::{
    AskKind, InboxItem, InboxKind, InboxList, Pane, SessionState, Settings, SettingsView, WarRoom,
};
use aivory_sessions::StoreRoots;
use aivoryd::outbox::Outbox;
use aivoryd::push::{HttpSender, Options, Outcome, PushService, Sender, Source};
use aivoryd::remote::{Caller, Pairings, PushTarget, as_caller};
use aivoryd::{Config, Daemon};
use serde_json::{Value, json};

const NOON: u32 = 12 * 60;
const LATE: u32 = 23 * 60 + 30;

fn noon() -> u32 {
    NOON
}

fn late() -> u32 {
    LATE
}

// ---- fakes -------------------------------------------------------------------------------

/// A war-room feed that the test drives.
struct Feed {
    view: Mutex<SettingsView>,
    inbox: Mutex<Vec<InboxItem>>,
    war_room: WarRoom,
    out: Mutex<Option<Outbox>>,
    seq: AtomicUsize,
    /// Reads of `settings/get` and `inbox/list`: the test waits on them to order its changes.
    settings_reads: AtomicUsize,
    inbox_reads: AtomicUsize,
}

impl Feed {
    fn new() -> Arc<Self> {
        let demo = Demo::new();
        Arc::new(Self {
            view: Mutex::new(demo.settings),
            inbox: Mutex::new(demo.inbox.items),
            war_room: demo.war_room,
            out: Mutex::new(None),
            seq: AtomicUsize::new(0),
            settings_reads: AtomicUsize::new(0),
            inbox_reads: AtomicUsize::new(0),
        })
    }

    fn settings(&self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.view.lock().unwrap().settings);
    }

    fn wait_subscribed(&self) {
        let end = Instant::now() + Duration::from_secs(5);
        while self.out.lock().unwrap().is_none() {
            assert!(Instant::now() < end, "the worker did not subscribe");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_reads(counter: &AtomicUsize, n: usize) {
        let end = Instant::now() + Duration::from_secs(5);
        while counter.load(Ordering::SeqCst) < n {
            assert!(Instant::now() < end, "the worker did not read {n} times");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn publish(&self, delta: Delta) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) as u64 + 1;
        let note = DeltaNote { seq, delta };
        let message = json!({ "jsonrpc": "2.0", "method": "warroom/delta", "params": note });
        let out = self.out.lock().unwrap();
        assert!(out.as_ref().expect("subscribed").try_send(message));
    }

    /// A pane of the snapshot in the given state. A pane that asks gets the ask of `like`.
    fn pane(&self, id: &str, state: SessionState, ask: Option<AskKind>) -> Pane {
        let mut pane = self.war_room.session(id).expect("a demo session").clone();
        pane.state = state;
        pane.ask = ask.map(|kind| {
            let mut a = self
                .war_room
                .sessions
                .iter()
                .find_map(|p| p.ask.clone())
                .expect("a demo ask");
            a.kind = kind;
            a.id = format!("{id}-{kind:?}");
            a
        });
        pane
    }

    fn working_session(&self) -> String {
        self.war_room
            .sessions
            .iter()
            .find(|p| p.state == SessionState::Working)
            .expect("a working demo session")
            .id
            .clone()
    }
}

impl Source for Feed {
    fn request(&self, method: &str, _params: Value, out: &Outbox) -> Option<Value> {
        let result = match method {
            "warroom/subscribe" => {
                *self.out.lock().unwrap() = Some(out.clone());
                // The response goes to the outbox, as the demo backend does it.
                let sub = Subscription {
                    epoch: 1,
                    cursor: 0,
                    snapshot: Some(self.war_room.clone()),
                    deltas: Vec::new(),
                };
                assert!(out.try_send(json!({ "jsonrpc": "2.0", "id": 1, "result": sub })));
                return None;
            }
            "settings/get" => {
                let view = serde_json::to_value(&*self.view.lock().unwrap()).unwrap();
                self.settings_reads.fetch_add(1, Ordering::SeqCst);
                view
            }
            "inbox/list" => {
                let list = InboxList {
                    items: self.inbox.lock().unwrap().clone(),
                    done: Vec::new(),
                };
                self.inbox_reads.fetch_add(1, Ordering::SeqCst);
                serde_json::to_value(list).unwrap()
            }
            other => panic!("unexpected method {other}"),
        };
        Some(json!({ "jsonrpc": "2.0", "id": 1, "result": result }))
    }
}

/// A sender that records the pushes and answers from a script (default: sent).
#[derive(Debug, Default)]
struct Fake {
    sent: Mutex<Vec<(String, PushTarget, sealed::Envelope)>>,
    script: Mutex<VecDeque<Outcome>>,
}

impl Fake {
    fn count(&self) -> usize {
        self.sent.lock().unwrap().len()
    }

    fn wait_for(&self, n: usize) {
        let end = Instant::now() + Duration::from_secs(5);
        while self.count() < n {
            assert!(
                Instant::now() < end,
                "expected {n} pushes, got {}",
                self.count()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Open push `i` with the device secret.
    fn open(&self, i: usize, secret: &[u8; 32]) -> sealed::PushPlain {
        sealed::open(secret, &self.sent.lock().unwrap()[i].2).unwrap()
    }
}

impl Sender for Fake {
    fn send(&self, url: &str, target: &PushTarget, envelope: &sealed::Envelope) -> Outcome {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((url.to_string(), target.clone(), envelope.clone()));
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Outcome::Sent)
    }
}

// ---- helpers -----------------------------------------------------------------------------

struct Rig {
    feed: Arc<Feed>,
    fake: Arc<Fake>,
    pairings: Arc<Pairings>,
    device: String,
    secret: [u8; 32],
    _service: PushService,
}

const URL: &str = "https://push.test/v1/push";

/// Pair one phone with a push target and start the worker.
fn pair(pairings: &Pairings, key: &str) -> String {
    let code = pairings.pair().code;
    let hello = Hello {
        code: Some(code),
        device: "Test phone".into(),
        os: "test".into(),
        key: key.into(),
        need_token: false,
    };
    pairings.admit_key(key, &hello).expect("paired").0.id
}

fn rig(url: Option<&str>, minute: fn() -> u32) -> Rig {
    let feed = Feed::new();
    let fake = Arc::new(Fake::default());
    let pairings = Arc::new(Pairings::load_with(None, None));
    let device = pair(&pairings, &"ab".repeat(32));
    let (secret, public) = sealed::generate_keypair();
    assert!(pairings.set_push(
        &device,
        PushTarget {
            token: "0a1b2c".into(),
            platform: aivory_protocol::PushPlatform::Apns,
            key: sealed::to_base64url(&public),
        }
    ));
    let service = PushService::start(
        Arc::clone(&feed) as Arc<dyn Source>,
        Arc::clone(&pairings),
        Arc::clone(&fake) as Arc<dyn Sender>,
        Options {
            url: url.map(str::to_string),
            local_minute: minute,
        },
    )
    .unwrap();
    feed.wait_subscribed();
    // The first read of the inbox ends the baseline: the worker knows the snapshot.
    Feed::wait_reads(&feed.inbox_reads, 1);
    Rig {
        feed,
        fake,
        pairings,
        device,
        secret,
        _service: service,
    }
}

fn session(pane: Pane) -> Delta {
    Delta::Session {
        session: Box::new(pane),
    }
}

// ---- the worker --------------------------------------------------------------------------

#[test]
fn a_needs_you_transition_sends_one_sealed_push_and_a_repeat_does_not() {
    let r = rig(Some(URL), noon);
    let id = r.feed.working_session();
    let title = r.feed.war_room.session(&id).unwrap().title.clone();

    r.feed.publish(session(r.feed.pane(
        &id,
        SessionState::NeedsYou,
        Some(AskKind::Permission),
    )));
    r.fake.wait_for(1);
    let plain = r.fake.open(0, &r.secret);
    assert_eq!(plain.session, id);
    assert_eq!(plain.kind, "approval");
    assert!(plain.text.contains(&title), "{}", plain.text);
    assert!(plain.text.chars().count() <= 120);
    assert_eq!(r.fake.sent.lock().unwrap()[0].0, URL);
    assert_eq!(r.fake.sent.lock().unwrap()[0].1.token, "0a1b2c");

    // The session goes back to work and asks again within 60 s: no second push.
    r.feed
        .publish(session(r.feed.pane(&id, SessionState::Working, None)));
    r.feed.publish(session(r.feed.pane(
        &id,
        SessionState::NeedsYou,
        Some(AskKind::Permission),
    )));
    // An error of the same session is another kind: it passes. It proves that the repeat
    // was handled before it.
    r.feed.publish(session(r.feed.pane(
        &id,
        SessionState::Error,
        Some(AskKind::Error),
    )));
    r.fake.wait_for(2);
    assert_eq!(r.fake.open(1, &r.secret).kind, "error");
    assert_eq!(r.fake.count(), 2);
}

#[test]
fn the_snapshot_state_sends_nothing() {
    let r = rig(Some(URL), noon);
    // The demo snapshot has sessions that need the user. A session that stays in its state
    // sends nothing. The error below is the first and only push.
    let id = r.feed.working_session();
    r.feed.publish(session(r.feed.pane(
        &id,
        SessionState::Error,
        Some(AskKind::Error),
    )));
    r.fake.wait_for(1);
    assert_eq!(r.fake.count(), 1);
    assert_eq!(r.fake.open(0, &r.secret).kind, "error");
}

#[test]
fn quiet_hours_hold_back_everything_except_errors() {
    let r = rig(Some(URL), late);
    let id = r.feed.working_session();
    r.feed.publish(session(r.feed.pane(
        &id,
        SessionState::NeedsYou,
        Some(AskKind::Plan),
    )));
    r.feed.publish(session(r.feed.pane(
        &id,
        SessionState::Error,
        Some(AskKind::Error),
    )));
    r.fake.wait_for(1);
    assert_eq!(r.fake.open(0, &r.secret).kind, "error");
    assert_eq!(r.fake.count(), 1);
}

#[test]
fn quiet_hours_off_let_the_night_through() {
    let r = rig(Some(URL), late);
    r.feed.settings(|s| s.quiet_hours.on = false);
    let id = r.feed.working_session();
    r.feed.publish(session(r.feed.pane(
        &id,
        SessionState::NeedsYou,
        Some(AskKind::Question),
    )));
    r.fake.wait_for(1);
    assert_eq!(r.fake.open(0, &r.secret).kind, "question");
}

#[test]
fn a_new_review_item_pushes_and_the_phone_switch_is_respected() {
    let r = rig(Some(URL), noon);
    let template = r
        .feed
        .inbox
        .lock()
        .unwrap()
        .iter()
        .find(|i| i.kind == InboxKind::Review)
        .cloned()
        .expect("a demo review item");
    let item = |id: &str| InboxItem {
        id: id.into(),
        ..template.clone()
    };

    // Phone off for review: the new item sends nothing.
    r.feed.settings(|s| s.notify.review.phone = false);
    r.feed.inbox.lock().unwrap().push(item("review-new-1"));
    r.feed.publish(Delta::Counts {
        queued: 0,
        inbox_open: 90,
    });
    Feed::wait_reads(&r.feed.settings_reads, 1);
    // Phone on again: the next new item pushes.
    r.feed.settings(|s| s.notify.review.phone = true);
    r.feed.inbox.lock().unwrap().push(item("review-new-2"));
    r.feed.publish(Delta::Counts {
        queued: 0,
        inbox_open: 91,
    });
    r.fake.wait_for(1);
    let plain = r.fake.open(0, &r.secret);
    assert_eq!(plain.kind, "review");
    assert!(plain.text.starts_with("Ready for review: "));
    assert_eq!(r.fake.count(), 1);
}

#[test]
fn a_new_conflict_item_pushes() {
    let r = rig(Some(URL), noon);
    let mut conflict = r
        .feed
        .inbox
        .lock()
        .unwrap()
        .iter()
        .find(|i| i.kind == InboxKind::Conflict)
        .cloned()
        .expect("a demo conflict item");
    conflict.id = "conflict-new".into();
    r.feed.inbox.lock().unwrap().push(conflict);
    r.feed.publish(Delta::Counts {
        queued: 0,
        inbox_open: 99,
    });
    r.fake.wait_for(1);
    assert_eq!(r.fake.open(0, &r.secret).kind, "conflict");
}

#[test]
fn no_url_means_no_push_and_the_setting_turns_it_on() {
    let r = rig(None, noon);
    let id = r.feed.working_session();
    r.feed.publish(session(r.feed.pane(
        &id,
        SessionState::NeedsYou,
        Some(AskKind::Plan),
    )));
    // The worker reads the settings once for this event. Change them after that.
    Feed::wait_reads(&r.feed.settings_reads, 1);
    r.feed
        .settings(|s| s.remote.push_url = "https://setting.test/v1/push".into());
    r.feed.publish(session(r.feed.pane(
        &id,
        SessionState::Error,
        Some(AskKind::Error),
    )));
    r.fake.wait_for(1);
    assert_eq!(r.fake.count(), 1);
    assert_eq!(
        r.fake.sent.lock().unwrap()[0].0,
        "https://setting.test/v1/push"
    );
    assert_eq!(r.fake.open(0, &r.secret).kind, "error");
}

#[test]
fn a_410_removes_the_push_target() {
    let r = rig(Some(URL), noon);
    r.fake.script.lock().unwrap().push_back(Outcome::Gone);
    let id = r.feed.working_session();
    r.feed.publish(session(r.feed.pane(
        &id,
        SessionState::Error,
        Some(AskKind::Error),
    )));
    r.fake.wait_for(1);
    let end = Instant::now() + Duration::from_secs(5);
    while !r.pairings.push_targets().is_empty() {
        assert!(Instant::now() < end, "the target stays");
        thread::sleep(Duration::from_millis(10));
    }
    // The device stays paired.
    assert!(r.pairings.devices().iter().any(|d| d.id == r.device));
}

// ---- the RPC -----------------------------------------------------------------------------

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

fn rpc(daemon: &Daemon, method: &str, params: Value) -> Value {
    let (out, _rx) = Outbox::new(4);
    let line = json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params });
    daemon
        .handle_line(&line.to_string(), &out)
        .expect("a response")
}

/// Send the request as the calling device `id`, as a paired connection does.
fn rpc_as(daemon: &Daemon, id: &str, method: &str, params: Value) -> Value {
    as_caller(Caller::Device(id.to_string()), || {
        rpc(daemon, method, params)
    })
}

#[test]
fn devices_push_belongs_to_the_calling_device() {
    let pairings = Arc::new(Pairings::load_with(None, None));
    let one = pair(&pairings, &"11".repeat(32));
    let two = pair(&pairings, &"22".repeat(32));
    let daemon = Daemon::with_demo(config()).with_pairings(Arc::clone(&pairings));
    let (_, public) = sealed::generate_keypair();
    let key = sealed::to_base64url(&public);
    let body = json!({ "token": "0a1b", "platform": "apns", "pushKey": key });

    // The local pipe (the desktop) cannot register a push target.
    let r = rpc(&daemon, "devices/push", body.clone());
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("paired phone"),
        "{r}"
    );
    assert!(pairings.push_targets().is_empty());

    // No `id`: the target goes to the caller.
    let r = rpc_as(&daemon, &one, "devices/push", body.clone());
    assert_eq!(r["result"], json!({ "ok": true }), "{r}");
    // The own id is fine.
    let mut own = body.clone();
    own["id"] = json!(one);
    assert_eq!(
        rpc_as(&daemon, &one, "devices/push", own)["result"]["ok"],
        true
    );

    // The id of another device is an error, and the target of that device does not change.
    let mut other = body.clone();
    other["id"] = json!(two);
    other["token"] = json!("ffff");
    let r = rpc_as(&daemon, &one, "devices/push", other);
    assert_eq!(r["error"]["code"], -32602, "{r}");
    let targets = pairings.push_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].0, one);
    assert_eq!(targets[0].1.token, "0a1b");

    // A removed caller is unknown.
    assert!(pairings.remove(&one));
    let r = rpc_as(&daemon, &one, "devices/push", body);
    assert_eq!(r["error"]["code"], -32005, "{r}");
}

#[test]
fn devices_push_registers_validates_and_devices_remove_clears() {
    let pairings = Arc::new(Pairings::load_with(None, None));
    let device = pair(&pairings, &"cd".repeat(32));
    let daemon = Daemon::with_demo(config()).with_pairings(Arc::clone(&pairings));
    let (_, public) = sealed::generate_keypair();
    let key = sealed::to_base64url(&public);
    let good = json!({ "id": device, "token": "0a1b", "platform": "apns", "pushKey": key });

    let r = rpc_as(&daemon, &device, "devices/push", good.clone());
    assert_eq!(r["result"], json!({ "ok": true }), "{r}");
    let targets = pairings.push_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].0, device);
    assert_eq!(targets[0].1.key, key);

    // An FCM token may replace it.
    let fcm = json!({ "id": device, "token": "fcm:token_1", "platform": "fcm", "pushKey": key });
    assert_eq!(
        rpc_as(&daemon, &device, "devices/push", fcm)["result"]["ok"],
        true
    );
    assert_eq!(pairings.push_targets()[0].1.token, "fcm:token_1");

    for (patch, what) in [
        (json!({ "id": "dnone" }), "the id of another device"),
        (json!({ "pushKey": "AAAA" }), "short key"),
        (
            json!({ "pushKey": sealed::to_base64url(&[0u8; 32]) }),
            "weak key",
        ),
        (json!({ "token": "" }), "empty token"),
        (json!({ "token": "not-hex!" }), "apns token not hex"),
        (json!({ "platform": "sms" }), "unknown platform"),
    ] {
        let mut params = good.clone();
        for (k, v) in patch.as_object().unwrap() {
            params[k] = v.clone();
        }
        let r = rpc_as(&daemon, &device, "devices/push", params);
        assert!(r.get("error").is_some(), "{what}: {r}");
    }

    let r = rpc(&daemon, "devices/remove", json!({ "id": device }));
    assert_eq!(r["result"]["ok"], true);
    assert!(pairings.push_targets().is_empty());
}

#[test]
fn push_targets_survive_a_restart() {
    let dir = std::env::temp_dir().join(format!("aivoryd-push-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("devices.json");
    let _ = std::fs::remove_file(&file);
    let pairings = Pairings::load_with(Some(file.clone()), None);
    let device = pair(&pairings, &"ef".repeat(32));
    let (_, public) = sealed::generate_keypair();
    let target = PushTarget {
        token: "t1".into(),
        platform: aivory_protocol::PushPlatform::Fcm,
        key: sealed::to_base64url(&public),
    };
    assert!(pairings.set_push(&device, target.clone()));
    assert!(!pairings.set_push("dnone", target.clone()));
    let again = Pairings::load_with(Some(file), None);
    assert_eq!(again.push_targets(), vec![(device, target)]);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- the HTTPS sender --------------------------------------------------------------------

/// A gateway stand-in: answers the scripted statuses, one connection each, and records the
/// requests.
fn gateway(statuses: &[u16]) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/push", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let mut script: VecDeque<u16> = statuses.iter().copied().collect();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            // Read the head, then the body by its length.
            loop {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).into_owned();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if body.len() >= len {
                        break;
                    }
                }
            }
            log.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf).into_owned());
            let status = script.pop_front().unwrap_or(500);
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    });
    (url, seen)
}

fn target_and_envelope() -> (PushTarget, sealed::Envelope) {
    let (_, public) = sealed::generate_keypair();
    let plain = sealed::PushPlain {
        session: "s1".into(),
        kind: "error".into(),
        text: "Stopped on an error: x".into(),
        ts: 1,
    };
    (
        PushTarget {
            token: "0a1b2c".into(),
            platform: aivory_protocol::PushPlatform::Apns,
            key: sealed::to_base64url(&public),
        },
        sealed::seal(&public, &plain).unwrap(),
    )
}

fn sender() -> HttpSender {
    HttpSender::new("secret-key").with_backoff(vec![Duration::from_millis(5); 3])
}

#[test]
fn the_sender_posts_the_contract_body_with_the_bearer_key() {
    let (url, seen) = gateway(&[202]);
    let (target, env) = target_and_envelope();
    assert_eq!(sender().send(&url, &target, &env), Outcome::Sent);
    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert!(request.starts_with("POST /v1/push "), "{request}");
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer secret-key")
    );
    let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["device_token"], "0a1b2c");
    assert_eq!(body["platform"], "apns");
    assert_eq!(body["envelope"], serde_json::to_value(&env).unwrap());
}

#[test]
fn the_sender_retries_5xx_three_times() {
    let (url, seen) = gateway(&[503, 502, 202]);
    let (target, env) = target_and_envelope();
    assert_eq!(sender().send(&url, &target, &env), Outcome::Sent);
    assert_eq!(seen.lock().unwrap().len(), 3);

    let (url, seen) = gateway(&[503, 503, 503, 503, 202]);
    assert_eq!(sender().send(&url, &target, &env), Outcome::Failed);
    assert_eq!(seen.lock().unwrap().len(), 4);
}

#[test]
fn the_sender_drops_4xx_and_reports_410() {
    let (target, env) = target_and_envelope();
    let (url, seen) = gateway(&[400]);
    assert_eq!(sender().send(&url, &target, &env), Outcome::Failed);
    assert_eq!(seen.lock().unwrap().len(), 1);
    let (url, seen) = gateway(&[410]);
    assert_eq!(sender().send(&url, &target, &env), Outcome::Gone);
    assert_eq!(seen.lock().unwrap().len(), 1);
}
