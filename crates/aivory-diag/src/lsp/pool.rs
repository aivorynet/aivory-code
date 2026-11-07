//! The LSP pool: one language server per repo lineage, started on demand, stopped when idle or
//! when the memory cap is reached.
//!
//! - A **lineage** is a repo plus its worktrees. The server runs in the root of the lineage.
//! - **Overlay swap:** a query carries the content of the session's changed files. Before the
//!   query the pool sends `didOpen`, `didChange`, and `didClose` so that the server sees exactly
//!   the files of that session. One query runs at a time per server.
//! - **Idle stop:** [`LspPool::tick`] stops a server after `idle_timeout` (15 minutes) without a
//!   query. The daemon calls `tick` from a timer.
//! - **Memory cap:** `tick` and each server start check the memory of the servers (and their child
//!   processes). While the total is above the cap, the least recently used server stops first.
//!
//! The pool never writes files. `rename` returns the edits, and the caller applies them.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use aivory_syntax::Language;
use lsp_types::{
    CodeActionOrCommand, CodeActionProviderCapability, Diagnostic, DocumentChangeOperation,
    DocumentChanges, GotoDefinitionResponse, Hover, HoverContents, MarkedString, OneOf, Position,
    Range, TextEdit, WorkspaceEdit,
};
use serde_json::{Value, json};

use super::client::{Client, Launch};
use super::servers::{ServerSpec, known_servers, language_id};
use super::uri::{path_to_uri, relative_to, uri_to_path};
use crate::error::{Error, Result};
use crate::linters::process_path;

/// A repo and its worktrees. One server serves the whole lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lineage {
    /// A stable id, for example the path of the main worktree.
    pub id: String,
    /// The folder where the server starts and where the paths of the queries start.
    pub root: PathBuf,
}

/// The content of one file in a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayFile {
    /// The path relative to the root of the session, with `/` as separator.
    pub path: String,
    pub text: String,
}

/// The files that a session changed, with their content.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Overlay {
    pub session: String,
    pub files: Vec<OverlayFile>,
}

impl Overlay {
    pub fn new(session: &str, files: Vec<OverlayFile>) -> Self {
        Self {
            session: session.to_owned(),
            files,
        }
    }
}

/// The context of a query: the lineage and the overlay of the session.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    pub lineage: &'a Lineage,
    pub overlay: &'a Overlay,
}

/// A place in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// Relative to the root of the lineage when the file lies under it.
    pub path: String,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HoverInfo {
    /// Markdown text.
    pub text: String,
    pub range: Option<Range>,
}

/// The edits of one file. Positions count UTF-16 units, as in the LSP.
#[derive(Debug, Clone, PartialEq)]
pub struct FileEdit {
    pub path: String,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileDiagnostics {
    pub file: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// One semantic token, decoded with the legend of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticToken {
    pub line: u32,
    pub start: u32,
    pub length: u32,
    pub token_type: String,
    pub modifiers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeActionItem {
    pub title: String,
    pub kind: Option<String>,
    pub preferred: bool,
    /// The edits of the action. Empty for a command-only action.
    pub edits: Vec<FileEdit>,
    /// True when the action also runs a server command (the pool does not run it).
    pub has_command: bool,
}

/// A running server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub lineage: String,
    pub server: String,
    pub pid: u32,
    pub memory_bytes: u64,
    pub idle: Duration,
    pub queries: u64,
}

/// What `tick` did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TickReport {
    /// `(lineage, server)` of the servers stopped for being idle.
    pub idle_stopped: Vec<(String, String)>,
    /// `(lineage, server)` of the servers stopped for the memory cap, in the order of stopping.
    pub evicted: Vec<(String, String)>,
}

/// Reads the memory of processes. The pool uses [`SysinfoProbe`]; tests use a fake.
pub trait MemoryProbe: Send + Sync + std::fmt::Debug {
    /// The memory in bytes of each pid, including its child processes.
    fn sample(&self, pids: &[u32]) -> HashMap<u32, u64>;
}

/// Reads the memory with the `sysinfo` crate.
#[derive(Debug, Default)]
pub struct SysinfoProbe;

