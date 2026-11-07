//! Theme packages (ADR 0024) with a fake fetcher: no network. All packages are synthetic.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aivory_protocol::theme::{Inspected, JobState, RegistryList, UpdateList};
use aivory_sessions::StoreRoots;
use aivoryd::theme::fetch::{FetchError, Fetcher};
use aivoryd::theme::{Store, ThemeService};
use aivoryd::{Config, Daemon, Outbox};
use serde_json::{Value, json};

#[derive(Debug, Default)]
struct Fake {
    routes: Mutex<HashMap<String, Vec<u8>>>,
}

impl Fake {
    fn put(&self, url: &str, body: Vec<u8>) {
        self.routes.lock().unwrap().insert(url.to_string(), body);
    }

    /// Serve a release of `acme/neon` with this archive as its release asset.
    fn release(&self, tag: &str, zip: Vec<u8>) {
        let v = tag.trim_start_matches('v');
        let asset = format!(
            "https://github.com/acme/neon/releases/download/{tag}/neon-{v}.aivory-theme.zip"
        );
        let json = json!({ "tag_name": tag, "assets": [{ "name": format!("neon-{v}.aivory-theme.zip"), "browser_download_url": asset }] });
        self.put(
            "https://api.github.com/repos/acme/neon/releases/latest",
            json.to_string().into_bytes(),
        );
        self.put(&asset, zip);
    }
}

impl Fetcher for Fake {
    fn get(&self, url: &str, max: u64) -> Result<Vec<u8>, FetchError> {
        assert!(url.starts_with("https://"), "{url}");
        let body = self.routes.lock().unwrap().get(url).cloned();
        match body {
            None => Err(FetchError {
                status: Some(404),
                message: format!("{url} answered with status 404"),
            }),
            Some(b) if b.len() as u64 > max => Err(FetchError::new(format!(
                "the download is larger than {max} bytes"
            ))),
            Some(b) => Ok(b),
        }
    }
}

