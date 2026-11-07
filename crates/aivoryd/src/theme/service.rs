//! The theme service: stage, add, list, remove, update, diff, registries, and the jobs that run
//! the downloads on a worker thread (the connection handler blocks per request).
//!
//! Safety (ADR 0024 section 8): the download goes through [`Fetcher`] (HTTPS only, 10 MB stop),
//! [`aivory_theme::archive::extract`] unpacks it into `themes/.staging/<name>`, and
//! [`aivory_theme::validate::validate_dir`] checks it. Only a valid theme moves to `themes/<id>`
//! with one rename. `theme.lock` keeps the hashes.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use aivory_protocol::theme::{
    Inspected, InstalledTheme, JobRef, JobState, JobStatus, OFFICIAL_REGISTRY,
    OFFICIAL_REGISTRY_URL, Registry, RegistryEntry, RegistryList, RemoveResult, ThemeDiff,
    ThemeList, UpdateAvailable, UpdateList, UpdateResult,
};
use aivory_theme::archive::{extract, sha256_hex};
use aivory_theme::package::{self, Semver};
use aivory_theme::validate::{Options, format_issues, validate_dir};
use serde::Serialize;
use serde_json::Value;

use super::fetch::{self, Fetcher, JSON_MAX, Source, download, require_https};
use super::lock::{self, Lock};
use super::store::Store;

/// A registry index has at most this many items.
pub const REGISTRY_ITEMS_MAX: usize = 2000;
/// Staged downloads that wait for `theme/add`.
const STAGED_MAX: usize = 8;
/// Jobs that run at the same time.
const JOBS_RUNNING_MAX: usize = 4;
/// Finished jobs that the service keeps.
const JOBS_KEPT: usize = 64;

#[derive(Debug)]
struct Staged {
    dir: PathBuf,
    inspected: Inspected,
}

#[derive(Debug, Default)]
struct State {
    jobs: HashMap<String, JobStatus>,
    order: VecDeque<String>,
    staged: HashMap<String, Staged>,
    staged_order: VecDeque<String>,
}

/// A downloaded, unpacked and checked theme in a staging folder.
#[derive(Debug)]
struct Fetched {
    dir: PathBuf,
    package: package::Package,
    lock: Lock,
    archive_bytes: u64,
}

#[derive(Debug)]
pub struct ThemeService {
    store: Store,
    fetcher: Arc<dyn Fetcher>,
    state: Mutex<State>,
    /// One writer at a time on the themes folder.
    apply: Mutex<()>,
    registries: Mutex<()>,
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the OS random source works");
    aivory_theme::archive::sha256_hex(&buf)[..bytes * 2].to_string()
}