impl MemoryProbe for SysinfoProbe {
    fn sample(&self, pids: &[u32]) -> HashMap<u32, u64> {
        use sysinfo::{ProcessesToUpdate, System};
        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::All);
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, p) in sys.processes() {
            if let Some(parent) = p.parent() {
                children
                    .entry(parent.as_u32())
                    .or_default()
                    .push(pid.as_u32());
            }
        }
        let mut out = HashMap::new();
        for &root in pids {
            let mut total = 0u64;
            let mut stack = vec![root];
            let mut seen = 0;
            while let Some(pid) = stack.pop() {
                seen += 1;
                if seen > 10_000 {
                    break;
                }
                if let Some(p) = sys.process(sysinfo::Pid::from_u32(pid)) {
                    total += p.memory();
                }
                if let Some(c) = children.get(&pid) {
                    stack.extend(c);
                }
            }
            out.insert(root, total);
        }
        out
    }
}

/// The total memory of this machine in bytes.
pub fn total_memory() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.total_memory()
}

/// The settings of a pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// A server without a query for this long stops. Default: 15 minutes.
    pub idle_timeout: Duration,
    /// The total memory of all servers. Default: 25 percent of the RAM.
    pub memory_cap_bytes: u64,
    pub start_timeout: Duration,
    pub request_timeout: Duration,
    /// How long `diagnostics` waits for the server to publish after a change.
    pub diagnostics_wait: Duration,
    /// The `PATH` where the servers are searched. Default: the `PATH` of the process.
    pub path_var: OsString,
    /// A folder for the data of servers that need one (jdtls `-data`).
    pub data_dir: PathBuf,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(15 * 60),
            memory_cap_bytes: total_memory() / 4,
            start_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
            diagnostics_wait: Duration::from_secs(3),
            path_var: process_path(),
            data_dir: std::env::temp_dir().join("aivory-lsp"),
        }
    }
}

type Key = (String, String);

struct Meta {
    last_used: Instant,
    queries: u64,
    /// The pid while the server runs.
    pid: Option<u32>,
}

struct Slot {
    key: Key,
    spec: ServerSpec,
    root: PathBuf,
    meta: Mutex<Meta>,
    server: Mutex<Option<Server>>,
}

struct DocState {
    version: i32,
    hash: u64,
}

struct Server {
    client: Client,
    docs: HashMap<String, DocState>,
}

fn text_hash(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Server {
    /// Make the open documents of the server equal to the overlay plus `extra` files (read from
    /// disk when the overlay lacks them). Returns the changed documents with the number of
    /// diagnostics publishes seen before the change.
    fn sync(
        &mut self,
        root: &Path,
        overlay: &Overlay,
        extra: &[&str],
    ) -> Result<Vec<(String, u64)>> {
        let mut desired: BTreeMap<String, (String, String)> = BTreeMap::new();
        for f in &overlay.files {
            let uri = path_to_uri(&root.join(&f.path));
            desired.insert(uri, (f.path.clone(), f.text.clone()));
        }
        for rel in extra {
            let uri = path_to_uri(&root.join(rel));
            if let std::collections::btree_map::Entry::Vacant(slot) = desired.entry(uri) {
                let text = std::fs::read_to_string(root.join(rel))?;
                slot.insert(((*rel).to_owned(), text));
            }
        }
        let mut changed = Vec::new();
        for (uri, (rel, text)) in &desired {
            let hash = text_hash(text);
            match self.docs.get_mut(uri) {
                None => {
                    changed.push((uri.clone(), self.client.diag_epoch(uri)));
                    let lang = Language::detect(rel).map_or("plaintext", language_id);
                    self.client.notify(
                        "textDocument/didOpen",
                        json!({"textDocument": {"uri": uri, "languageId": lang, "version": 1, "text": text}}),
                    )?;
                    self.docs.insert(uri.clone(), DocState { version: 1, hash });
                }
                Some(doc) if doc.hash != hash => {
                    changed.push((uri.clone(), self.client.diag_epoch(uri)));
                    doc.version += 1;
                    doc.hash = hash;
                    self.client.notify(
                        "textDocument/didChange",
                        json!({"textDocument": {"uri": uri, "version": doc.version},
                               "contentChanges": [{"text": text}]}),
                    )?;
                }
                Some(_) => {}
            }
        }
        let stale: Vec<String> = self
            .docs
            .keys()
            .filter(|u| !desired.contains_key(*u))
            .cloned()
            .collect();
        for uri in stale {
            self.docs.remove(&uri);
            self.client.notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": uri}}),
            )?;
        }
        Ok(changed)
    }
}