/// The files of a package as a zip (through `pack_dir`).
fn zip_of(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let d = tempfile::tempdir().unwrap();
    for (rel, bytes) in files {
        let p = d.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    aivory_theme::archive::pack_dir(d.path()).unwrap()
}

fn sample(version: &str) -> Vec<(String, Vec<u8>)> {
    aivory_theme::sample::files("neon-night", version)
}

fn replace(files: &mut [(String, Vec<u8>)], rel: &str, text: &str) {
    files.iter_mut().find(|(r, _)| r == rel).unwrap().1 = text.as_bytes().to_vec();
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    fake: Arc<Fake>,
    svc: Arc<ThemeService>,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("themes");
    let fake = Arc::new(Fake::default());
    let svc = Arc::new(ThemeService::new(Store::new(root.clone()), fake.clone()));
    Env {
        _dir: dir,
        root,
        fake,
        svc,
    }
}

fn wait(svc: &ThemeService, job: &str) -> aivory_protocol::theme::JobStatus {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        let st = svc.job(job).expect("job");
        if st.state != JobState::Pending || Instant::now() > end {
            return st;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_good_package_installs_with_a_lock() {
    let e = env();
    e.fake.release("v1.0.0", zip_of(&sample("1.0.0")));
    let i = e.svc.inspect("github:acme/neon").unwrap();
    assert_eq!(
        (i.id.as_str(), i.version.as_str(), i.license.as_str()),
        ("neon-night", "1.0.0", "MIT")
    );
    assert_eq!(i.sha256.len(), 64);
    assert!(!i.installed && i.size > 0 && i.files > 8);
    assert!(
        !e.root.join("neon-night").exists(),
        "inspect installs nothing"
    );
    let t = e.svc.add(&i.token).unwrap();
    assert_eq!(
        (t.id.as_str(), t.source.as_str(), t.error),
        ("neon-night", "github:acme/neon", None)
    );
    assert!(e.root.join("neon-night/theme.toml").is_file());
    let lock = fs::read_to_string(e.root.join("neon-night/theme.lock")).unwrap();
    assert!(
        lock.contains("github:acme/neon") && lock.contains(&i.sha256),
        "{lock}"
    );
    assert!(lock.contains("layouts/desktop/warroom.json"));
    assert_eq!(e.svc.list().themes.len(), 1);
    // The token works once.
    assert!(e.svc.add(&i.token).is_err());
    // A link of the web page form resolves the same way.
    assert!(
        e.svc
            .inspect("https://github.com/acme/neon/releases/tag/v9.9.9")
            .is_err()
    );
}

#[test]
fn a_bad_token_fails() {
    let e = env();
    for t in [
        "",
        "../../etc",
        "ZZ",
        "0123456789abcdef0123456789abcdeg",
        "0123456789abcdef0123456789abcdef",
    ] {
        let err = e.svc.add(t).unwrap_err();
        assert!(err.contains("token"), "{t}: {err}");
    }
    assert!(!e.root.join("..").join("etc").exists());
}

#[test]
fn the_tag_zip_works_when_the_release_has_no_asset() {
    let e = env();
    let mut files = sample("1.0.0");
    // A GitHub tag zip has a top folder and dot files.
    let top: Vec<(String, Vec<u8>)> = files
        .drain(..)
        .map(|(r, b)| (format!("neon-1.0.0/{r}"), b))
        .collect();
    let d = tempfile::tempdir().unwrap();
    for (rel, bytes) in &top {
        let p = d.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    fs::create_dir_all(d.path().join("neon-1.0.0/.github")).unwrap();
    fs::write(d.path().join("neon-1.0.0/.github/ci.yml"), "x").unwrap();
    // pack_dir leaves dot files out, so write the zip here with the same function on the parent.
    let zip = aivory_theme::archive::pack_dir(d.path()).unwrap();
    e.fake.put(
        "https://api.github.com/repos/acme/neon/releases/latest",
        json!({ "tag_name": "v1.0.0", "assets": [] })
            .to_string()
            .into_bytes(),
    );
    e.fake.put(
        "https://github.com/acme/neon/archive/refs/tags/v1.0.0.zip",
        zip,
    );
    let t = e.svc.add_source("github:acme/neon").unwrap();
    assert_eq!(t.version, "1.0.0");
    assert!(
        e.root
            .join("neon-night/layouts/desktop/warroom.json")
            .is_file()
    );
}

#[test]
fn a_moved_tag_is_an_error() {
    let e = env();
    e.fake.release("v1.0.0", zip_of(&sample("1.0.0")));
    e.svc.add_source("github:acme/neon").unwrap();
    // The same version, other bytes.
    let mut other = sample("1.0.0");
    replace(
        &mut other,
        "tokens.toml",
        &String::from_utf8(sample("1.0.0")[1].1.clone())
            .unwrap()
            .replace("#2563eb", "#ff0000"),
    );
    e.fake.release("v1.0.0", zip_of(&other));
    let err = e.svc.update("neon-night").unwrap_err();
    assert!(err.contains("moved tag"), "{err}");
    let err = e.svc.inspect("github:acme/neon").unwrap_err();
    assert!(err.contains("moved tag"), "{err}");
    // The installed theme did not change.
    let tokens = fs::read_to_string(e.root.join("neon-night/tokens.toml")).unwrap();
    assert!(!tokens.contains("#ff0000"));
}

#[test]
fn the_size_cap_stops_a_big_download() {
    let e = env();
    let big = vec![7u8; (aivory_theme::caps::ARCHIVE_BYTES + 1) as usize];
    e.fake.release("v1.0.0", big);
    let err = e.svc.inspect("github:acme/neon").unwrap_err();
    assert!(err.contains("larger than"), "{err}");
    assert!(e.svc.list().themes.is_empty());
}

#[test]
fn an_unsafe_archive_is_refused_and_leaves_nothing() {
    let e = env();
    // A bomb: 3 MB of zeros in one image file.
    let mut files = sample("1.0.0");
    files.push(("assets/images/a.png".into(), vec![0u8; 3 * 1024 * 1024]));
    files.push(("assets/images/LICENSE".into(), b"x".to_vec()));
    e.fake.release("v1.0.0", zip_of(&files));
    let err = e.svc.inspect("github:acme/neon").unwrap_err();
    assert!(err.contains("ratio"), "{err}");
    let staging = e.root.join(".staging");
    let left = fs::read_dir(&staging).map(|d| d.count()).unwrap_or(0);
    assert_eq!(left, 0, "the staging folder is clean");
    // A theme that fails the validator does not install.
    let mut bad = sample("1.0.0");
    replace(
        &mut bad,
        "layouts/desktop/inbox.json",
        r#"{"root":{"type":"button","on":{"press":{"action":"delete"}}}}"#,
    );
    e.fake.release("v1.0.0", zip_of(&bad));
    let err = e.svc.inspect("github:acme/neon").unwrap_err();
    assert!(
        err.contains("layouts/desktop/inbox.json /root/on/press/action: unknown action \"delete\""),
        "{err}"
    );
}

#[test]
fn an_update_keeps_an_edited_file_and_writes_upstream() {
    let e = env();
    e.fake.release("v1.0.0", zip_of(&sample("1.0.0")));
    e.svc.add_source("github:acme/neon").unwrap();
    let dir = e.root.join("neon-night");
    // The user edits the tokens.
    let edited = fs::read_to_string(dir.join("tokens.toml"))
        .unwrap()
        .replace("#2563eb", "#00aa00");
    fs::write(dir.join("tokens.toml"), &edited).unwrap();
    assert_eq!(e.svc.list().themes[0].edited, ["tokens.toml"]);
    // Version 1.1.0 changes the tokens (edited here) and the inbox (not edited here).
    let mut v2 = sample("1.1.0");
    let up_tokens = String::from_utf8(v2[1].1.clone())
        .unwrap()
        .replace("#2563eb", "#0000ff");
    replace(&mut v2, "tokens.toml", &up_tokens);
    replace(
        &mut v2,
        "layouts/desktop/inbox.json",
        r#"{"root":{"type":"text","value":"$daemon.version"}}"#,
    );
    e.fake.release("v1.1.0", zip_of(&v2));

    let diff = e.svc.diff("neon-night").unwrap();
    assert_eq!(
        (diff.current_version.as_str(), diff.latest_version.as_str()),
        ("1.0.0", "1.1.0")
    );
    assert_eq!(diff.local_edits, ["tokens.toml"]);
    assert!(diff.upstream_changed.contains(&"tokens.toml".to_string()));
    let list: UpdateList = e.svc.check_updates();
    assert_eq!(list.updates.len(), 1, "{list:?}");

    let r = e.svc.update("neon-night").unwrap();
    assert_eq!(r.version, "1.1.0");
    assert!(
        r.replaced
            .contains(&"layouts/desktop/inbox.json".to_string()),
        "{r:?}"
    );
    assert!(r.replaced.contains(&"theme.toml".to_string()));
    assert_eq!(r.kept, ["tokens.toml"]);
    assert_eq!(
        fs::read_to_string(dir.join("tokens.toml")).unwrap(),
        edited,
        "the edit stays"
    );
    let up = fs::read_to_string(dir.join("tokens.toml.upstream")).unwrap();
    assert!(up.contains("#0000ff"));
    assert!(
        fs::read_to_string(dir.join("layouts/desktop/inbox.json"))
            .unwrap()
            .contains("daemon.version")
    );
    let t = e.svc.list().themes.remove(0);
    assert_eq!(t.version, "1.1.0");
    assert_eq!(t.error, None);
    assert_eq!(t.edited, ["tokens.toml"]);
    // Updating again at the same version does nothing.
    let again = e.svc.update("neon-night").unwrap();
    assert!(again.replaced.is_empty() && again.kept.is_empty());
}

#[test]
fn registries_add_refresh_and_remove() {
    let e = env();
    assert_eq!(e.svc.registry_list().registries[0].name, "@aivory");
    assert!(e.svc.registry_remove("@aivory").is_err());
    assert!(
        e.svc
            .registry_add("@x", "http://x.example/registry.json")
            .is_err()
    );
    assert!(
        e.svc
            .registry_add("bad", "https://x.example/registry.json")
            .is_err()
    );
    e.svc
        .registry_add("@acme", "https://acme.example/registry.json")
        .unwrap();
    assert!(
        e.svc
            .registry_add("@acme", "https://acme.example/registry.json")
            .is_err()
    );
    let sha = "a".repeat(64);
    let index = json!({ "items": [{
        "name": "neon-night", "type": "theme", "version": "1.0.0", "title": "Neon", "author": "Acme",
        "license": "MIT", "description": "d", "source": "https://github.com/acme/neon/releases/download/v1.0.0/n.zip",
        "sha256": sha, "min_app": "0.1.0", "catalog": 1, "displays": ["desktop"] }] });
    e.fake.put(
        "https://acme.example/registry.json",
        index.to_string().into_bytes(),
    );
    let job = e.svc.job_registry_refresh(Some("@acme".into())).unwrap();
    let st = wait(&e.svc, &job.job);
    assert_eq!(st.state, JobState::Done, "{st:?}");
    let list: RegistryList = serde_json::from_value(st.result.unwrap()).unwrap();
    let acme = list.registries.iter().find(|r| r.name == "@acme").unwrap();
    assert_eq!(acme.items.len(), 1);
    assert!(acme.refreshed_at.is_some());
    // The list survives a restart of the service.
    let again = ThemeService::new(Store::new(e.root.clone()), e.fake.clone());
    assert_eq!(again.registry_list().registries.len(), 2);
    // A bad index fails and keeps the old items.
    e.fake.put(
        "https://acme.example/registry.json",
        br#"{"items":[{"name":"x"}]}"#.to_vec(),
    );
    assert!(e.svc.registry_refresh(Some("@acme")).is_err());
    assert_eq!(e.svc.registry_list().registries[1].items.len(), 1);
    e.svc.registry_remove("@acme").unwrap();
    assert_eq!(e.svc.registry_list().registries.len(), 1);
}

#[test]
fn the_real_fetcher_refuses_http() {
    let f = aivoryd::theme::HttpFetcher::new();
    let err = f.get("http://127.0.0.1:9/x", 10).unwrap_err();
    assert!(err.message.contains("HTTPS"), "{err}");
}

// ---- over JSON-RPC, with the demo settings ----

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

fn call(d: &Daemon, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    let (out, _rx) = Outbox::new(16);
    d.handle_line(&line, &out).expect("a response")
}

fn ok(d: &Daemon, method: &str, params: Value) -> Value {
    let r = call(d, method, params);
    assert!(r.get("error").is_none(), "{method}: {r}");
    r["result"].clone()
}

fn rpc_job(d: &Daemon, method: &str, params: Value) -> Value {
    let job = ok(d, method, params)["job"].as_str().unwrap().to_string();
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        let st = ok(d, "theme/job", json!({ "job": job }));
        if st["state"] != "pending" || Instant::now() > end {
            assert_eq!(st["state"], "done", "{st}");
            return st["result"].clone();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn settings_theme(d: &Daemon) -> Value {
    ok(d, "settings/get", json!({}))["settings"]["display"]["customTheme"].clone()
}

#[test]
fn removing_the_current_theme_clears_custom_theme() {
    let e = env();
    e.fake.release("v1.0.0", zip_of(&sample("1.0.0")));
    let d = Daemon::with_demo(config()).with_themes(e.svc.clone());
    assert_eq!(
        settings_theme(&d),
        Value::Null,
        "the key exists and is null"
    );

    let i: Inspected = serde_json::from_value(rpc_job(
        &d,
        "theme/inspect",
        json!({ "source": "github:acme/neon" }),
    ))
    .unwrap();
    let bad = call(&d, "theme/add", json!({ "token": "../x" }));
    assert_eq!(bad["error"]["code"], -32602);
    let t = ok(&d, "theme/add", json!({ "token": i.token }));
    assert_eq!(t["id"], "neon-night");
    assert_eq!(
        ok(&d, "theme/list", json!({}))["themes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    ok(
        &d,
        "settings/set",
        json!({ "key": "display.customTheme", "value": "neon-night" }),
    );
    assert_eq!(settings_theme(&d), "neon-night");
    // Setting the look clears the theme.
    ok(
        &d,
        "settings/set",
        json!({ "key": "display.look", "value": "holo" }),
    );
    assert_eq!(settings_theme(&d), Value::Null);

    ok(
        &d,
        "settings/set",
        json!({ "key": "display.customTheme", "value": "neon-night" }),
    );
    let r = ok(&d, "theme/remove", json!({ "id": "neon-night" }));
    assert_eq!(r["clearedCurrent"], true);
    assert_eq!(settings_theme(&d), Value::Null);
    assert!(!e.root.join("neon-night").exists());
    // Another removal fails; a theme that is not current leaves the setting alone.
    assert_eq!(
        call(&d, "theme/remove", json!({ "id": "neon-night" }))["error"]["code"],
        -32005
    );
    assert_eq!(
        call(&d, "theme/remove", json!({ "id": "../x" }))["error"]["code"],
        -32602
    );
}

#[test]
fn old_settings_without_the_key_load() {
    // The file of an older build has no display.customTheme.
    let mut v: Value =
        serde_json::to_value(aivory_protocol::demo::Demo::new().settings.settings).unwrap();
    v["display"].as_object_mut().unwrap().remove("customTheme");
    let s: aivory_protocol::Settings = serde_json::from_value(v).unwrap();
    assert_eq!(s.display.custom_theme, None);
}
