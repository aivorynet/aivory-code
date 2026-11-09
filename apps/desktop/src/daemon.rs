//! The daemon link. One background thread keeps a `warroom/subscribe` feed open on the daemon
//! endpoint (named pipe or Unix socket), resubscribes with its cursor after a drop, and sends
//! the events to the UI.

use std::io::{self, BufRead, Write};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use aivory_protocol::rpc::{
    self, DaemonInfo, DeltaNote, Empty, Method, SubscribeParams, Subscription, WARROOM_DELTA,
    method,
};
use aivoryd::Endpoint;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use serde_json::Value;

const INFO_ID: u64 = 1;
const SUBSCRIBE_ID: u64 = 2;
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// What the feed thread tells the UI.
#[derive(Debug)]
pub enum Event {
    Connected(DaemonInfo),
    Subscribed(Box<Subscription>),
    Delta(DeltaNote),
    /// The connection failed or dropped. The thread retries on its own.
    Offline(String),
}

/// Start the feed thread. It stops when the receiver is dropped.
pub fn spawn_feed(endpoint: Endpoint) -> UnboundedReceiver<Event> {
    let (tx, rx) = unbounded();
    thread::Builder::new()
        .name("aivory-feed".into())
        .spawn(move || feed(&endpoint, &tx))
        .expect("cannot start the feed thread");
    rx
}

/// Serve the demo war room in this process on a private endpoint (`aivory --demo`).
pub fn start_demo() -> anyhow::Result<Endpoint> {
    // One name for each call: two demo daemons of one process (the tests) must not share a pipe.
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let endpoint = Endpoint::new(&format!("desktop-demo-{}-{n}", std::process::id()))?;
    let config = aivoryd::Config::from_environment().map_err(anyhow::Error::msg)?;
    let daemon = Arc::new(aivoryd::Daemon::with_demo(config));
    let server = aivoryd::Server::bind(daemon, endpoint.clone())?;
    thread::Builder::new()
        .name("aivoryd-demo".into())
        .spawn(move || server.run())?;
    Ok(endpoint)
}

/// Start `aivoryd serve --iroh` from the app folder when no daemon answers on the endpoint.
/// The installers place `aivoryd` next to `aivory` and register no service, so the app starts
/// it. `--iroh` lets a phone pair from Settings > Devices (iroh accepts only paired devices).
/// Runs on its own thread; the feed thread connects when the daemon is up.
pub fn ensure_daemon(name: String, endpoint: Endpoint) {
    thread::spawn(move || {
        if call::<method::DaemonInfoMethod>(&endpoint, &Empty {}).is_ok() {
            return;
        }
        if let Err(e) = spawn_daemon(&name) {
            eprintln!("aivory: cannot start aivoryd: {e:#}");
        }
    });
}

fn spawn_daemon(name: &str) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("the app has no folder"))?;
    let daemon = dir.join(format!("aivoryd{}", std::env::consts::EXE_SUFFIX));
    if !daemon.exists() {
        anyhow::bail!("{} is missing", daemon.display());
    }
    let mut cmd = std::process::Command::new(&daemon);
    cmd.args(["serve", "--iroh", "--endpoint", name])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | DETACHED_PROCESS: no console, and the daemon outlives the app.
        cmd.creation_flags(0x0800_0000 | 0x0000_0008);
    }
    cmd.spawn()?;
    Ok(())
}

/// Call one method on its own short connection. Blocking: run it on the background executor.
pub fn call<M: Method>(endpoint: &Endpoint, params: &M::Params) -> anyhow::Result<M::Result> {
    let params = serde_json::to_value(params)?;
    let response = aivoryd::client::call(endpoint, M::NAME, params, &mut |_| {})?;
    Ok(rpc::parse_response::<M>(&response)?)
}

fn feed(endpoint: &Endpoint, tx: &UnboundedSender<Event>) {
    let mut resume = SubscribeParams::default();
    let mut backoff = Duration::from_millis(250);
    loop {
        let error = match subscribe(endpoint, &mut resume, tx, &mut backoff) {
            Ok(()) => return,
            Err(e) => e,
        };
        if tx
            .unbounded_send(Event::Offline(error.to_string()))
            .is_err()
        {
            return;
        }
        thread::sleep(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Run one connection until it drops. `Ok` means the UI is gone.
fn subscribe(
    endpoint: &Endpoint,
    resume: &mut SubscribeParams,
    tx: &UnboundedSender<Event>,
    backoff: &mut Duration,
) -> io::Result<()> {
    let mut conn = aivoryd::endpoint::connect(endpoint)?;
    send(
        &mut conn.writer,
        &rpc::request::<method::DaemonInfoMethod>(INFO_ID, &Empty {}),
    )?;
    send(
        &mut conn.writer,
        &rpc::request::<method::WarroomSubscribe>(SUBSCRIBE_ID, resume),
    )?;
    *backoff = Duration::from_millis(250);
    let mut line = String::new();
    loop {
        line.clear();
        if conn.reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the daemon closed the connection",
            ));
        }
        let message: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
        let event = match message.get("id").and_then(Value::as_u64) {
            Some(INFO_ID) => Event::Connected(parse::<method::DaemonInfoMethod>(&message)?),
            Some(SUBSCRIBE_ID) => {
                let sub = parse::<method::WarroomSubscribe>(&message)?;
                resume.epoch = Some(sub.epoch);
                resume.cursor = Some(sub.cursor);
                Event::Subscribed(Box::new(sub))
            }
            _ if message.get("method").and_then(Value::as_str) == Some(WARROOM_DELTA) => {
                let note: DeltaNote =
                    serde_json::from_value(message["params"].clone()).map_err(io::Error::other)?;
                resume.cursor = Some(note.seq);
                Event::Delta(note)
            }
            _ => continue,
        };
        if tx.unbounded_send(event).is_err() {
            return Ok(());
        }
    }
}

fn send(writer: &mut impl Write, request: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(request).map_err(io::Error::other)?;
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

fn parse<M: Method>(response: &Value) -> io::Result<M::Result> {
    rpc::parse_response::<M>(response).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use futures::executor::block_on;

    #[test]
    fn feed_receives_the_demo_snapshot_over_the_endpoint() {
        let endpoint = start_demo().expect("demo daemon");
        let mut events = spawn_feed(endpoint);
        let info = match block_on(events.next()) {
            Some(Event::Connected(info)) => info,
            other => panic!("expected Connected, got {other:?}"),
        };
        assert!(info.demo);
        match block_on(events.next()) {
            Some(Event::Subscribed(sub)) => {
                let war = sub.snapshot.expect("a fresh subscription has a snapshot");
                assert!(!war.sessions.is_empty());
                assert_eq!(war.epoch, sub.epoch);
            }
            other => panic!("expected Subscribed, got {other:?}"),
        }
    }
}