/// The pool of language servers.
pub struct LspPool {
    cfg: PoolConfig,
    specs: Vec<ServerSpec>,
    probe: Arc<dyn MemoryProbe>,
    slots: Mutex<HashMap<Key, Arc<Slot>>>,
}

impl std::fmt::Debug for LspPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspPool")
            .field("cfg", &self.cfg)
            .field("servers", &self.specs.len())
            .finish()
    }
}

impl Default for LspPool {
    fn default() -> Self {
        Self::new(
            PoolConfig::default(),
            known_servers(),
            Arc::new(SysinfoProbe),
        )
    }
}

impl LspPool {
    pub fn new(cfg: PoolConfig, specs: Vec<ServerSpec>, probe: Arc<dyn MemoryProbe>) -> Self {
        Self {
            cfg,
            specs,
            probe,
            slots: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &PoolConfig {
        &self.cfg
    }

    /// The server spec for a language, when the pool has one and its binary is installed.
    pub fn available(&self, language: Language) -> Option<&ServerSpec> {
        self.specs
            .iter()
            .find(|s| s.serves(language) && s.find(&self.cfg.path_var).is_some())
    }

    /// True when a server for the language runs for the lineage now. The pipeline uses this to
    /// collect LSP diagnostics only from servers that already run.
    pub fn is_running(&self, lineage: &Lineage, language: Language) -> bool {
        let Some(spec) = self.specs.iter().find(|s| s.serves(language)) else {
            return false;
        };
        let slots = lock(&self.slots);
        slots
            .get(&(lineage.id.clone(), spec.id.clone()))
            .is_some_and(|s| lock(&s.meta).pid.is_some())
    }

    /// The servers that run now.
    pub fn running(&self) -> Vec<ServerInfo> {
        let slots: Vec<Arc<Slot>> = lock(&self.slots).values().cloned().collect();
        let live: Vec<(Arc<Slot>, u32, Instant, u64)> = slots
            .into_iter()
            .filter_map(|s| {
                let m = lock(&s.meta);
                m.pid.map(|pid| (s.clone(), pid, m.last_used, m.queries))
            })
            .collect();
        let pids: Vec<u32> = live.iter().map(|l| l.1).collect();
        let mem = self.probe.sample(&pids);
        let mut out: Vec<ServerInfo> = live
            .into_iter()
            .map(|(s, pid, last, queries)| ServerInfo {
                lineage: s.key.0.clone(),
                server: s.key.1.clone(),
                pid,
                memory_bytes: mem.get(&pid).copied().unwrap_or(0),
                idle: last.elapsed(),
                queries,
            })
            .collect();
        out.sort_by(|a, b| (&a.lineage, &a.server).cmp(&(&b.lineage, &b.server)));
        out
    }

    /// Stop the idle servers and enforce the memory cap. Call it from a timer.
    pub fn tick(&self) -> TickReport {
        self.tick_at(Instant::now())
    }

    /// Like [`LspPool::tick`] with an explicit "now" (for tests).
    pub fn tick_at(&self, now: Instant) -> TickReport {
        let mut report = TickReport::default();
        let slots: Vec<Arc<Slot>> = lock(&self.slots).values().cloned().collect();
        for slot in &slots {
            let (running, last) = {
                let m = lock(&slot.meta);
                (m.pid.is_some(), m.last_used)
            };
            if running
                && now.saturating_duration_since(last) >= self.cfg.idle_timeout
                && self.stop_slot(slot)
            {
                report.idle_stopped.push(slot.key.clone());
            }
        }
        report.evicted = self.enforce_memory(None);
        report
    }

    /// Stop every server.
    pub fn stop_all(&self) {
        let slots: Vec<Arc<Slot>> = lock(&self.slots).values().cloned().collect();
        for s in &slots {
            self.stop_slot(s);
        }
    }

    /// Stop the servers of one lineage (for example when the repo closes).
    pub fn stop_lineage(&self, lineage_id: &str) {
        let slots: Vec<Arc<Slot>> = lock(&self.slots)
            .values()
            .filter(|s| s.key.0 == lineage_id)
            .cloned()
            .collect();
        for s in &slots {
            self.stop_slot(s);
        }
    }

    /// Stop a server unless a query uses it now. Returns true when the server stopped.
    fn stop_slot(&self, slot: &Slot) -> bool {
        let mut guard = match slot.server.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return false,
        };
        lock(&slot.meta).pid = None;
        match guard.take() {
            Some(server) => {
                server.client.shutdown();
                true
            }
            None => false,
        }
    }

    /// Stop servers while their total memory is above the cap. The least recently used server
    /// stops first. `keep` is never stopped. Returns the keys in the order of stopping.
    fn enforce_memory(&self, keep: Option<&Key>) -> Vec<Key> {
        let slots: Vec<Arc<Slot>> = lock(&self.slots).values().cloned().collect();
        let mut live: Vec<(Arc<Slot>, u32, Instant, u64)> = slots
            .into_iter()
            .filter_map(|s| {
                let m = lock(&s.meta);
                m.pid.map(|pid| (s.clone(), pid, m.last_used, m.queries))
            })
            .collect();
        if live.is_empty() {
            return Vec::new();
        }
        let pids: Vec<u32> = live.iter().map(|l| l.1).collect();
        let mem = self.probe.sample(&pids);
        let mut total: u64 = pids.iter().map(|p| mem.get(p).copied().unwrap_or(0)).sum();
        let mut evicted = Vec::new();
        if total <= self.cfg.memory_cap_bytes {
            return evicted;
        }
        live.sort_by_key(|(s, _, last, queries)| (*last, *queries, s.key.clone()));
        for (slot, pid, _, _) in live {
            if total <= self.cfg.memory_cap_bytes {
                break;
            }
            if Some(&slot.key) == keep {
                continue;
            }
            if self.stop_slot(&slot) {
                total = total.saturating_sub(mem.get(&pid).copied().unwrap_or(0));
                evicted.push(slot.key.clone());
            }
        }
        evicted
    }

    fn slot_for(&self, lineage: &Lineage, spec: &ServerSpec) -> Arc<Slot> {
        let key = (lineage.id.clone(), spec.id.clone());
        lock(&self.slots)
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(Slot {
                    key,
                    spec: spec.clone(),
                    root: lineage.root.clone(),
                    meta: Mutex::new(Meta {
                        last_used: Instant::now(),
                        queries: 0,
                        pid: None,
                    }),
                    server: Mutex::new(None),
                })
            })
            .clone()
    }

    fn start(&self, slot: &Slot) -> Result<Server> {
        let exe = slot
            .spec
            .find(&self.cfg.path_var)
            .ok_or_else(|| Error::ServerNotFound(slot.spec.id.clone()))?;
        let data =
            self.cfg
                .data_dir
                .join(format!("{:016x}-{}", text_hash(&slot.key.0), slot.spec.id));
        let args: Vec<String> = slot
            .spec
            .args
            .iter()
            .map(|a| a.replace("{data_dir}", &data.to_string_lossy()))
            .collect();
        if args.iter().any(|a| a.contains(&*data.to_string_lossy())) {
            std::fs::create_dir_all(&data)?;
        }
        let client = Client::start(Launch {
            exe: &exe,
            args: &args,
            root: &slot.root,
            init_options: slot.spec.init_options.as_ref(),
            start_timeout: self.cfg.start_timeout,
            request_timeout: self.cfg.request_timeout,
        })?;
        Ok(Server {
            client,
            docs: HashMap::new(),
        })
    }

    /// Run `f` on the server for `language`. The server starts when it does not run. The slot
    /// lock keeps the queries of one server in a line.
    fn with_server<T>(
        &self,
        lineage: &Lineage,
        language: Language,
        f: impl FnOnce(&mut Server) -> Result<T>,
    ) -> Result<T> {
        let spec = self
            .specs
            .iter()
            .find(|s| s.serves(language))
            .ok_or_else(|| Error::NoServer(language.name().to_owned()))?;
        let slot = self.slot_for(lineage, spec);
        let mut guard = lock(&slot.server);
        if guard.as_ref().is_some_and(|s| !s.client.is_alive()) {
            *guard = None;
            lock(&slot.meta).pid = None;
        }
        if guard.is_none() {
            let server = self.start(&slot)?;
            lock(&slot.meta).pid = Some(server.client.pid);
            *guard = Some(server);
            // The new server counts too: make room by stopping the least used other servers.
            self.enforce_memory(Some(&slot.key));
        }
        {
            let mut m = lock(&slot.meta);
            m.last_used = Instant::now();
            m.queries += 1;
        }
        let server = guard
            .as_mut()
            .ok_or_else(|| Error::Server("no server".into()))?;
        let result = f(server);
        lock(&slot.meta).last_used = Instant::now();
        result
    }

    /// Run a query on the file's server after the overlay swap.
    fn query<T>(
        &self,
        ctx: &Ctx<'_>,
        file: &str,
        f: impl FnOnce(&mut Server, &str) -> Result<T>,
    ) -> Result<T> {
        let language = Language::detect(file).ok_or_else(|| Error::NoServer(file.to_owned()))?;
        let root = ctx.lineage.root.clone();
        let uri = path_to_uri(&root.join(file));
        self.with_server(ctx.lineage, language, |server| {
            server.sync(&root, ctx.overlay, &[file])?;
            f(server, &uri)
        })
    }

    fn doc_pos(uri: &str, pos: Position) -> Value {
        json!({"textDocument": {"uri": uri}, "position": {"line": pos.line, "character": pos.character}})
    }

    /// The hover text at a position.
    pub fn hover(&self, ctx: &Ctx<'_>, file: &str, pos: Position) -> Result<Option<HoverInfo>> {
        self.query(ctx, file, |server, uri| {
            let v = server
                .client
                .request("textDocument/hover", Self::doc_pos(uri, pos))?;
            let hover: Option<Hover> = serde_json::from_value(v)?;
            Ok(hover.map(|h| HoverInfo {
                text: hover_text(&h.contents),
                range: h.range,
            }))
        })
    }

    /// The definitions of the symbol at a position.
    pub fn definition(&self, ctx: &Ctx<'_>, file: &str, pos: Position) -> Result<Vec<Place>> {
        let root = ctx.lineage.root.clone();
        self.query(ctx, file, |server, uri| {
            let v = server
                .client
                .request("textDocument/definition", Self::doc_pos(uri, pos))?;
            let resp: Option<GotoDefinitionResponse> = serde_json::from_value(v)?;
            let places = match resp {
                None => Vec::new(),
                Some(GotoDefinitionResponse::Scalar(l)) => vec![place(&root, &l.uri, l.range)],
                Some(GotoDefinitionResponse::Array(a)) => a
                    .into_iter()
                    .map(|l| place(&root, &l.uri, l.range))
                    .collect(),
                Some(GotoDefinitionResponse::Link(a)) => a
                    .into_iter()
                    .map(|l| place(&root, &l.target_uri, l.target_selection_range))
                    .collect(),
            };
            Ok(places)
        })
    }

    /// The references of the symbol at a position (with the declaration).
    pub fn references(&self, ctx: &Ctx<'_>, file: &str, pos: Position) -> Result<Vec<Place>> {
        let root = ctx.lineage.root.clone();
        self.query(ctx, file, |server, uri| {
            let mut params = Self::doc_pos(uri, pos);
            params["context"] = json!({"includeDeclaration": true});
            let v = server.client.request("textDocument/references", params)?;
            let list: Option<Vec<lsp_types::Location>> = serde_json::from_value(v)?;
            Ok(list
                .unwrap_or_default()
                .into_iter()
                .map(|l| place(&root, &l.uri, l.range))
                .collect())
        })
    }

    /// The edits of a rename. The pool does not apply them.
    pub fn rename(
        &self,
        ctx: &Ctx<'_>,
        file: &str,
        pos: Position,
        new_name: &str,
    ) -> Result<Vec<FileEdit>> {
        let root = ctx.lineage.root.clone();
        self.query(ctx, file, |server, uri| {
            let mut params = Self::doc_pos(uri, pos);
            params["newName"] = json!(new_name);
            let v = server.client.request("textDocument/rename", params)?;
            let edit: Option<WorkspaceEdit> = serde_json::from_value(v)?;
            Ok(edit.map(|e| file_edits(&root, e)).unwrap_or_default())
        })
    }

    /// The semantic tokens of a file.
    pub fn semantic_tokens(&self, ctx: &Ctx<'_>, file: &str) -> Result<Vec<SemanticToken>> {
        self.query(ctx, file, |server, uri| {
            let legend = server
                .client
                .legend
                .clone()
                .ok_or_else(|| Error::Unsupported("textDocument/semanticTokens".into()))?;
            let v = server.client.request(
                "textDocument/semanticTokens/full",
                json!({"textDocument": {"uri": uri}}),
            )?;
            let data: Vec<u32> = v
                .get("data")
                .and_then(|d| serde_json::from_value(d.clone()).ok())
                .unwrap_or_default();
            let (mut line, mut start) = (0u32, 0u32);
            let mut out = Vec::new();
            for t in data.chunks_exact(5) {
                if t[0] > 0 {
                    line += t[0];
                    start = t[1];
                } else {
                    start += t[1];
                }
                let modifiers = legend
                    .token_modifiers
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i < 32 && t[4] & (1 << i) != 0)
                    .map(|(_, m)| m.as_str().to_owned())
                    .collect();
                out.push(SemanticToken {
                    line,
                    start,
                    length: t[2],
                    token_type: legend
                        .token_types
                        .get(t[3] as usize)
                        .map_or_else(|| "unknown".to_owned(), |t| t.as_str().to_owned()),
                    modifiers,
                });
            }
            Ok(out)
        })
    }

    /// The code actions for a range.
    pub fn code_actions(
        &self,
        ctx: &Ctx<'_>,
        file: &str,
        range: Range,
    ) -> Result<Vec<CodeActionItem>> {
        let root = ctx.lineage.root.clone();
        self.query(ctx, file, |server, uri| {
            let diags: Vec<Diagnostic> = server
                .client
                .diagnostics(uri)
                .into_iter()
                .filter(|d| d.range.start <= range.end && range.start <= d.range.end)
                .collect();
            let v = server.client.request(
                "textDocument/codeAction",
                json!({"textDocument": {"uri": uri}, "range": range,
                       "context": {"diagnostics": diags}}),
            )?;
            let list: Option<Vec<CodeActionOrCommand>> = serde_json::from_value(v)?;
            let can_resolve = matches!(
                &server.client.capabilities.code_action_provider,
                Some(CodeActionProviderCapability::Options(o)) if o.resolve_provider == Some(true)
            );
            let mut out = Vec::new();
            for item in list.unwrap_or_default() {
                match item {
                    CodeActionOrCommand::Command(c) => out.push(CodeActionItem {
                        title: c.title,
                        kind: None,
                        preferred: false,
                        edits: Vec::new(),
                        has_command: true,
                    }),
                    CodeActionOrCommand::CodeAction(mut a) => {
                        if a.edit.is_none()
                            && can_resolve
                            && let Ok(v) = server
                                .client
                                .request("codeAction/resolve", serde_json::to_value(&a)?)
                            && let Ok(resolved) = serde_json::from_value::<lsp_types::CodeAction>(v)
                        {
                            a = resolved;
                        }
                        out.push(CodeActionItem {
                            title: a.title,
                            kind: a.kind.map(|k| k.as_str().to_owned()),
                            preferred: a.is_preferred.unwrap_or(false),
                            edits: a.edit.map(|e| file_edits(&root, e)).unwrap_or_default(),
                            has_command: a.command.is_some(),
                        });
                    }
                }
            }
            Ok(out)
        })
    }

    /// The diagnostics of files. The pool sends the overlay, then waits until the server
    /// published for the changed files (at most `diagnostics_wait`). Files of languages without
    /// a server are left out.
    pub fn diagnostics(&self, ctx: &Ctx<'_>, files: &[String]) -> Result<Vec<FileDiagnostics>> {
        self.diagnostics_within(ctx, files, self.cfg.diagnostics_wait)
    }

    /// Like [`LspPool::diagnostics`] with an own wait time per server.
    pub fn diagnostics_within(
        &self,
        ctx: &Ctx<'_>,
        files: &[String],
        wait: Duration,
    ) -> Result<Vec<FileDiagnostics>> {
        let mut groups: BTreeMap<String, (Language, Vec<&str>)> = BTreeMap::new();
        for f in files {
            let Some(lang) = Language::detect(f) else {
                continue;
            };
            let Some(spec) = self.specs.iter().find(|s| s.serves(lang)) else {
                continue;
            };
            groups
                .entry(spec.id.clone())
                .or_insert_with(|| (lang, Vec::new()))
                .1
                .push(f);
        }
        let root = ctx.lineage.root.clone();
        let mut out = Vec::new();
        for (_, (lang, list)) in groups {
            let part = self.with_server(ctx.lineage, lang, |server| {
                let changed = server.sync(&root, ctx.overlay, &list)?;
                // Only the requested files are awaited.
                let wanted: Vec<(String, u64)> = changed
                    .into_iter()
                    .filter(|(u, _)| list.iter().any(|f| path_to_uri(&root.join(f)) == *u))
                    .collect();
                server.client.wait_diagnostics(&wanted, wait);
                Ok(list
                    .iter()
                    .map(|f| FileDiagnostics {
                        file: (*f).to_owned(),
                        diagnostics: server.client.diagnostics(&path_to_uri(&root.join(f))),
                    })
                    .collect::<Vec<_>>())
            })?;
            out.extend(part);
        }
        Ok(out)
    }
}

