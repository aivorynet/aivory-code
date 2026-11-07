//! The shared state of the code service: the indexes, the language server pool, and the worker
//! for the end of a turn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use aivory_diag::lsp::{Lineage, LspPool, PoolConfig, SysinfoProbe, known_servers};
use aivory_index::{Index, IndexOptions};
use aivory_syntax::Language;

/// The name of the snapshot of the main checkout in each index.
const SNAPSHOT: &str = "head";
/// A query starts a new refresh when the last one is older than this.
const REFRESH_EVERY: Duration = Duration::from_secs(60);
/// The pool stops idle servers on this timer.
const TICK_EVERY: Duration = Duration::from_secs(30);
/// How long the end of a turn waits for the first refresh of an index.
const READY_WAIT: Duration = Duration::from_secs(60);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Default)]
struct RefreshState {
    running: bool,
    /// The first refresh ended (with or without an error).
    first_done: bool,
    last: Option<Instant>,
    error: Option<String>,
}

/// The index of one codebase and the state of its refresh.
#[derive(Debug)]
pub struct IndexEntry {
    pub root: PathBuf,
    pub index: Index,
    skip: Vec<PathBuf>,
    state: Mutex<RefreshState>,
    done: Condvar,
}

impl IndexEntry {
    /// Start a refresh in a background thread, unless one runs.
    pub fn refresh_now(self: &Arc<Self>) {
        {
            let mut st = lock(&self.state);
            if st.running {
                return;
            }
            st.running = true;
        }
        let me = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name("aivoryd-index".into())
            .spawn(move || {
                let opts = IndexOptions {
                    skip_dirs: me.skip.clone(),
                    ..IndexOptions::default()
                };
                let result = me.index.refresh(&me.root, SNAPSHOT, &opts);
                let mut st = lock(&me.state);
                st.running = false;
                st.first_done = true;
                st.last = Some(Instant::now());
                st.error = result.err().map(|e| e.to_string());
                if let Some(e) = &st.error {
                    eprintln!("aivoryd: index of {}: {e}", me.root.display());
                }
                me.done.notify_all();
            });
        if let Err(e) = spawned {
            eprintln!("aivoryd: cannot start the index thread: {e}");
            let mut st = lock(&self.state);
            st.running = false;
            st.first_done = true;
            self.done.notify_all();
        }
    }

    /// Start a refresh when the last one is old.
    pub fn refresh_if_old(self: &Arc<Self>) {
        let old = {
            let st = lock(&self.state);
            !st.running && st.first_done && st.last.is_none_or(|t| t.elapsed() >= REFRESH_EVERY)
        };
        if old {
            self.refresh_now();
        }
    }

