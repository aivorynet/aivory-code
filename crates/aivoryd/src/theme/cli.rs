//! The command line of `aivoryd theme ...` and `aivoryd registry ...`. A command that needs the
//! themes folder calls the running daemon when there is one (so the daemon can clear
//! `display.customTheme`), and uses the library otherwise.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aivory_protocol::theme::{
    Inspected, InstalledTheme, ItemSource, ItemType, Registry, RegistryEntry, RegistryItem,
    RegistryList, RemoveResult, ThemeDiff, ThemeList, UpdateList, UpdateResult,
};
use aivory_theme::archive::{pack_dir, sha256_hex};
use aivory_theme::validate::{Options, format_issues, validate_dir};
use clap::Subcommand;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::fetch::HttpFetcher;
use super::service::ThemeService;
use super::store::Store;
use crate::client;
use crate::endpoint::Endpoint;

#[derive(Subcommand, Debug)]
pub enum ThemeCommand {
    /// Make a starter folder `./<id>` from Minimal, or from an installed theme
    Init {
        id: String,
        /// The id of an installed theme to copy
        #[arg(long)]
        from: Option<String>,
    },
    /// Download a theme (github:<owner>/<repo>[@tag] or a GitHub link), check it, and install it
    Add {
        source: String,
        /// Show name, author, license, size and hash; install nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Show the installed themes, their source, version and local changes
    List,
    /// Show the local edits and the upstream changes of a theme
    Diff { id: String },
    /// Update a theme at file level: an edited file stays and gets <file>.upstream
    Update { id: String },
    /// Remove a theme; the current theme falls back to Minimal
    Remove { id: String },
    /// Ask the source of each theme for a newer version
    Check {
        /// A theme folder to check; without it: check for updates of the installed themes
        dir: Option<PathBuf>,
        /// The Git tag of the release; the version must equal it without the v
        #[arg(long)]
        tag: Option<String>,
    },
    /// Make <id>-<version>.aivory-theme.zip from a theme folder and print its SHA-256
    Pack {
        dir: PathBuf,
        /// The folder for the zip (default: the current folder)
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Check a theme folder on each save and print the errors
    Dev {
        dir: PathBuf,
        /// Poll interval in milliseconds
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
        /// Check once and end
        #[arg(long)]
        once: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum RegistryCommand {
    /// Make registry.json and the item archives from a theme repo with registry.toml
    Build {
        dir: PathBuf,
        /// The output folder (default: <dir>/dist)
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Add a registry: aivoryd registry add @someone https://someone.github.io/aivory/registry.json
    Add { name: String, url: String },
    /// List the registries and their items
    List {
        /// Download the index of each registry first
        #[arg(long)]
        refresh: bool,
    },
    /// Remove a registry
    Remove { name: String },
}

enum Backend {
    Daemon(Endpoint),
    Library(Arc<ThemeService>),
}

fn rpc<T: DeserializeOwned>(ep: &Endpoint, method: &str, params: Value) -> Result<T, String> {
    let resp = client::call(ep, method, params, &mut |_| {}).map_err(|e| e.to_string())?;
    if let Some(err) = resp.get("error") {
        return Err(err["message"]
            .as_str()
            .unwrap_or("the daemon returned an error")
            .to_string());
    }
    serde_json::from_value(resp["result"].clone()).map_err(|e| format!("unexpected answer: {e}"))
}

fn rpc_job<T: DeserializeOwned>(ep: &Endpoint, method: &str, params: Value) -> Result<T, String> {
    let job: aivory_protocol::theme::JobRef = rpc(ep, method, params)?;
    let start = Instant::now();
    loop {
        let st: aivory_protocol::theme::JobStatus =
            rpc(ep, "theme/job", json!({ "job": job.job }))?;
        match st.state {
            aivory_protocol::theme::JobState::Pending => {
                if start.elapsed() > Duration::from_secs(180) {
                    return Err("the job takes too long".to_string());
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            aivory_protocol::theme::JobState::Failed => return Err(st.error.unwrap_or_default()),
            aivory_protocol::theme::JobState::Done => {
                return serde_json::from_value(st.result.unwrap_or(Value::Null))
                    .map_err(|e| e.to_string());
            }
        }
    }
}

impl Backend {
    fn open(endpoint: &str, cache_file: Option<&Path>) -> Result<Self, String> {
        let ep = Endpoint::new(endpoint).map_err(|e| e.to_string())?;
        // The daemon answers `theme/list` when it runs and has the theme methods.
        if rpc::<ThemeList>(&ep, "theme/list", json!({})).is_ok() {
            return Ok(Self::Daemon(ep));
        }
        let svc = ThemeService::new(Store::for_user(cache_file), Arc::new(HttpFetcher::new()));
        Ok(Self::Library(Arc::new(svc)))
    }

    fn list(&self) -> Result<ThemeList, String> {
        match self {
            Self::Daemon(ep) => rpc(ep, "theme/list", json!({})),
            Self::Library(s) => Ok(s.list()),
        }
    }

    fn inspect(&self, source: &str) -> Result<Inspected, String> {
        match self {
            Self::Daemon(ep) => rpc_job(ep, "theme/inspect", json!({ "source": source })),
            Self::Library(s) => s.inspect(source),
        }
    }

    fn add(&self, token: &str) -> Result<InstalledTheme, String> {
        match self {
            Self::Daemon(ep) => rpc(ep, "theme/add", json!({ "token": token })),
            Self::Library(s) => s.add(token),
        }
    }

    fn remove(&self, id: &str) -> Result<RemoveResult, String> {
        match self {
            Self::Daemon(ep) => rpc(ep, "theme/remove", json!({ "id": id })),
            Self::Library(s) => s.remove(id),
        }
    }

    fn check(&self) -> Result<UpdateList, String> {
        match self {
            Self::Daemon(ep) => rpc_job(ep, "theme/checkUpdates", json!({})),
            Self::Library(s) => Ok(s.check_updates()),
        }
    }

    fn update(&self, id: &str) -> Result<UpdateResult, String> {
        match self {
            Self::Daemon(ep) => rpc_job(ep, "theme/update", json!({ "id": id })),
            Self::Library(s) => s.update(id),
        }
    }

    fn diff(&self, id: &str) -> Result<ThemeDiff, String> {
        match self {
            Self::Daemon(ep) => rpc_job(ep, "theme/diff", json!({ "id": id })),
            Self::Library(s) => s.diff(id),
        }
    }

    fn registries(&self, refresh: bool) -> Result<RegistryList, String> {
        match self {
            Self::Daemon(ep) => {
                if refresh {
                    rpc_job(ep, "registry/refresh", json!({}))
                } else {
                    rpc(ep, "registry/list", json!({}))
                }
            }
            Self::Library(s) => {
                if refresh {
                    s.registry_refresh(None)
                } else {
                    Ok(s.registry_list())
                }
            }
        }
    }

    fn registry_add(&self, name: &str, url: &str) -> Result<RegistryEntry, String> {
        match self {
            Self::Daemon(ep) => rpc(ep, "registry/add", json!({ "name": name, "url": url })),
            Self::Library(s) => s.registry_add(name, url),
        }
    }

    fn registry_remove(&self, name: &str) -> Result<(), String> {
        match self {
            Self::Daemon(ep) => {
                rpc::<Value>(ep, "registry/remove", json!({ "name": name })).map(|_| ())
            }
            Self::Library(s) => s.registry_remove(name),
        }
    }
}

fn say(text: impl AsRef<str>) {
    println!("{}", text.as_ref());
}

fn ok() -> Result<ExitCode, String> {
    Ok(ExitCode::SUCCESS)
}

/// Run a `theme` command.
pub fn run_theme(
    cmd: &ThemeCommand,
    endpoint: &str,
    cache_file: Option<&Path>,
) -> Result<ExitCode, String> {
    match cmd {
        ThemeCommand::Check {
            dir: Some(dir),
            tag,
        } => check_dir(dir, tag.as_deref()),
        ThemeCommand::Pack { dir, out } => pack(dir, out.as_deref()),
        ThemeCommand::Dev {
            dir,
            interval_ms,
            once,
        } => dev(dir, Duration::from_millis(*interval_ms), *once),
        ThemeCommand::Init { id, from } => init(id, from.as_deref(), cache_file),
        // A local folder installs straight into the themes folder. The daemon reads the
        // folder on each call, so it needs no message.
        ThemeCommand::Add {
            source,
            dry_run: false,
        } if Path::new(source).is_dir() => {
            let svc = ThemeService::new(Store::for_user(cache_file), Arc::new(HttpFetcher::new()));
            let t = svc.add_local(Path::new(source))?;
            say(format!("installed {} {}", t.id, t.version));
            ok()
        }
        _ => {
            let b = Backend::open(endpoint, cache_file)?;
            run_backend(cmd, &b)
        }
    }
}

fn run_backend(cmd: &ThemeCommand, b: &Backend) -> Result<ExitCode, String> {
    match cmd {
        ThemeCommand::Add { source, dry_run } => {
            let i = b.inspect(source)?;
            say(format!(
                "{} ({})  version {}\n  author  {}\n  license {}\n  size    {} bytes unpacked, {} files, archive {} bytes\n  sha256  {}",
                i.name,
                i.id,
                i.version,
                i.author,
                i.license,
                i.size,
                i.files,
                i.archive_bytes,
                i.sha256
            ));
            if *dry_run {
                say("dry run: nothing is installed");
                return ok();
            }
            let t = b.add(&i.token)?;
            say(format!("installed {} {}", t.id, t.version));
        }
        ThemeCommand::List => {
            let l = b.list()?;
            if l.themes.is_empty() {
                say("no themes installed");
            }
            for t in l.themes {
                let edits = if t.edited.is_empty() {
                    String::new()
                } else {
                    format!("  {} edited", t.edited.len())
                };
                let err = t
                    .error
                    .map(|e| format!("  INVALID: {e}"))
                    .unwrap_or_default();
                say(format!(
                    "{:<24} {:<10} {}{edits}{err}",
                    t.id, t.version, t.source
                ));
            }
        }
        ThemeCommand::Diff { id } => {
            let d = b.diff(id)?;
            say(format!(
                "{} {} -> {}",
                d.id, d.current_version, d.latest_version
            ));
            for (label, list) in [
                ("local edit", &d.local_edits),
                ("upstream changed", &d.upstream_changed),
                ("upstream added", &d.upstream_added),
                ("upstream removed", &d.upstream_removed),
            ] {
                for f in list {
                    say(format!("  {label}: {f}"));
                }
            }
        }
        ThemeCommand::Update { id } => {
            let r = b.update(id)?;
            say(format!("{} is at {}", r.id, r.version));
            for (label, list) in [
                ("replaced", &r.replaced),
                ("added", &r.added),
                ("removed", &r.removed),
                ("kept (see .upstream)", &r.kept),
            ] {
                for f in list {
                    say(format!("  {label}: {f}"));
                }
            }
        }
        ThemeCommand::Remove { id } => {
            let r = b.remove(id)?;
            say(format!("removed {id}"));
            if matches!(b, Backend::Library(_)) {
                say("note: no daemon runs; the app uses Minimal when its theme is missing");
            } else if r.cleared_current {
                say("the current theme was this one; the look is Minimal now");
            }
        }
        ThemeCommand::Check { dir: None, .. } => {
            let u = b.check()?;
            if u.updates.is_empty() {
                say("all themes are up to date");
            }
            for x in u.updates {
                say(format!("{}: {} -> {}", x.id, x.current, x.latest));
            }
            for e in u.errors {
                eprintln!("{e}");
            }
        }
        _ => unreachable!("handled in run_theme"),
    }
    ok()
}

fn check_dir(dir: &Path, tag: Option<&str>) -> Result<ExitCode, String> {
    let r = validate_dir(dir, &Options { tag, app: None });
    match r.package {
        Some(p) if r.issues.is_empty() => {
            say(format!(
                "ok: {} {} ({} files, {} bytes)",
                p.manifest.id, p.manifest.version, p.files, p.bytes
            ));
            ok()
        }
        _ => {
            eprintln!("{}", format_issues(&r.issues));
            eprintln!("{} error(s)", r.issues.len());
            Ok(ExitCode::from(1))
        }
    }
}

fn pack(dir: &Path, out: Option<&Path>) -> Result<ExitCode, String> {
    let r = validate_dir(dir, &Options::default());
    let Some(p) = r.package.filter(|_| r.issues.is_empty()) else {
        eprintln!("{}", format_issues(&r.issues));
        return Ok(ExitCode::from(1));
    };
    let bytes = pack_dir(dir)?;
    if bytes.len() as u64 > aivory_theme::caps::ARCHIVE_BYTES {
        return Err(format!(
            "the archive is larger than {} bytes",
            aivory_theme::caps::ARCHIVE_BYTES
        ));
    }
    let name = format!("{}-{}.aivory-theme.zip", p.manifest.id, p.manifest.version);
    let path = out.map_or_else(|| PathBuf::from(&name), |o| o.join(&name));
    fs::write(&path, &bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    say(format!("{}\nsha256 {}", path.display(), sha256_hex(&bytes)));
    ok()
}

/// A fingerprint of the files of a folder: path, size and time of each file.
pub fn fingerprint(dir: &Path) -> String {
    let mut parts = Vec::new();
    fn walk(dir: &Path, parts: &mut Vec<String>, depth: usize) {
        if depth > 12 {
            return;
        }
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                walk(&e.path(), parts, depth + 1);
            } else {
                let t = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                parts.push(format!("{}|{}|{t}", e.path().display(), m.len()));
            }
        }
    }
    walk(dir, &mut parts, 0);
    parts.sort();
    parts.join("\n")
}

/// One check for `theme dev`: the text to print.
pub fn dev_report(dir: &Path) -> String {
    let r = validate_dir(dir, &Options::default());
    match r.package {
        Some(p) if r.issues.is_empty() => format!("ok: {} {}", p.manifest.id, p.manifest.version),
        _ => format!("{}\n{} error(s)", format_issues(&r.issues), r.issues.len()),
    }
}

fn dev(dir: &Path, every: Duration, once: bool) -> Result<ExitCode, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    let mut last = String::new();
    loop {
        let now = fingerprint(dir);
        if now != last {
            last = now;
            say(dev_report(dir));
        }
        if once {
            return ok();
        }
        std::thread::sleep(every);
    }
}

fn init(id: &str, from: Option<&str>, cache_file: Option<&Path>) -> Result<ExitCode, String> {
    if !super::store::valid_id(id) {
        return Err(format!(
            "\"{id}\" is not a theme id (3 to 40 characters of [a-z0-9-])"
        ));
    }
    let target = PathBuf::from(id);
    if target.exists() {
        return Err(format!("{} exists already", target.display()));
    }
    match from {
        None => {
            for (rel, bytes) in aivory_theme::sample::files(id, "0.1.0") {
                let p = target.join(&rel);
                fs::create_dir_all(p.parent().unwrap_or(&target)).map_err(|e| e.to_string())?;
                fs::write(&p, bytes).map_err(|e| e.to_string())?;
            }
        }
        Some(from) => {
            let store = Store::for_user(cache_file);
            let src = store.dir(from)?;
            if !store.exists(from) {
                return Err(format!("theme \"{from}\" is not installed"));
            }
            let files = aivory_theme::package::scan(&src).map_err(|i| format_issues(&i))?;
            for f in files {
                if f.rel == "theme.lock" || f.rel.ends_with(".upstream") {
                    continue;
                }
                let p = target.join(&f.rel);
                fs::create_dir_all(p.parent().unwrap_or(&target)).map_err(|e| e.to_string())?;
                fs::copy(&f.abs, &p).map_err(|e| e.to_string())?;
            }
            let path = target.join("theme.toml");
            let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let mut t: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
            t.insert("id".into(), id.into());
            t.insert("name".into(), id.into());
            t.insert("version".into(), "0.1.0".into());
            fs::write(
                &path,
                toml::to_string_pretty(&t).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    say(format!(
        "made {}; edit it, then run: aivoryd theme dev {id}",
        target.display()
    ));
    ok()
}

/// Run a `registry` command.
pub fn run_registry(
    cmd: &RegistryCommand,
    endpoint: &str,
    cache_file: Option<&Path>,
) -> Result<ExitCode, String> {
    match cmd {
        RegistryCommand::Build { dir, out } => build(dir, out.as_deref()),
        RegistryCommand::Add { name, url } => {
            let e = Backend::open(endpoint, cache_file)?.registry_add(name, url)?;
            say(format!("added {} {}", e.name, e.url));
            ok()
        }
        RegistryCommand::List { refresh } => {
            let l = Backend::open(endpoint, cache_file)?.registries(*refresh)?;
            for r in l.registries {
                say(format!(
                    "{:<16} {} ({} items)",
                    r.name,
                    r.url,
                    r.items.len()
                ));
                for i in r.items {
                    say(format!(
                        "  {}/{}@{}  {}",
                        r.name, i.name, i.version, i.title
                    ));
                }
            }
            ok()
        }
        RegistryCommand::Remove { name } => {
            Backend::open(endpoint, cache_file)?.registry_remove(name)?;
            say(format!("removed {name}"));
            ok()
        }
    }
}

/// `registry.toml` in a theme repo.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryToml {
    name: Option<String>,
    /// Where the archives are published, for example a GitHub release download folder.
    base_url: String,
    #[serde(default)]
    item: Vec<ItemToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemToml {
    /// The folder of the theme, relative to the repo.
    path: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    preview: Option<String>,
    #[serde(default)]
    requires: Vec<String>,
}

fn build(dir: &Path, out: Option<&Path>) -> Result<ExitCode, String> {
    let text = fs::read_to_string(dir.join("registry.toml"))
        .map_err(|e| format!("cannot read registry.toml: {e}"))?;
    let cfg: RegistryToml = toml::from_str(&text).map_err(|e| format!("registry.toml: {e}"))?;
    super::fetch::require_https(&cfg.base_url).map_err(|e| e.to_string())?;
    let out_dir = out.map_or_else(|| dir.join("dist"), Path::to_path_buf);
    fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for it in &cfg.item {
        let folder = dir.join(&it.path);
        let r = validate_dir(&folder, &Options::default());
        let Some(p) = r.package.filter(|_| r.issues.is_empty()) else {
            eprintln!("{}:\n{}", it.path, format_issues(&r.issues));
            return Ok(ExitCode::from(1));
        };
        let m = &p.manifest;
        let bytes = pack_dir(&folder)?;
        let file = format!("{}-{}.aivory-theme.zip", m.id, m.version);
        fs::write(out_dir.join(&file), &bytes).map_err(|e| e.to_string())?;
        items.push(RegistryItem {
            name: m.id.clone(),
            kind: ItemType::Theme,
            version: m.version.clone(),
            title: it.title.clone().unwrap_or_else(|| m.name.clone()),
            author: m.author.name.clone(),
            license: m.license.clone(),
            description: m.description.clone(),
            preview: it.preview.clone(),
            source: ItemSource::Archive(format!("{}/{file}", cfg.base_url.trim_end_matches('/'))),
            sha256: sha256_hex(&bytes),
            min_app: m.min_app.clone(),
            catalog: m.catalog,
            displays: m.displays.to_vec(),
            requires: it.requires.clone(),
        });
        say(format!("packed {file}"));
    }
    let reg = Registry {
        schema: None,
        name: cfg.name,
        items,
    };
    let json = serde_json::to_string_pretty(&reg).map_err(|e| e.to_string())?;
    // The same check that the app runs on a downloaded index.
    super::service::parse_registry(json.as_bytes())?;
    fs::write(out_dir.join("registry.json"), json).map_err(|e| e.to_string())?;
    say(format!(
        "wrote {} ({} items)",
        out_dir.join("registry.json").display(),
        reg.items.len()
    ));
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_sample(dir: &Path, id: &str, version: &str) {
        for (rel, bytes) in aivory_theme::sample::files(id, version) {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, bytes).unwrap();
        }
    }

    #[test]
    fn dev_report_follows_the_files() {
        let d = tempfile::tempdir().unwrap();
        write_sample(d.path(), "my-theme", "1.0.0");
        let f1 = fingerprint(d.path());
        assert!(dev_report(d.path()).starts_with("ok:"));
        fs::write(
            d.path().join("layouts/desktop/inbox.json"),
            r#"{"root":{"type":"nope"}}"#,
        )
        .unwrap();
        assert_ne!(fingerprint(d.path()), f1);
        let r = dev_report(d.path());
        assert!(
            r.contains("layouts/desktop/inbox.json /root/type: unknown component"),
            "{r}"
        );
    }

    #[test]
    fn registry_build_packs_and_hashes() {
        let d = tempfile::tempdir().unwrap();
        write_sample(&d.path().join("themes/my-theme"), "my-theme", "1.0.0");
        fs::write(
            d.path().join("registry.toml"),
            "name = \"Mine\"\nbase_url = \"https://example.com/dl\"\n[[item]]\npath = \"themes/my-theme\"\n",
        )
        .unwrap();
        build(d.path(), None).unwrap();
        let json = fs::read_to_string(d.path().join("dist/registry.json")).unwrap();
        let reg: Registry = serde_json::from_str(&json).unwrap();
        let zip = fs::read(d.path().join("dist/my-theme-1.0.0.aivory-theme.zip")).unwrap();
        assert_eq!(reg.items[0].sha256, sha256_hex(&zip));
    }
}