/// A token is 32 lowercase hex characters. Anything else is not a token.
pub fn token_ok(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn to_value<T: Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

impl ThemeService {
    pub fn new(store: Store, fetcher: Arc<dyn Fetcher>) -> Self {
        store.clean_staging();
        Self {
            store,
            fetcher,
            state: Mutex::new(State::default()),
            apply: Mutex::new(()),
            registries: Mutex::new(()),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    // ---- stage and add ----

    fn fetch_stage(&self, source: &Source) -> Result<Fetched, String> {
        let d = download(self.fetcher.as_ref(), source)?;
        let sha = sha256_hex(&d.bytes);
        let dir = self.store.new_staging(&random_hex(16))?;
        match self.unpack(&dir, &d, &sha, source) {
            Ok(f) => Ok(f),
            Err(e) => {
                let _ = fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    fn unpack(
        &self,
        dir: &Path,
        d: &fetch::Downloaded,
        sha: &str,
        source: &Source,
    ) -> Result<Fetched, String> {
        let out = extract(&d.bytes, dir, d.strip_top)
            .map_err(|e| format!("the archive is not safe: {e}"))?;
        if let Some(f) = out.files.keys().find(|p| p.ends_with(".upstream")) {
            return Err(format!(
                "\"{f}\": the name .upstream is reserved for updates"
            ));
        }
        // The package may carry its own theme.lock. It has no meaning here.
        let _ = fs::remove_file(dir.join(lock::FILE));
        let report = validate_dir(
            dir,
            &Options {
                tag: Some(&d.tag),
                app: None,
            },
        );
        let Some(package) = report.package else {
            return Err(format!(
                "the theme is not valid:\n{}",
                format_issues(&report.issues)
            ));
        };
        let files: BTreeMap<String, String> = out
            .files
            .into_iter()
            .filter(|(p, _)| p != lock::FILE)
            .collect();
        let new_lock = Lock {
            source: source.canonical(),
            version: package.manifest.version.clone(),
            tag: d.tag.clone(),
            archive_sha256: sha.to_string(),
            files,
        };
        if self.store.exists(&package.manifest.id) {
            let have = Lock::read(&self.store.dir(&package.manifest.id)?)?;
            have.check_same_version(&new_lock.version, sha)?;
        }
        Ok(Fetched {
            dir: dir.to_path_buf(),
            package,
            lock: new_lock,
            archive_bytes: d.bytes.len() as u64,
        })
    }

    /// `theme/inspect`: download, check and stage. Nothing is installed.
    pub fn inspect(&self, source: &str) -> Result<Inspected, String> {
        let source = Source::parse(source)?;
        let f = self.fetch_stage(&source)?;
        if let Err(e) = f.lock.write(&f.dir) {
            let _ = fs::remove_dir_all(&f.dir);
            return Err(e);
        }
        let m = &f.package.manifest;
        let token = random_hex(16);
        let inspected = Inspected {
            token: token.clone(),
            id: m.id.clone(),
            name: m.name.clone(),
            author: m.author.name.clone(),
            license: m.license.clone(),
            version: m.version.clone(),
            source: f.lock.source.clone(),
            archive_bytes: f.archive_bytes,
            size: f.package.bytes,
            files: u32::try_from(f.package.files).unwrap_or(u32::MAX),
            sha256: f.lock.archive_sha256.clone(),
            installed: self.store.exists(&m.id),
        };
        let mut st = guard(&self.state);
        while st.staged_order.len() >= STAGED_MAX {
            if let Some(old) = st.staged_order.pop_front()
                && let Some(s) = st.staged.remove(&old)
            {
                let _ = fs::remove_dir_all(s.dir);
            }
        }
        st.staged.insert(
            token.clone(),
            Staged {
                dir: f.dir,
                inspected: inspected.clone(),
            },
        );
        st.staged_order.push_back(token);
        Ok(inspected)
    }

    /// `theme/add`: commit a staged theme. The token comes from `theme/inspect`.
    pub fn add(&self, token: &str) -> Result<InstalledTheme, String> {
        if !token_ok(token) {
            return Err("bad token: a token has 32 lowercase hex characters".to_string());
        }
        let _g = guard(&self.apply);
        let staged = {
            let mut st = guard(&self.state);
            st.staged_order.retain(|t| t != token);
            st.staged.remove(token)
        };
        let Some(staged) = staged else {
            return Err("unknown or expired token; run theme/inspect again".to_string());
        };
        let id = staged.inspected.id.clone();
        let result = (|| {
            // The folder is ours, but check again: the add is the last gate.
            let report = validate_dir(&staged.dir, &Options::default());
            if !report.is_ok() {
                return Err(format!(
                    "the theme is not valid:\n{}",
                    format_issues(&report.issues)
                ));
            }
            self.store.commit(&staged.dir, &id)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staged.dir);
        }
        result?;
        self.installed(&id)
    }

    /// Install a theme from a local folder (command line only). The folder is checked, copied to
    /// the staging area, locked as `local:<path>` and committed.
    pub fn add_local(&self, src: &Path) -> Result<InstalledTheme, String> {
        let report = validate_dir(src, &Options::default());
        let Some(package) = report.package.filter(|_| report.issues.is_empty()) else {
            return Err(format!(
                "the theme is not valid:\n{}",
                format_issues(&report.issues)
            ));
        };
        let id = package.manifest.id.clone();
        if self.store.exists(&id) {
            return Err(format!(
                "theme \"{id}\" is installed already; use theme update"
            ));
        }
        let files = package::scan(src).map_err(|i| format_issues(&i))?;
        let stage = self.store.new_staging(&random_hex(16))?;
        let result = (|| {
            for f in &files {
                if f.rel == lock::FILE {
                    continue;
                }
                let to = stage.join(&f.rel);
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::copy(&f.abs, &to).map_err(|e| format!("{}: {e}", f.rel))?;
            }
            let hashes = package::hash_files(&stage)?;
            Lock {
                source: format!("local:{}", src.display()),
                version: package.manifest.version.clone(),
                tag: String::new(),
                archive_sha256: sha256_hex(format!("{hashes:?}").as_bytes()),
                files: hashes,
            }
            .write(&stage)?;
            let _g = guard(&self.apply);
            self.store.commit(&stage, &id)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&stage);
        }
        result?;
        self.installed(&id)
    }

    /// Stage and commit in one step (the command line without a daemon).
    pub fn add_source(&self, source: &str) -> Result<InstalledTheme, String> {
        let i = self.inspect(source)?;
        self.add(&i.token)
    }

    // ---- list and remove ----

    /// One installed theme with its edits and its check result.
    pub fn installed(&self, id: &str) -> Result<InstalledTheme, String> {
        let dir = self.store.dir(id)?;
        let lock = Lock::read(&dir).ok();
        let report = validate_dir(&dir, &Options::default());
        let manifest = report
            .package
            .as_ref()
            .map(|p| p.manifest.clone())
            .or_else(|| {
                let text = fs::read_to_string(dir.join("theme.toml")).ok()?;
                package::parse_manifest(&text).ok()
            });
        let error = (!report.is_ok()).then(|| {
            let first = report
                .issues
                .first()
                .map(ToString::to_string)
                .unwrap_or_default();
            match report.issues.len() {
                0 | 1 => first,
                n => format!("{first} (and {} more)", n - 1),
            }
        });
        let edited = lock
            .as_ref()
            .and_then(|l| l.edited(&dir).ok())
            .unwrap_or_default();
        Ok(InstalledTheme {
            id: id.to_string(),
            name: manifest
                .as_ref()
                .map_or_else(|| id.to_string(), |m| m.name.clone()),
            version: manifest
                .as_ref()
                .map_or_else(String::new, |m| m.version.clone()),
            author: manifest
                .as_ref()
                .map_or_else(String::new, |m| m.author.name.clone()),
            license: manifest
                .as_ref()
                .map_or_else(String::new, |m| m.license.clone()),
            source: lock.map(|l| l.source).unwrap_or_default(),
            edited,
            error,
        })
    }

    pub fn list(&self) -> ThemeList {
        ThemeList {
            themes: self
                .store
                .ids()
                .iter()
                .filter_map(|id| self.installed(id).ok())
                .collect(),
        }
    }

    /// `theme/remove` (the folder only). The caller clears `display.customTheme`.
    pub fn remove(&self, id: &str) -> Result<RemoveResult, String> {
        let _g = guard(&self.apply);
        self.store.remove(id)?;
        Ok(RemoveResult {
            ok: true,
            cleared_current: false,
        })
    }

    // ---- updates ----

    /// `theme/checkUpdates`: ask the source of each theme for its latest release. No download
    /// of the archive.
    pub fn check_updates(&self) -> UpdateList {
        let mut out = UpdateList {
            updates: Vec::new(),
            errors: Vec::new(),
        };
        for id in self.store.ids() {
            let r = (|| -> Result<Option<UpdateAvailable>, String> {
                let dir = self.store.dir(&id)?;
                let lock = Lock::read(&dir)?;
                let src = Source::parse(&lock.source)?.with_tag(None);
                let latest = fetch::resolve(self.fetcher.as_ref(), &src)?;
                let latest_v = latest
                    .tag
                    .strip_prefix('v')
                    .unwrap_or(&latest.tag)
                    .to_string();
                let newer = match (Semver::parse(&latest_v), Semver::parse(&lock.version)) {
                    (Ok(a), Ok(b)) => a > b,
                    _ => latest_v != lock.version,
                };
                Ok(newer.then(|| UpdateAvailable {
                    id: id.clone(),
                    current: lock.version,
                    latest: latest_v,
                }))
            })();
            match r {
                Ok(Some(u)) => out.updates.push(u),
                Ok(None) => {}
                Err(e) => out.errors.push(format!("{id}: {e}")),
            }
        }
        out
    }

    fn latest_of(&self, id: &str) -> Result<(PathBuf, Lock, Fetched), String> {
        let dir = self.store.dir(id)?;
        if !self.store.exists(id) {
            return Err(format!("theme \"{id}\" is not installed"));
        }
        let lock = Lock::read(&dir)?;
        let src = Source::parse(&lock.source)?.with_tag(None);
        let f = self.fetch_stage(&src)?;
        if f.package.manifest.id != id {
            let got = f.package.manifest.id.clone();
            let _ = fs::remove_dir_all(&f.dir);
            return Err(format!("the source now has theme \"{got}\", not \"{id}\""));
        }
        if let Err(e) = lock.check_same_version(&f.package.manifest.version, &f.lock.archive_sha256)
        {
            let _ = fs::remove_dir_all(&f.dir);
            return Err(e);
        }
        Ok((dir, lock, f))
    }

    /// `theme/diff`: local edits and upstream changes.
    pub fn diff(&self, id: &str) -> Result<ThemeDiff, String> {
        let (dir, lock, f) = self.latest_of(id)?;
        let _ = fs::remove_dir_all(&f.dir);
        let local_edits = lock.edited(&dir)?;
        let (mut changed, mut added, mut removed) = (Vec::new(), Vec::new(), Vec::new());
        for (p, h) in &f.lock.files {
            match lock.files.get(p) {
                None => added.push(p.clone()),
                Some(old) if old != h => changed.push(p.clone()),
                Some(_) => {}
            }
        }
        for p in lock.files.keys() {
            if !f.lock.files.contains_key(p) {
                removed.push(p.clone());
            }
        }
        Ok(ThemeDiff {
            id: id.to_string(),
            current_version: lock.version,
            latest_version: f.package.manifest.version,
            local_edits,
            upstream_changed: changed,
            upstream_added: added,
            upstream_removed: removed,
        })
    }

    /// `theme/update` at file level. An unedited file gets replaced. An edited file stays, and
    /// the new upstream file is next to it as `<file>.upstream`. There is no three-way merge.
    /// The merged theme is built in the staging area and checked before it replaces the folder.
    pub fn update(&self, id: &str) -> Result<UpdateResult, String> {
        let _g = guard(&self.apply);
        let (dir, old_lock, f) = self.latest_of(id)?;
        let result = self.apply_update(id, &dir, &old_lock, &f);
        let _ = fs::remove_dir_all(&f.dir);
        result
    }

    fn apply_update(
        &self,
        id: &str,
        dir: &Path,
        old: &Lock,
        f: &Fetched,
    ) -> Result<UpdateResult, String> {
        let version = f.package.manifest.version.clone();
        let mut res = UpdateResult {
            id: id.to_string(),
            version: version.clone(),
            replaced: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
            kept: Vec::new(),
        };
        if old.version == version {
            return Ok(res); // same version, same hash: nothing to do
        }
        if let (Ok(new_v), Ok(old_v)) = (Semver::parse(&version), Semver::parse(&old.version))
            && new_v < old_v
        {
            return Err(format!(
                "the latest release {version} is older than the installed {}",
                old.version
            ));
        }
        let local = package::hash_files(dir)?;
        let merged = self
            .store
            .staging()
            .join(format!("{}-merged", random_hex(8)));
        let outcome = (|| -> Result<(), String> {
            copy_dir(dir, &merged)?;
            remove_upstream_files(&merged)?;
            for (p, new_h) in &f.lock.files {
                let from = f.dir.join(p);
                let to = merged.join(p);
                let here = local.get(p);
                match old.files.get(p) {
                    Some(o) if o == new_h => {}
                    Some(o) if here == Some(o) => {
                        copy_file(&from, &to)?;
                        res.replaced.push(p.clone());
                    }
                    Some(_) => {
                        copy_file(&from, &PathBuf::from(format!("{}.upstream", to.display())))?;
                        res.kept.push(p.clone());
                    }
                    None if here.is_none() => {
                        copy_file(&from, &to)?;
                        res.added.push(p.clone());
                    }
                    None if here == Some(new_h) => {}
                    None => {
                        copy_file(&from, &PathBuf::from(format!("{}.upstream", to.display())))?;
                        res.kept.push(p.clone());
                    }
                }
            }
            for (p, o) in &old.files {
                if !f.lock.files.contains_key(p) && local.get(p) == Some(o) {
                    fs::remove_file(merged.join(p)).map_err(|e| format!("{p}: {e}"))?;
                    res.removed.push(p.clone());
                }
            }
            f.lock.write(&merged)?;
            let report = validate_dir(&merged, &Options::default());
            if !report.is_ok() {
                return Err(format!(
                    "the merged theme is not valid; the installed theme is not changed:\n{}",
                    format_issues(&report.issues)
                ));
            }
            Ok(())
        })();
        if let Err(e) = outcome {
            let _ = fs::remove_dir_all(&merged);
            return Err(e);
        }
        // Swap with two renames; undo the first when the second fails.
        let aside = self.store.staging().join(format!("{}-old", random_hex(8)));
        fs::rename(dir, &aside).map_err(|e| format!("cannot replace the theme: {e}"))?;
        if let Err(e) = fs::rename(&merged, dir) {
            let _ = fs::rename(&aside, dir);
            let _ = fs::remove_dir_all(&merged);
            return Err(format!("cannot replace the theme: {e}"));
        }
        let _ = fs::remove_dir_all(&aside);
        Ok(res)
    }

    // ---- registries ----

    fn load_registries(&self) -> Vec<RegistryEntry> {
        let mut list: Vec<RegistryEntry> = fs::read_to_string(self.store.registries_file())
            .ok()
            .and_then(|t| serde_json::from_str::<RegistryList>(&t).ok())
            .map(|l| l.registries)
            .unwrap_or_default();
        list.retain(|r| !r.official);
        let official = fs::read_to_string(self.store.registries_file())
            .ok()
            .and_then(|t| serde_json::from_str::<RegistryList>(&t).ok())
            .and_then(|l| l.registries.into_iter().find(|r| r.official));
        let mut out = vec![RegistryEntry {
            name: OFFICIAL_REGISTRY.to_string(),
            url: OFFICIAL_REGISTRY_URL.to_string(),
            official: true,
            refreshed_at: official.as_ref().and_then(|o| o.refreshed_at),
            items: official.map(|o| o.items).unwrap_or_default(),
        }];
        out.extend(list);
        out
    }

    fn save_registries(&self, list: &[RegistryEntry]) -> Result<(), String> {
        fs::create_dir_all(self.store.root()).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&RegistryList {
            registries: list.to_vec(),
        })
        .map_err(|e| e.to_string())?;
        let tmp = self.store.root().join(".registries.tmp");
        fs::write(&tmp, text).map_err(|e| e.to_string())?;
        fs::rename(&tmp, self.store.registries_file()).map_err(|e| e.to_string())
    }

    pub fn registry_list(&self) -> RegistryList {
        let _g = guard(&self.registries);
        RegistryList {
            registries: self.load_registries(),
        }
    }

    /// `registry/add`: a name `@name` and an HTTPS URL.
    pub fn registry_add(&self, name: &str, url: &str) -> Result<RegistryEntry, String> {
        let ok = name.strip_prefix('@').is_some_and(|n| {
            (1..=32).contains(&n.len())
                && n.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
        if !ok {
            return Err(format!(
                "\"{name}\" is not a registry name; use @<name> with [a-z0-9-]"
            ));
        }
        require_https(url).map_err(|e| e.to_string())?;
        let _g = guard(&self.registries);
        let mut list = self.load_registries();
        if list.iter().any(|r| r.name == name) {
            return Err(format!("registry {name} exists already"));
        }
        let entry = RegistryEntry {
            name: name.to_string(),
            url: url.to_string(),
            official: false,
            refreshed_at: None,
            items: Vec::new(),
        };
        list.push(entry.clone());
        self.save_registries(&list)?;
        Ok(entry)
    }

    pub fn registry_remove(&self, name: &str) -> Result<(), String> {
        if name == OFFICIAL_REGISTRY {
            return Err("the official registry @aivory cannot be removed".to_string());
        }
        let _g = guard(&self.registries);
        let mut list = self.load_registries();
        let before = list.len();
        list.retain(|r| r.name != name);
        if list.len() == before {
            return Err(format!("registry {name} is not known"));
        }
        self.save_registries(&list)
    }

    /// `registry/refresh`: download the index of one registry or of all. The index has at most
    /// 2 MB and 2,000 items. Registries that work are saved even when another one fails.
    pub fn registry_refresh(&self, name: Option<&str>) -> Result<RegistryList, String> {
        let snapshot = {
            let _g = guard(&self.registries);
            self.load_registries()
        };
        if let Some(n) = name
            && !snapshot.iter().any(|r| r.name == n)
        {
            return Err(format!("registry {n} is not known"));
        }
        let mut fresh: HashMap<String, Registry> = HashMap::new();
        let mut errors = Vec::new();
        for r in snapshot.iter().filter(|r| name.is_none_or(|n| n == r.name)) {
            let got = self
                .fetcher
                .get(&r.url, JSON_MAX)
                .map_err(|e| e.to_string())
                .and_then(|b| parse_registry(&b));
            match got {
                Ok(reg) => {
                    fresh.insert(r.name.clone(), reg);
                }
                Err(e) => errors.push(format!("{}: {e}", r.name)),
            }
        }
        let now = now_ms();
        let _g = guard(&self.registries);
        let mut list = self.load_registries();
        for r in &mut list {
            if let Some(reg) = fresh.remove(&r.name) {
                r.items = reg.items;
                r.refreshed_at = Some(now);
            }
        }
        self.save_registries(&list)?;
        if errors.is_empty() {
            Ok(RegistryList { registries: list })
        } else {
            Err(errors.join("; "))
        }
    }

    // ---- jobs ----

    /// Run `work` on a worker thread. Poll the result with [`ThemeService::job`].
    pub fn start_job<F>(self: &Arc<Self>, work: F) -> Result<JobRef, String>
    where
        F: FnOnce(&ThemeService) -> Result<Value, String> + Send + 'static,
    {
        let job = format!("job-{}", random_hex(8));
        {
            let mut st = guard(&self.state);
            let running = st
                .jobs
                .values()
                .filter(|j| j.state == JobState::Pending)
                .count();
            if running >= JOBS_RUNNING_MAX {
                return Err("too many jobs run; wait for one to finish".to_string());
            }
            st.jobs.insert(
                job.clone(),
                JobStatus {
                    job: job.clone(),
                    state: JobState::Pending,
                    result: None,
                    error: None,
                },
            );
            st.order.push_back(job.clone());
            while st.order.len() > JOBS_KEPT {
                if let Some(old) = st.order.pop_front() {
                    st.jobs.remove(&old);
                }
            }
        }
        let me = Arc::clone(self);
        let id = job.clone();
        let spawned = std::thread::Builder::new()
            .name("theme-job".into())
            .spawn(move || {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&me)))
                    .unwrap_or_else(|_| Err("internal error".to_string()));
                let mut st = guard(&me.state);
                if let Some(j) = st.jobs.get_mut(&id) {
                    match r {
                        Ok(v) => {
                            j.state = JobState::Done;
                            j.result = Some(v);
                        }
                        Err(e) => {
                            j.state = JobState::Failed;
                            j.error = Some(e);
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            guard(&self.state).jobs.remove(&job);
            return Err(format!("cannot start the job: {e}"));
        }
        Ok(JobRef { job })
    }

    pub fn job(&self, job: &str) -> Option<JobStatus> {
        guard(&self.state).jobs.get(job).cloned()
    }

    pub fn job_inspect(self: &Arc<Self>, source: String) -> Result<JobRef, String> {
        self.start_job(move |s| to_value(&s.inspect(&source)?))
    }

    pub fn job_check_updates(self: &Arc<Self>) -> Result<JobRef, String> {
        self.start_job(|s| to_value(&s.check_updates()))
    }

    pub fn job_update(self: &Arc<Self>, id: String) -> Result<JobRef, String> {
        self.start_job(move |s| to_value(&s.update(&id)?))
    }

    pub fn job_diff(self: &Arc<Self>, id: String) -> Result<JobRef, String> {
        self.start_job(move |s| to_value(&s.diff(&id)?))
    }

    pub fn job_registry_refresh(self: &Arc<Self>, name: Option<String>) -> Result<JobRef, String> {
        self.start_job(move |s| to_value(&s.registry_refresh(name.as_deref())?))
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

/// Parse and check a registry index.
pub fn parse_registry(bytes: &[u8]) -> Result<Registry, String> {
    if bytes.len() as u64 > JSON_MAX {
        return Err("the registry index is larger than 2 MB".to_string());
    }
    let reg: Registry =
        serde_json::from_slice(bytes).map_err(|e| format!("not a valid registry: {e}"))?;
    if reg.items.len() > REGISTRY_ITEMS_MAX {
        return Err(format!(
            "the registry has more than {REGISTRY_ITEMS_MAX} items"
        ));
    }
    for (i, it) in reg.items.iter().enumerate() {
        let at = |m: &str| format!("/items/{i}: {m}");
        let name_ok = (3..=40).contains(&it.name.len())
            && it
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !name_ok {
            return Err(at("name must be 3 to 40 characters of [a-z0-9-]"));
        }
        if Semver::parse(&it.version).is_err() || Semver::parse(&it.min_app).is_err() {
            return Err(at("version and min_app must be SemVer"));
        }
        if it.sha256.len() != 64
            || !it
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(at("sha256 must be 64 lowercase hex characters"));
        }
        let urls: Vec<&str> = match &it.source {
            aivory_protocol::theme::ItemSource::Archive(u) => vec![u.as_str()],
            aivory_protocol::theme::ItemSource::Files(f) => f.iter().map(String::as_str).collect(),
        };
        for u in urls.into_iter().chain(it.preview.as_deref()) {
            require_https(u).map_err(|e| at(&e.to_string()))?;
        }
    }
    Ok(reg)
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(p) = to.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", to.display()))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for e in fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        let ty = e.file_type().map_err(|e| e.to_string())?;
        let dest = to.join(e.file_name());
        if ty.is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else if ty.is_file() {
            fs::copy(e.path(), &dest).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn remove_upstream_files(dir: &Path) -> Result<(), String> {
    for e in fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let ty = e.file_type().map_err(|e| e.to_string())?;
        if ty.is_dir() {
            remove_upstream_files(&e.path())?;
        } else if e.file_name().to_string_lossy().ends_with(".upstream") {
            fs::remove_file(e.path()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
