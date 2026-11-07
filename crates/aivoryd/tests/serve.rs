//! Starts the server on a unique endpoint and calls it like a UI would.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use aivory_sessions::StoreRoots;
use aivoryd::{Config, Daemon, Endpoint, Server, client};
use serde_json::{Value, json};

fn fixture_config() -> Config {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("aivory-sessions")
        .join("tests")
        .join("fixtures");
    Config {
        roots: StoreRoots {
            claude: base.join("claude"),
            codex: base.join("codex"),
            gemini: base.join("gemini"),
        },
        cache_file: None,
    }
}

#[test]
fn sessions_list_over_the_local_socket() {
    let endpoint = Endpoint::new(&format!("test-{}", std::process::id())).unwrap();
    let server =
        Server::bind(Arc::new(Daemon::new(fixture_config())), endpoint.clone()).expect("bind");
    let handle = thread::spawn(move || server.run());

    // daemon/info
    let info = client::call(&endpoint, "daemon/info", Value::Null, &mut |_| {}).unwrap();
    assert_eq!(info["jsonrpc"], "2.0");
    assert_eq!(info["result"]["name"], "aivoryd");

    // sessions/list: all providers, then filtered
    let all = client::call(&endpoint, "sessions/list", json!({}), &mut |_| {}).unwrap();
    let sessions = all["result"]["sessions"]
        .as_array()
        .expect("sessions array");
    assert_eq!(
        sessions.len(),
        5,
        "2 Claude + 1 Codex + 2 Gemini fixture sessions"
    );
    for provider in ["claude", "codex", "gemini"] {
        assert!(
            sessions.iter().any(|s| s["provider"] == provider),
            "provider {provider} missing"
        );
    }

    let codex = client::call(
        &endpoint,
        "sessions/list",
        json!({ "provider": "codex" }),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(codex["result"]["sessions"].as_array().unwrap().len(), 1);

    // sessions/get, with an id prefix
    let got = client::call(
        &endpoint,
        "sessions/get",
        json!({ "id": "2222" }),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(got["result"]["id"], "22222222-2222-4222-8222-222222222222");

    // errors
    let missing = client::call(
        &endpoint,
        "sessions/get",
        json!({ "id": "nope" }),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(missing["error"]["code"], -32001);
    let unknown = client::call(&endpoint, "no/such/method", Value::Null, &mut |_| {}).unwrap();
    assert_eq!(unknown["error"]["code"], -32601);

    // A client cannot pass CLI flags through RPC.
    let flags = client::call(
        &endpoint,
        "sessions/resume",
        json!({ "id": "2222", "prompt": "x", "extra_args": ["--dangerous"] }),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(flags["error"]["code"], -32602);

    let bye = client::call(&endpoint, "daemon/shutdown", Value::Null, &mut |_| {}).unwrap();
    assert_eq!(bye["result"]["ok"], true);
    handle.join().unwrap().unwrap();
}
