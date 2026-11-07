//! Phone push (ADR 0015). When a session needs the user, the daemon seals a short event for
//! each paired phone and posts it to the push gateway. The gateway forwards it to APNs or FCM
//! and cannot read it.
//!
//! - `devices/push` ([`register`]): a paired phone gives its push token and its X25519 push
//!   key. [`crate::remote::Pairings`] keeps them in `devices.json`. The target belongs to the
//!   caller of the request (the device of the connection, [`Caller`]). A request has no device
//!   id; an `id` that is not the id of the caller is an error. The local pipe cannot register.
//! - [`PushService`]: a worker thread follows the war-room feed in process (the same feed that
//!   `warroom/subscribe` gives a client) and finds the events: a session that goes to needs-you
//!   or error, a new review item, a new conflict item. A second thread sends the jobs.
//! - The gateway URL is the flag `--push-url` (env `AIVORY_PUSH_URL`) or the setting
//!   `remote.pushUrl`. No URL: no push.
//! - The daemon key is a random bearer key in `push.key`. `aivoryd push key` prints its
//!   SHA-256 for the allow list of the gateway.
//!
//! The text of a push is a fixed label and the title of the session. It never holds transcript
//! content.

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aivory_link::push::{self as sealed, Envelope, PushPlain};
use aivory_protocol::rpc::{Delta, DeltaNote, Subscription, code};
use aivory_protocol::{
    AskKind, Channels, InboxItem, InboxKind, InboxList, Pane, PushPlatform, SessionState,
    SettingsView,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::outbox::{OUTBOX_CAPACITY, Out, Outbox};
use crate::remote::{Caller, Pairings, PushTarget};
use crate::rpc::{Daemon, RpcError};

/// One push per session and kind in this time.
pub const DEDUPE: Duration = Duration::from_secs(60);
const MAX_TOKEN_CHARS: usize = 4096;
const POLL: Duration = Duration::from_millis(200);
const RESUBSCRIBE_WAIT: Duration = Duration::from_secs(2);
const SUBSCRIBE_WAIT: Duration = Duration::from_secs(5);
const KEY_FILE: &str = "push.key";

/// The file of the daemon key, next to `iroh.key`.
pub fn key_file(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(KEY_FILE)
}

// ---- the RPC -----------------------------------------------------------------------------

/// Params of `devices/push`. `id` is optional: a client may send its own id, never another.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterParams {
    #[serde(default)]
    pub id: Option<String>,
    pub token: String,
    pub platform: PushPlatform,
    pub push_key: String,
}

/// `devices/push`: save the push target of the calling device.
pub fn register(pairings: &Pairings, caller: &Caller, p: RegisterParams) -> Result<(), RpcError> {
    let invalid = |m: &str| RpcError::new(code::INVALID_PARAMS, m);
    let Caller::Device(device) = caller else {
        return Err(RpcError::new(
            code::NOT_AVAILABLE,
            "'devices/push' is for a paired phone; the local connection has no push target",
        ));
    };
    if p.id.as_ref().is_some_and(|id| id != device) {
        return Err(invalid("the id is not the id of the calling device"));
    }
    if p.token.is_empty()
        || p.token.chars().count() > MAX_TOKEN_CHARS
        || p.token.chars().any(char::is_control)
    {
        return Err(invalid("the push token is empty, too long, or not text"));
    }
    if p.platform == PushPlatform::Apns && !p.token.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid("an APNs token is a hex string"));
    }
    sealed::parse_public_key(&p.push_key)
        .map_err(|e| invalid(&format!("the push key is not valid: {e}")))?;
    let target = PushTarget {
        token: p.token,
        platform: p.platform,
        key: p.push_key,
    };
    if pairings.set_push(device, target) {
        Ok(())
    } else {
        Err(RpcError::not_found("device", device))
    }
}

// ---- the daemon key ----------------------------------------------------------------------