    /// Wait until the first refresh ended. Returns `false` when it still runs after `timeout`.
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut st = lock(&self.state);
        while !st.first_done {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            st = self
                .done
                .wait_timeout(st, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// True while the first refresh has not ended.
    pub fn indexing(&self) -> bool {
        !lock(&self.state).first_done
    }
}

/// What the end of a turn tells the core.
#[derive(Debug, Clone)]
pub struct TurnEnd {
    /// The native session id (the key of the ledger).
    pub session: String,
    pub n: u32,
    /// The folder of the session (the folder that the snapshot covers).
    pub workdir: PathBuf,
    /// The files that the turn changed, relative to `workdir`.
    pub files: Vec<String>,
    /// The snapshot before the turn.
    pub parent_snapshot: Option<String>,
    /// The snapshot at the end of the turn. `None` when the turn changed nothing.
    pub snapshot: Option<String>,
    /// Milliseconds.
    pub ended_ms: i64,
}

/// A queued job: the end of a turn, with the core that handles it.
type Job = (Arc<CodeCore>, TurnEnd);

/// The shared state of one data folder.
pub struct CodeCore {
    dir: PathBuf,
    db: PathBuf,
    pool: Arc<LspPool>,
    indexes: Mutex<HashMap<PathBuf, Arc<IndexEntry>>>,
    worker: Mutex<Option<Sender<Job>>>,
    warmed: Mutex<Vec<(PathBuf, Language)>>,
    stop: AtomicBool,
}

impl std::fmt::Debug for CodeCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeCore")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

static REGISTRY: Mutex<Vec<(PathBuf, Weak<CodeCore>)>> = Mutex::new(Vec::new());

/// The core of the data folder of the database file `db`. All callers with the same folder
/// share one core, so that the ledger worker and the RPC methods use one pool and one index.
pub fn core_for(db: &Path) -> Arc<CodeCore> {
    let mut reg = lock(&REGISTRY);
    reg.retain(|(_, w)| w.strong_count() > 0);
    if let Some(core) = reg
        .iter()
        .find(|(p, _)| p == db)
        .and_then(|(_, w)| w.upgrade())
    {
        return core;
    }
    let core = CodeCore::new(db);
    reg.push((db.to_path_buf(), Arc::downgrade(&core)));
    core
}

/// Tell the core of the database `db` that a turn ended. The work runs in the background.
pub fn turn_ended(db: &Path, turn: TurnEnd) {
    core_for(db).submit(turn);
}

impl CodeCore {
    fn new(db: &Path) -> Arc<Self> {
        let dir = db
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let cfg = PoolConfig {
            data_dir: dir.join("lsp"),
            ..PoolConfig::default()
        };
        let core = Arc::new(Self {
            dir,
            db: db.to_path_buf(),
            pool: Arc::new(LspPool::new(cfg, known_servers(), Arc::new(SysinfoProbe))),
            indexes: Mutex::new(HashMap::new()),
            worker: Mutex::new(None),
            warmed: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&core);
        let ticker = thread::Builder::new()
            .name("aivoryd-lsp-tick".into())
            .spawn(move || {
                loop {
                    thread::sleep(TICK_EVERY);
                    let Some(core) = weak.upgrade() else { break };
                    if core.stop.load(Ordering::SeqCst) {
                        break;
                    }
                    core.pool.tick();
                }
            });
        if let Err(e) = ticker {
            eprintln!("aivoryd: cannot start the language server timer: {e}");
        }
        core
    }

    /// The database file of the ledger and the diagnostics.
    pub fn db(&self) -> &Path {
        &self.db
    }

    pub fn pool(&self) -> &Arc<LspPool> {
        &self.pool
    }

    /// The index of the checkout `root`. The first call opens the index and starts the first
    /// refresh. `name` names the folder of the index below `index/` (the codebase id).
    pub fn entry(&self, root: &Path, name: &str) -> Result<Arc<IndexEntry>, String> {
        let key = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let mut map = lock(&self.indexes);
        if let Some(e) = map.get(&key) {
            let e = Arc::clone(e);
            drop(map);
            e.refresh_if_old();
            return Ok(e);
        }
        let mut folder = safe_name(name);
        if map.values().any(|e| {
            e.index
                .dir()
                .file_name()
                .is_some_and(|n| n == folder.as_str())
        }) {
            folder = format!("{folder}-{:08x}", short_hash(&key.to_string_lossy()));
        }
        let index = Index::open(self.dir.join("index").join(&folder))
            .map_err(|e| format!("cannot open the index of {name}: {e}"))?;
        let entry = Arc::new(IndexEntry {
            root: key.clone(),
            index,
            skip: vec![self.dir.clone()],
            state: Mutex::new(RefreshState::default()),
            done: Condvar::new(),
        });
        map.insert(key, Arc::clone(&entry));
        drop(map);
        entry.refresh_now();
        Ok(entry)
    }

    /// A lineage for the language servers of the checkout `root`.
    pub fn lineage(root: &Path) -> Lineage {
        Lineage {
            id: root.to_string_lossy().into_owned(),
            root: root.to_path_buf(),
        }
    }

    /// Start the language server for a file in the background, once for each folder and
    /// language. The queries use it as soon as it runs.
    pub fn warm_lsp(&self, root: &Path, path: &str) {
        let Some(language) = Language::detect(path) else {
            return;
        };
        if self.pool.available(language).is_none() {
            return;
        }
        {
            let mut warmed = lock(&self.warmed);
            if warmed.iter().any(|(r, l)| r == root && *l == language) {
                return;
            }
            warmed.push((root.to_path_buf(), language));
        }
        let (pool, root, path) = (Arc::clone(&self.pool), root.to_path_buf(), path.to_owned());
        let _ = thread::Builder::new()
            .name("aivoryd-lsp-warm".into())
            .spawn(move || {
                let lineage = CodeCore::lineage(&root);
                let overlay = aivory_diag::lsp::Overlay::default();
                let ctx = aivory_diag::lsp::Ctx {
                    lineage: &lineage,
                    overlay: &overlay,
                };
                // A hover at the start of the file starts the server and opens the file.
                let _ = pool.hover(
                    &ctx,
                    &path,
                    aivory_diag::lsp::lsp_types::Position::new(0, 0),
                );
            });
    }

    /// Queue the work for the end of a turn. A queued job keeps the core alive.
    fn submit(self: &Arc<Self>, turn: TurnEnd) {
        let mut worker = lock(&self.worker);
        if worker.is_none() {
            let (tx, rx) = channel::<Job>();
            let spawned = thread::Builder::new()
                .name("aivoryd-code".into())
                .spawn(move || {
                    while let Ok((core, t)) = rx.recv() {
                        core.handle_turn_end(&t);
                    }
                });
            match spawned {
                Ok(_) => *worker = Some(tx),
                Err(e) => {
                    eprintln!("aivoryd: cannot start the code thread: {e}");
                    return;
                }
            }
        }
        if let Some(tx) = worker.as_ref() {
            let _ = tx.send((Arc::clone(self), turn));
        }
    }

    /// Update the session delta of the index, then run the diagnostics.
    fn handle_turn_end(&self, t: &TurnEnd) {
        if t.n == 0 || t.files.is_empty() {
            return;
        }
        let name = t
            .workdir
            .file_name()
            .map_or_else(|| "repo".to_owned(), |n| n.to_string_lossy().into_owned());
        let entry = match self.entry(&t.workdir, &name) {
            Ok(entry) => {
                if entry.wait_ready(READY_WAIT) {
                    let removed: Vec<String> = t
                        .files
                        .iter()
                        .filter(|f| !t.workdir.join(f).is_file())
                        .cloned()
                        .collect();
                    if let Err(e) = entry
                        .index
                        .session_update(&t.session, None, &t.workdir, &t.files, &removed)
                    {
                        eprintln!("aivoryd: session delta of {}: {e}", t.session);
                    }
                }
                Some(entry)
            }
            Err(e) => {
                eprintln!("aivoryd: {e}");
                None
            }
        };
        // The claims, the conflict radar, and the mail of the coordination. Tier 3 of the radar
        // reads the references of the index delta that is up to date now.
        crate::coord::hub_for(&self.db).turn_ended(
            t,
            entry
                .as_ref()
                .map(|e| &e.index as &dyn aivory_coord::RefSource),
        );
        if let Err(e) = super::diag::run_turn(self, t) {
            eprintln!("aivoryd: diagnostics of {}/{}: {e}", t.session, t.n);
        }
    }
}

impl Drop for CodeCore {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.pool.stop_all();
    }
}

/// A folder name from a codebase id.
fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() || s.chars().all(|c| c == '.') {
        "repo".to_owned()
    } else {
        s
    }
}

fn short_hash(text: &str) -> u32 {
    // FNV-1a.
    let mut h: u32 = 0x811c_9dc5;
    for b in text.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(safe_name("aivory-code"), "aivory-code");
        assert_eq!(safe_name("a/b c"), "a_b_c");
        assert_eq!(safe_name(".."), "repo");
        assert_eq!(safe_name(""), "repo");
    }
}