fn place(root: &Path, uri: &lsp_types::Uri, range: Range) -> Place {
    let path = match uri_to_path(uri.as_str()) {
        Some(p) => relative_to(root, &p),
        None => uri.as_str().to_owned(),
    };
    Place { path, range }
}

fn marked(m: &MarkedString) -> String {
    match m {
        MarkedString::String(s) => s.clone(),
        MarkedString::LanguageString(l) => format!("```{}\n{}\n```", l.language, l.value),
    }
}

fn hover_text(c: &HoverContents) -> String {
    match c {
        HoverContents::Scalar(m) => marked(m),
        HoverContents::Array(a) => a.iter().map(marked).collect::<Vec<_>>().join("\n\n"),
        HoverContents::Markup(m) => m.value.clone(),
    }
}

/// The text edits of a workspace edit, grouped by file. Resource operations (create, rename,
/// and delete a file) are not part of the result.
fn file_edits(root: &Path, edit: WorkspaceEdit) -> Vec<FileEdit> {
    let mut map: BTreeMap<String, Vec<TextEdit>> = BTreeMap::new();
    let path_of = |uri: &lsp_types::Uri| match uri_to_path(uri.as_str()) {
        Some(p) => relative_to(root, &p),
        None => uri.as_str().to_owned(),
    };
    for (uri, edits) in edit.changes.unwrap_or_default() {
        map.entry(path_of(&uri)).or_default().extend(edits);
    }
    let mut docs = Vec::new();
    match edit.document_changes {
        Some(DocumentChanges::Edits(e)) => docs.extend(e),
        Some(DocumentChanges::Operations(ops)) => {
            for op in ops {
                if let DocumentChangeOperation::Edit(e) = op {
                    docs.push(e);
                }
            }
        }
        None => {}
    }
    for d in docs {
        let list = map.entry(path_of(&d.text_document.uri)).or_default();
        for e in d.edits {
            list.push(match e {
                OneOf::Left(t) => t,
                OneOf::Right(a) => a.text_edit,
            });
        }
    }
    map.into_iter()
        .map(|(path, edits)| FileEdit { path, edits })
        .collect()
}