/// Read the bearer key of the daemon from `path`, or create the file with a new random key.
/// On Unix only the owner can read the file.
pub fn load_or_create_key(path: &Path) -> io::Result<String> {
    if let Ok(text) = std::fs::read_to_string(path)
        && !text.trim().is_empty()
    {
        return Ok(text.trim().to_string());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let key = random_key();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(key.as_bytes())?;
    Ok(key)
}

/// A random key of 64 hex characters.
pub fn random_key() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("the OS random source works");
    hex(&buf)
}

/// The SHA-256 of the key as hex. This is the line for the allow list of the gateway.
pub fn key_hash(key: &str) -> String {
    hex(&Sha256::digest(key.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- the sender --------------------------------------------------------------------------

/// The result of one push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The gateway accepted the message.
    Sent,
    /// The gateway says the device token is not valid (410). Drop the target.
    Gone,
    /// The message is lost: a 4xx answer, or a 5xx answer after the last retry.
    Failed,
}

/// Posts a sealed envelope to the gateway. The worker thread calls it, so it may block.
pub trait Sender: Send + Sync + Debug {
    fn send(&self, url: &str, target: &PushTarget, envelope: &Envelope) -> Outcome;
}

/// The sender over HTTPS (`ureq` with rustls). It retries a 5xx answer and a network error
/// after each pause of the back-off list, and drops a 4xx answer.
#[derive(Debug)]
pub struct HttpSender {
    agent: ureq::Agent,
    key: String,
    backoff: Vec<Duration>,
}

impl HttpSender {
    /// A sender with the daemon key and 3 retries (after 0.5 s, 2 s, and 5 s).
    pub fn new(key: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .into();
        Self {
            agent,
            key: key.to_string(),
            backoff: vec![
                Duration::from_millis(500),
                Duration::from_secs(2),
                Duration::from_secs(5),
            ],
        }
    }

    /// Change the pauses before the retries. One entry is one retry.
    pub fn with_backoff(mut self, backoff: Vec<Duration>) -> Self {
        self.backoff = backoff;
        self
    }
}

impl Sender for HttpSender {
    fn send(&self, url: &str, target: &PushTarget, envelope: &Envelope) -> Outcome {
        let body = json!({
            "device_token": target.token,
            "platform": target.platform.as_str(),
            "envelope": envelope,
        })
        .to_string();
        for attempt in 0..=self.backoff.len() {
            if attempt > 0 {
                thread::sleep(self.backoff[attempt - 1]);
            }
            let result = self
                .agent
                .post(url)
                .header("Authorization", format!("Bearer {}", self.key))
                .content_type("application/json")
                .send(body.as_str());
            match result.map(|r| r.status().as_u16()) {
                Ok(200..=299) => return Outcome::Sent,
                Ok(410) => return Outcome::Gone,
                // 5xx and a network error: try again.
                Ok(500..=599) | Err(_) => {}
                Ok(_) => return Outcome::Failed,
            }
        }
        Outcome::Failed
    }
}

// ---- the feed ----------------------------------------------------------------------------

/// Where the worker reads the war room, the inbox, and the settings. The daemon is the source;
/// a test can serve its own.
pub trait Source: Send + Sync {
    /// Handle one JSON-RPC request in process. Like [`Daemon::handle_line`]: a feed
    /// subscription keeps `out`, and the response may go to `out` instead of the return value.
    fn request(&self, method: &str, params: Value, out: &Outbox) -> Option<Value>;
}

impl Source for Daemon {
    fn request(&self, method: &str, params: Value, out: &Outbox) -> Option<Value> {
        let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        self.handle_line(&line.to_string(), out)
    }
}

fn call(source: &dyn Source, method: &str, params: Value) -> Result<Value, String> {
    let (out, _rx) = Outbox::new(1);
    let response = source.request(method, params, &out).ok_or("no response")?;
    match response.get("result") {
        Some(result) => Ok(result.clone()),
        None => Err(response["error"]["message"]
            .as_str()
            .unwrap_or("error")
            .to_string()),
    }
}

// ---- events and policy -------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Approval,
    Plan,
    Question,
    Error,
    Review,
    Conflict,
}

impl Kind {
    /// The `kind` of the sealed payload.
    fn wire(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Plan => "plan",
            Self::Question => "question",
            Self::Error => "error",
            Self::Review => "review",
            Self::Conflict => "conflict",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Approval => "Needs permission",
            Self::Plan => "Plan to approve",
            Self::Question => "Question",
            Self::Error => "Stopped on an error",
            Self::Review => "Ready for review",
            Self::Conflict => "Conflict",
        }
    }

    /// The column `notify.<event>` of Settings that this kind belongs to.
    fn channels(self, view: &SettingsView) -> Channels {
        let n = &view.settings.notify;
        match self {
            Self::Approval | Self::Plan | Self::Question => n.needs_answer,
            Self::Error => n.error,
            Self::Review => n.review,
            Self::Conflict => n.conflict,
        }
    }
}

impl From<AskKind> for Kind {
    fn from(k: AskKind) -> Self {
        match k {
            AskKind::Permission => Self::Approval,
            AskKind::Plan => Self::Plan,
            AskKind::Question => Self::Question,
            AskKind::Error => Self::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Event {
    session: String,
    kind: Kind,
    text: String,
}

impl Event {
    /// The text is the label of the kind and the title of the session, at most 120 characters.
    fn new(session: &str, kind: Kind, title: &str) -> Self {
        let mut text = format!("{}: {}", kind.label(), title.trim());
        if text.chars().count() > sealed::MAX_TEXT_CHARS {
            text = text.chars().take(sealed::MAX_TEXT_CHARS - 1).collect();
            text.push('\u{2026}');
        }
        Self {
            session: session.to_string(),
            kind,
            text,
        }
    }
}

/// Minutes since midnight of `HH:MM`.
fn parse_clock(text: &str) -> Option<u32> {
    let (h, m) = text.trim().split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// `true` when `minute` lies in the quiet hours `from`-`to` (local time, the window may cross
/// midnight). A window with an equal start and end is empty.
fn in_quiet_hours(from: &str, to: &str, minute: u32) -> bool {
    let (Some(from), Some(to)) = (parse_clock(from), parse_clock(to)) else {
        return false;
    };
    match from.cmp(&to) {
        std::cmp::Ordering::Less => (from..to).contains(&minute),
        std::cmp::Ordering::Greater => minute >= from || minute < to,
        std::cmp::Ordering::Equal => false,
    }
}

/// The setting `notify.<event>.phone` and the quiet hours. There is no production flag yet, so
/// every error passes the quiet hours (the design lets production errors pass).
fn allowed(view: &SettingsView, kind: Kind, minute: u32) -> bool {
    if !kind.channels(view).phone {
        return false;
    }
    let q = &view.settings.quiet_hours;
    kind == Kind::Error || !(q.on && in_quiet_hours(&q.from, &q.to, minute))
}

/// At most one event per session and kind in [`DEDUPE`].
#[derive(Debug, Default)]
struct Dedupe(HashMap<(String, Kind), Instant>);

impl Dedupe {
    fn allow(&mut self, session: &str, kind: Kind, now: Instant) -> bool {
        self.0.retain(|_, at| now.duration_since(*at) < DEDUPE);
        let key = (session.to_string(), kind);
        if self.0.contains_key(&key) {
            return false;
        }
        self.0.insert(key, now);
        true
    }
}

/// What the worker knows of the war room, to find the transitions.
#[derive(Debug, Default)]
struct Tracker {
    /// Session id -> (state, id of the open ask).
    sessions: HashMap<String, (SessionState, Option<String>)>,
    /// Open inbox items that the worker has seen.
    items: HashSet<String>,
    inbox_open: u32,
}

impl Tracker {
    /// A pane arrived. Returns an event when it enters needs-you or error, or gets a new ask.
    fn session(&mut self, pane: &Pane) -> Option<Event> {
        let ask = pane.ask.as_ref();
        let now = (pane.state, ask.map(|a| a.id.clone()));
        let before = self.sessions.insert(pane.id.clone(), now.clone());
        if before.as_ref() == Some(&now) {
            return None;
        }
        let kind = match pane.state {
            SessionState::NeedsYou => ask.map_or(Kind::Question, |a| a.kind.into()),
            SessionState::Error => Kind::Error,
            _ => return None,
        };
        Some(Event::new(&pane.id, kind, &pane.title))
    }

    /// The open items arrived. Returns an event for each review or conflict item that is new.
    fn inbox(&mut self, items: &[InboxItem]) -> Vec<Event> {
        let events = items
            .iter()
            .filter(|i| !self.items.contains(&i.id))
            .filter_map(|i| {
                let kind = match i.kind {
                    InboxKind::Review => Kind::Review,
                    InboxKind::Conflict => Kind::Conflict,
                    _ => return None,
                };
                Some(Event::new(&i.session, kind, &i.title))
            })
            .collect();
        self.items = items.iter().map(|i| i.id.clone()).collect();
        events
    }
}

// ---- the service -------------------------------------------------------------------------

/// Settings of the push worker.
#[derive(Debug, Clone)]
pub struct Options {
    /// The gateway URL from the flag or the environment. It wins over the setting
    /// `remote.pushUrl`. `None` or empty: use the setting.
    pub url: Option<String>,
    /// Local time as minutes since midnight, for the quiet hours.
    pub local_minute: fn() -> u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            url: None,
            local_minute,
        }
    }
}

fn local_minute() -> u32 {
    use chrono::Timelike;
    let now = chrono::Local::now();
    now.hour() * 60 + now.minute()
}

#[derive(Debug)]
struct Job {
    device: String,
    url: String,
    target: PushTarget,
    envelope: Envelope,
}

/// The running push worker. Dropping it stops the worker thread.
#[derive(Debug)]
pub struct PushService {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl PushService {
    /// Start the worker thread (feed and policy) and the sender thread.
    pub fn start(
        source: Arc<dyn Source>,
        pairings: Arc<Pairings>,
        sender: Arc<dyn Sender>,
        options: Options,
    ) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (jobs, rx) = mpsc::channel::<Job>();
        let sending = Arc::clone(&pairings);
        thread::Builder::new()
            .name("aivoryd-push-send".into())
            .spawn(move || send_loop(&rx, sender.as_ref(), &sending))?;
        let mut worker = Worker {
            source,
            pairings,
            jobs,
            options,
            tracker: Tracker::default(),
            dedupe: Dedupe::default(),
        };
        let flag = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("aivoryd-push".into())
            .spawn(move || worker.run(&flag))?;
        Ok(Self {
            stop,
            worker: Some(handle),
        })
    }
}

impl Drop for PushService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn send_loop(rx: &Receiver<Job>, sender: &dyn Sender, pairings: &Pairings) {
    for job in rx {
        match sender.send(&job.url, &job.target, &job.envelope) {
            Outcome::Sent => {}
            Outcome::Gone => {
                if pairings.drop_push(&job.device, &job.target.token) {
                    eprintln!(
                        "aivoryd: the push token of device {} is not valid; target removed",
                        job.device
                    );
                }
            }
            Outcome::Failed => eprintln!("aivoryd: a push to device {} failed", job.device),
        }
    }
}

struct Worker {
    source: Arc<dyn Source>,
    pairings: Arc<Pairings>,
    jobs: mpsc::Sender<Job>,
    options: Options,
    tracker: Tracker,
    dedupe: Dedupe,
}

impl Worker {
    fn run(&mut self, stop: &AtomicBool) {
        let mut first = true;
        while !stop.load(Ordering::SeqCst) {
            match self.subscribe() {
                Ok((rx, snapshot)) => {
                    self.baseline(&snapshot, !first);
                    first = false;
                    if self.follow(&rx, stop) {
                        return;
                    }
                }
                Err(e) => eprintln!("aivoryd: push worker cannot follow the war room: {e}"),
            }
            sleep_unless(stop, RESUBSCRIBE_WAIT);
        }
    }

    fn subscribe(&self) -> Result<(Receiver<Out>, aivory_protocol::WarRoom), String> {
        let (out, rx) = Outbox::new(OUTBOX_CAPACITY);
        let response = match self.source.request("warroom/subscribe", json!({}), &out) {
            Some(r) => r,
            // The feed answers through the outbox, before any delta.
            None => loop {
                match rx.recv_timeout(SUBSCRIBE_WAIT) {
                    Ok(Out::Message(v)) if v.get("id").is_some() => break v,
                    Ok(_) => {}
                    Err(_) => return Err("no response to warroom/subscribe".into()),
                }
            },
        };
        let Some(result) = response.get("result") else {
            return Err(response["error"]["message"]
                .as_str()
                .unwrap_or("error")
                .to_string());
        };
        let sub: Subscription =
            serde_json::from_value(result.clone()).map_err(|e| e.to_string())?;
        let snapshot = sub.snapshot.ok_or("the feed sent no snapshot")?;
        Ok((rx, snapshot))
    }

    /// Learn the state of the snapshot. With `emit`, a change since the last feed (after a
    /// resubscribe) sends events; the first snapshot sends none.
    fn baseline(&mut self, snapshot: &aivory_protocol::WarRoom, emit: bool) {
        let mut events: Vec<Event> = snapshot
            .sessions
            .iter()
            .filter_map(|p| self.tracker.session(p))
            .collect();
        let live: HashSet<&str> = snapshot.sessions.iter().map(|p| p.id.as_str()).collect();
        self.tracker
            .sessions
            .retain(|id, _| live.contains(id.as_str()));
        self.tracker.inbox_open = snapshot.inbox_open;
        events.extend(self.refresh_inbox());
        if emit {
            for event in events {
                self.deliver(&event);
            }
        }
    }

    /// Read the feed until it ends (`false`) or the service stops (`true`).
    fn follow(&mut self, rx: &Receiver<Out>, stop: &AtomicBool) -> bool {
        loop {
            if stop.load(Ordering::SeqCst) {
                return true;
            }
            match rx.recv_timeout(POLL) {
                Ok(Out::Message(v)) => {
                    if v["method"] == aivory_protocol::rpc::WARROOM_DELTA
                        && let Ok(note) = serde_json::from_value::<DeltaNote>(v["params"].clone())
                    {
                        self.delta(&note.delta);
                    }
                }
                Ok(Out::Close) | Err(RecvTimeoutError::Disconnected) => return false,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn delta(&mut self, delta: &Delta) {
        let events = match delta {
            Delta::Session { session } => self.tracker.session(session).into_iter().collect(),
            Delta::SessionRemoved { id } => {
                self.tracker.sessions.remove(id);
                Vec::new()
            }
            Delta::Counts { inbox_open, .. } if *inbox_open != self.tracker.inbox_open => {
                self.tracker.inbox_open = *inbox_open;
                self.refresh_inbox()
            }
            _ => Vec::new(),
        };
        for event in events {
            self.deliver(&event);
        }
    }

    /// Read the open inbox items. Returns the events of the new review and conflict items.
    fn refresh_inbox(&mut self) -> Vec<Event> {
        let list = call(self.source.as_ref(), "inbox/list", json!({}))
            .and_then(|v| serde_json::from_value::<InboxList>(v).map_err(|e| e.to_string()));
        match list {
            Ok(list) => self.tracker.inbox(&list.items),
            Err(e) => {
                eprintln!("aivoryd: push worker cannot read the inbox: {e}");
                Vec::new()
            }
        }
    }

    /// Apply the policy, seal the event for each phone, and queue the jobs.
    fn deliver(&mut self, event: &Event) {
        let view = match call(self.source.as_ref(), "settings/get", json!({}))
            .and_then(|v| serde_json::from_value::<SettingsView>(v).map_err(|e| e.to_string()))
        {
            Ok(view) => view,
            Err(e) => {
                eprintln!("aivoryd: push worker cannot read the settings: {e}");
                return;
            }
        };
        let url = self
            .options
            .url
            .as_deref()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or(view.settings.remote.push_url.trim());
        if url.is_empty() || !allowed(&view, event.kind, (self.options.local_minute)()) {
            return;
        }
        let targets = self.pairings.push_targets();
        if targets.is_empty()
            || !self
                .dedupe
                .allow(&event.session, event.kind, Instant::now())
        {
            return;
        }
        let plain = PushPlain {
            session: event.session.clone(),
            kind: event.kind.wire().to_string(),
            text: event.text.clone(),
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        };
        for (device, target) in targets {
            let sealed =
                sealed::parse_public_key(&target.key).and_then(|key| sealed::seal(&key, &plain));
            match sealed {
                Ok(envelope) => {
                    let _ = self.jobs.send(Job {
                        device,
                        url: url.to_string(),
                        target,
                        envelope,
                    });
                }
                Err(e) => eprintln!("aivoryd: cannot seal a push for device {device}: {e}"),
            }
        }
    }
}

/// Sleep for `total`, in short steps, until `stop` is set.
fn sleep_unless(stop: &AtomicBool, total: Duration) {
    let end = Instant::now() + total;
    while Instant::now() < end && !stop.load(Ordering::SeqCst) {
        thread::sleep(POLL.min(total));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_hours_cross_midnight() {
        assert!(in_quiet_hours("22:00", "07:00", 23 * 60));
        assert!(in_quiet_hours("22:00", "07:00", 6 * 60 + 59));
        assert!(!in_quiet_hours("22:00", "07:00", 7 * 60));
        assert!(!in_quiet_hours("22:00", "07:00", 12 * 60));
        assert!(in_quiet_hours("01:00", "05:00", 2 * 60));
        assert!(!in_quiet_hours("01:00", "05:00", 5 * 60));
        assert!(!in_quiet_hours("08:00", "08:00", 8 * 60));
        assert!(!in_quiet_hours("nope", "07:00", 0));
    }

    #[test]
    fn dedupe_holds_for_a_minute() {
        let mut d = Dedupe::default();
        let t0 = Instant::now();
        assert!(d.allow("s1", Kind::Error, t0));
        assert!(!d.allow("s1", Kind::Error, t0 + Duration::from_secs(59)));
        assert!(d.allow("s1", Kind::Plan, t0 + Duration::from_secs(1)));
        assert!(d.allow("s2", Kind::Error, t0 + Duration::from_secs(1)));
        assert!(d.allow("s1", Kind::Error, t0 + Duration::from_secs(60)));
    }

    #[test]
    fn the_text_is_clipped_to_the_limit() {
        let e = Event::new("s", Kind::Review, &"x".repeat(300));
        assert_eq!(e.text.chars().count(), sealed::MAX_TEXT_CHARS);
        let e = Event::new("s", Kind::Error, "Redis failover");
        assert_eq!(e.text, "Stopped on an error: Redis failover");
    }

    #[test]
    fn the_key_is_created_once_and_hashed() {
        let dir = std::env::temp_dir().join(format!("aivoryd-push-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = key_file(&dir);
        let a = load_or_create_key(&file).unwrap();
        assert_eq!(a.len(), 64);
        assert_eq!(load_or_create_key(&file).unwrap(), a);
        assert_eq!(key_hash(&a).len(), 64);
        assert_eq!(
            key_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
