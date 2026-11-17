//! JSON-RPC 2.0 methods of the daemon. Transport-free: input is one JSON line, output goes to
//! the [`Outbox`] of the connection (zero or more notifications plus at most one response).
//!
//! Methods:
//! - `daemon/info` -> [`aivory_protocol::rpc::DaemonInfo`]
//! - `daemon/shutdown` -> `{ok: true}`
//! - `sessions/list` `{provider?, cwd?, limit?}` -> `{sessions: [...], stats}`
//! - `sessions/get` `{id, provider?}` -> session summary
//! - `sessions/resume` `{id, prompt, provider?, model?, include_raw?}` streams
//!   `session/event` notifications `{request_id, provider, session_id, seq, event}`, then
//!   returns `{status, exit_code, events}`.
//! - `code/*` and `ledger/*` ([`crate::code`]): code intelligence and the turn ledger. `serve`
//!   and `serve --demo` answer them.
//! - `daemon/restart` and the other action methods of the desktop controls (`repos/add`,
//!   `project/new`, `map/addPart`, `providers/addAcp`, `settings/importClaude`): see
//!   [`aivory_protocol::actions`] and [`crate::actions`]. A paired phone cannot call them.
//! - the war-room surface of [`aivory_protocol::rpc`] (`warroom/*`, `session/*`, `inbox/*`,
//!   `tasks/*`, `repos/*`, `settings/*`, `autopilot/*`, `devices/*`). Only `serve --demo`
//!   serves it so far; without `--demo` it answers `NOT_AVAILABLE`.
//!
//! Batch requests are not supported. A connection handles one request at a time; open a
//! second connection for parallel work.

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};

use aivory_agents::{AgentEvent, ResumeRequest, RunOptions, RunOutcome, run_resume};
pub use aivory_protocol::rpc::RpcError;
pub use aivory_protocol::rpc::code::*;
use aivory_protocol::rpc::{DaemonInfo, Empty};
use aivory_protocol::{DaemonKind, DaemonRef};
use aivory_sessions::{FindError, Provider, SessionFilter, SessionSummary};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::Config;
use crate::demo::DemoBackend;
use crate::live::{LiveBackend, LiveOptions};
use crate::outbox::Outbox;

fn find_error(e: FindError) -> RpcError {
    match &e {
        FindError::NotFound(_) => RpcError::new(SESSION_NOT_FOUND, e.to_string()),
        FindError::Ambiguous { candidates, .. } => RpcError {
            code: SESSION_AMBIGUOUS,
            message: e.to_string(),
            data: Some(
                json!({ "candidates": candidates.iter().map(|c| json!({"provider": c.provider, "id": c.id})).collect::<Vec<_>>() }),
            ),
        },
    }
}

#[derive(Debug, Deserialize)]
struct Request {
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: Option<String>,
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ListParams {
    provider: Option<Provider>,
    cwd: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct GetParams {
    id: String,
    provider: Option<Provider>,
}

/// Parameters of `sessions/resume`.
#[derive(Debug, Clone, Deserialize)]
pub struct ResumeParams {
    pub id: String,
    pub prompt: String,
    pub provider: Option<Provider>,
    pub model: Option<String>,
    /// Extra CLI arguments. The command line accepts them; `sessions/resume` rejects them.
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub include_raw: bool,
}

/// Callback for events of a resumed session: summary, sequence number, event.
pub type EventSink<'a> = &'a mut dyn FnMut(&SessionSummary, u64, AgentEvent) -> ControlFlow<()>;

#[derive(Debug)]
pub struct Daemon {
    config: Config,
    shutdown: AtomicBool,
    /// `daemon/restart` ran: `main` starts a new process after the server ends.
    restart: AtomicBool,
    demo: Option<DemoBackend>,
    live: Option<std::sync::Arc<LiveBackend>>,
    /// Paired phones of the remote endpoint; set by `serve --remote` (demo or live).
    pairings: Option<std::sync::Arc<crate::remote::Pairings>>,
    /// `peers/*`: the machine mesh (ADR 0026); set by `serve --iroh`.
    mesh: Option<std::sync::Arc<crate::mesh::Mesh>>,
    /// `peers/share*`: live file sync of the mesh.
    files: Option<std::sync::Arc<crate::mesh_sync::FileSync>>,
    /// `code/*` and `ledger/*` (demo or live).
    code: Option<std::sync::Arc<crate::code::CodeService>>,
    /// `resources/memory`: the RAM samples of the machine and the agents.
    resources: crate::resources::Sampler,
    /// `theme/*` and `registry/*` (ADR 0024).
    themes: std::sync::Arc<crate::theme::ThemeService>,
    /// Connections that get `theme/changed` (they called `theme/sync` or `theme/list`).
    theme_subs: std::sync::Arc<std::sync::Mutex<Vec<Outbox>>>,
}

impl Daemon {
    /// The live backend (`serve` without `--demo`). The mesh merges peer sessions into it.
    pub fn live_backend(&self) -> Option<&std::sync::Arc<LiveBackend>> {
        self.live.as_ref()
    }

    pub fn new(config: Config) -> Self {
        let themes = std::sync::Arc::new(crate::theme::ThemeService::new(
            crate::theme::Store::for_cache_file(config.cache_file.as_deref()),
            std::sync::Arc::new(crate::theme::HttpFetcher::new()),
        ));
        let theme_subs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        Self::hook_themes(&themes, &theme_subs);
        Self {
            themes,
            theme_subs,
            config,
            shutdown: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            demo: None,
            live: None,
            pairings: None,
            mesh: None,
            files: None,
            code: None,
            resources: crate::resources::Sampler::default(),
        }
    }

    /// Replace the theme service (tests use a fake fetcher and a scratch folder).
    pub fn with_themes(mut self, themes: std::sync::Arc<crate::theme::ThemeService>) -> Self {
        Self::hook_themes(&themes, &self.theme_subs);
        self.themes = themes;
        self
    }

    /// Send `theme/changed` to the subscribers when the service reports a change.
    fn hook_themes(
        themes: &crate::theme::ThemeService,
        subs: &std::sync::Arc<std::sync::Mutex<Vec<Outbox>>>,
    ) {
        let subs = std::sync::Arc::clone(subs);
        themes.set_notifier(std::sync::Arc::new(move |note| {
            Self::broadcast_themes(&subs, &note);
        }));
    }

    fn broadcast_themes(
        subs: &std::sync::Mutex<Vec<Outbox>>,
        note: &aivory_protocol::theme::ThemeChanged,
    ) {
        let message = json!({
            "jsonrpc": "2.0",
            "method": aivory_protocol::rpc::THEME_CHANGED,
            "params": note,
        });
        let mut subs = subs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        subs.retain(|o| o.try_send(message.clone()));
    }

    /// Register a connection for `theme/changed` (once).
    pub(crate) fn theme_subscribe(&self, out: &Outbox) {
        let mut subs = self
            .theme_subs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !subs.iter().any(|o| o.same_channel(out)) {
            subs.push(out.clone());
        }
    }

    /// A daemon that serves the war room on the native sessions of this machine.
    /// Let `devices/pair` and `devices/remove` use the remote endpoint.
    pub fn with_pairings(mut self, pairings: std::sync::Arc<crate::remote::Pairings>) -> Self {
        self.pairings = Some(pairings);
        self
    }

    /// Let `peers/share*` use the file sync of the mesh.
    pub fn with_files(mut self, files: std::sync::Arc<crate::mesh_sync::FileSync>) -> Self {
        self.files = Some(files);
        self
    }

    /// Let `peers/*` use the machine mesh.
    pub fn with_mesh(mut self, mesh: std::sync::Arc<crate::mesh::Mesh>) -> Self {
        self.mesh = Some(mesh);
        self
    }

    pub fn with_live(config: Config, options: LiveOptions) -> Self {
        // The code service shares the state database with the turn ledger. Without a state
        // database it uses a scratch folder (no ledger rows, an index in the temp folder).
        let db = options.store.clone().unwrap_or_else(|| {
            std::env::temp_dir()
                .join(format!("aivoryd-code-{}", std::process::id()))
                .join("aivory.db")
        });
        let live = LiveBackend::start(config.catalog(), options);
        let core = crate::code::core_for(&db);
        let resolver: std::sync::Arc<dyn crate::code::Resolver> = std::sync::Arc::new(
            crate::code::LiveResolver::new(std::sync::Arc::clone(&live), config.clone()),
        );
        // The live Claude runs reach the code tools and the coordination tools (`claim`, `ping`,
        // `subscribe`) through their MCP endpoint.
        let code_tools = crate::code::CodeTools::new(
            std::sync::Arc::clone(&core),
            std::sync::Arc::clone(&resolver),
        );
        let coord_tools = crate::coord::CoordTools::new(crate::coord::hub_for(&db));
        aivory_agents::mcp::set_default_tools(Some(std::sync::Arc::new(
            crate::coord::ToolSet::new(
                std::sync::Arc::new(code_tools),
                std::sync::Arc::new(coord_tools),
            ),
        )));
        Self {
            live: Some(live),
            code: Some(std::sync::Arc::new(crate::code::CodeService::live(
                core, resolver,
            ))),
            ..Self::new(config)
        }
    }

    /// A daemon that serves the war-room surface on the demo data.
    pub fn with_demo(config: Config) -> Self {
        Self {
            demo: Some(DemoBackend::new()),
            code: Some(std::sync::Arc::new(crate::code::CodeService::demo())),
            ..Self::new(config)
        }
    }

    /// The theme service of this daemon.
    pub fn themes(&self) -> &std::sync::Arc<crate::theme::ThemeService> {
        &self.themes
    }

    /// Call a method of the war-room engine (live or demo), or `None` without one.
    pub(crate) fn war_call(
        &self,
        method: &str,
        params: Value,
        id: &Value,
        out: &Outbox,
    ) -> Option<Result<Option<Value>, RpcError>> {
        match (&self.live, &self.demo) {
            (Some(live), _) => Some(live.dispatch(method, params, id, out)),
            (None, Some(demo)) => Some(demo.dispatch(method, params, id, out)),
            _ => None,
        }
    }

    pub fn is_demo(&self) -> bool {
        self.demo.is_some()
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn is_shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// True after `daemon/restart`: the server ends, and the caller starts the new process
    /// with [`crate::actions::spawn_restart`].
    pub fn is_restart_requested(&self) -> bool {
        self.restart.load(Ordering::SeqCst)
    }

    pub fn list(&self, p: &ListFilter) -> (Vec<SessionSummary>, aivory_sessions::ScanStats) {
        let report = self.config.catalog().scan();
        let filter = SessionFilter {
            provider: p.provider,
            cwd: p.cwd.clone(),
            limit: p.limit,
        };
        let mut sessions: Vec<SessionSummary> = report
            .sessions
            .into_iter()
            .filter(|s| filter.matches(s))
            .collect();
        if let Some(limit) = p.limit {
            sessions.truncate(limit);
        }
        (sessions, report.stats)
    }

    pub fn get(&self, id: &str, provider: Option<Provider>) -> Result<SessionSummary, RpcError> {
        self.config.catalog().find(id, provider).map_err(find_error)
    }

    /// Resume a session. Calls `on_event` for every event; `Break` kills the CLI.
    pub fn resume(
        &self,
        p: &ResumeParams,
        on_event: EventSink<'_>,
    ) -> Result<(SessionSummary, RunOutcome), RpcError> {
        let summary = self.get(&p.id, p.provider)?;
        let cwd = summary.cwd.clone().ok_or_else(|| {
            RpcError::new(
                SESSION_HAS_NO_CWD,
                "the session has no known working directory",
            )
        })?;
        let req = ResumeRequest {
            provider: summary.provider,
            session_id: summary.id.clone(),
            cwd: cwd.into(),
            prompt: p.prompt.clone(),
            session_path: Some(summary.path.clone()),
            model: p.model.clone(),
            extra_args: p.extra_args.clone(),
            program: None,
        };
        let mut seq = 0u64;
        let outcome = run_resume(
            &req,
            &RunOptions {
                include_raw: p.include_raw,
            },
            &mut |event| {
                seq += 1;
                on_event(&summary, seq, event)
            },
        )
        .map_err(|e| RpcError::new(SERVER_ERROR, e.to_string()))?;
        Ok((summary, outcome))
    }

    /// Handle one request line. Notifications go to `out`. Returns the response, or `None` for
    /// a notification request and for a response that went to `out` already.
    pub fn handle_line(&self, line: &str, out: &Outbox) -> Option<Value> {
        let req: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                return Some(error_response(
                    &Value::Null,
                    &RpcError::new(PARSE_ERROR, format!("parse error: {e}")),
                ));
            }
        };
        let id = req.id.clone().unwrap_or(Value::Null);
        let Some(method) = req
            .method
            .as_deref()
            .filter(|_| req.jsonrpc.as_deref() == Some("2.0"))
        else {
            return Some(error_response(
                &id,
                &RpcError::new(
                    INVALID_REQUEST,
                    "expected {\"jsonrpc\":\"2.0\",\"method\":...}",
                ),
            ));
        };
        let params = req.params.unwrap_or(Value::Null);
        let selects = method == "settings/set"
            && matches!(
                params["key"].as_str(),
                Some("display.look" | "display.customTheme")
            );
        let result = self.dispatch(method, params, &id, out);
        if selects && result.is_ok() {
            self.themes
                .notify(aivory_protocol::theme::ThemeChangeReason::Select, None);
        }
        req.id.as_ref()?;
        match result {
            Ok(Some(v)) => Some(json!({ "jsonrpc": "2.0", "id": id, "result": v })),
            Ok(None) => None,
            Err(e) => Some(error_response(&id, &e)),
        }
    }

    fn dispatch(
        &self,
        method: &str,
        params: Value,
        id: &Value,
        out: &Outbox,
    ) -> Result<Option<Value>, RpcError> {
        if LOCAL_ONLY.contains(&method)
            && !matches!(
                crate::remote::current_caller(),
                crate::remote::Caller::Local
            )
        {
            return Err(RpcError::new(
                NOT_AVAILABLE,
                format!("'{method}' is only for the local client, not for a paired phone"),
            ));
        }
        match method {
            "daemon/restart" => return self.restart(params).map(Some),
            "settings/importClaude" => return self.import_claude(params).map(Some),
            _ => {}
        }
        if let crate::remote::Caller::Peer(peer) = crate::remote::current_caller()
            && !aivory_protocol::mesh::PEER_METHODS.contains(&method)
        {
            return Err(RpcError::new(
                NOT_AVAILABLE,
                format!("'{method}' is not open to peer {peer}"),
            ));
        }
        if method == "machines/list" {
            let _: Empty = parse_params(params)?;
            return Ok(Some(json!(self.machines(id, out))));
        }
        if method.starts_with("peers/") {
            return self.peers(method, params).map(Some);
        }
        if let Some(mesh) = &self.mesh
            && matches!(
                crate::remote::current_caller(),
                crate::remote::Caller::Local
            )
            && let Some(result) = crate::mesh_host::route(mesh, method, &params)
        {
            return result.map(Some).map_err(|e| RpcError::new(SERVER_ERROR, e));
        }
        if let Some(pairings) = &self.pairings {
            match method {
                "devices/pair" => {
                    return serde_json::to_value(pairings.pair())
                        .map(Some)
                        .map_err(|e| RpcError::new(SERVER_ERROR, e.to_string()));
                }
                "devices/remove" => {
                    let p: aivory_protocol::rpc::DeviceRef = parse_params(params)?;
                    return if pairings.remove(&p.id) {
                        Ok(Some(json!({ "ok": true })))
                    } else {
                        Err(RpcError::not_found("device", &p.id))
                    };
                }
                "devices/push" => {
                    let p: crate::push::RegisterParams = parse_params(params)?;
                    let caller = crate::remote::current_caller();
                    return crate::push::register(pairings, &caller, p)
                        .map(|()| Some(json!({ "ok": true })));
                }
                _ => {}
            }
        }
        if method == "devices/push" {
            // Only reached without pairings (the arm above returns).
            return Err(RpcError::new(
                NOT_AVAILABLE,
                "'devices/push' needs paired phones; start the daemon with `--remote` or `--iroh`",
            ));
        }
        if crate::code::handles(method) {
            return match &self.code {
                Some(code) => {
                    // The demo ledger has no worktree: the revert is a simulation, and the
                    // session shows a line for it.
                    let reverted = (method == "ledger/revert")
                        .then(|| {
                            let session = params.get("session")?.as_str()?.to_string();
                            let turn = u32::try_from(params.get("turn")?.as_u64()?).ok()?;
                            Some((session, turn))
                        })
                        .flatten();
                    let result = code.dispatch(method, params).map(Some)?;
                    if let (Some(demo), Some((session, turn))) = (&self.demo, reverted) {
                        demo.note_revert(&session, turn);
                    }
                    Ok(result)
                }
                None => Err(RpcError::new(
                    NOT_AVAILABLE,
                    format!(
                        "'{method}' needs the code service; start the daemon with `aivoryd serve`"
                    ),
                )),
            };
        }
        if crate::theme::rpc::handles(method) {
            return crate::theme::rpc::dispatch(self, method, params, id, out);
        }
        if method == "resources/memory" {
            let _: Empty = parse_params(params)?;
            let now = chrono::Utc::now().timestamp_millis();
            return Ok(Some(json!(self.resources.memory(now))));
        }
        if method == "organizer/start" {
            return self.organizer_start(params, id, out);
        }
        if method == "compose/complete" {
            return self.compose_complete(params, id, out).map(Some);
        }
        // A Codex custom prompt (`/prompts:<name>`): the app server does not expand it.
        let params = if method == "session/send" {
            self.expand_codex_prompt(params, id, out)
        } else {
            params
        };
        if DemoBackend::handles(method) {
            if let Some(live) = &self.live {
                return live.dispatch(method, params, id, out);
            }
            return match &self.demo {
                Some(demo) => demo.dispatch(method, params, id, out),
                None => Err(RpcError::new(
                    NOT_AVAILABLE,
                    format!(
                        "'{method}' needs the war-room engine; start the daemon with `aivoryd serve --demo`"
                    ),
                )),
            };
        }
        self.dispatch_core(method, params, id, out).map(Some)
    }

    /// `compose/complete`: the agent and the folder come from the session (from the war room),
    /// else from the params. Without a war-room engine the list has the commands only.
    fn compose_complete(&self, params: Value, id: &Value, out: &Outbox) -> Result<Value, RpcError> {
        let p: aivory_protocol::complete::CompleteParams = parse_params(params)?;
        let snapshot = match (&self.live, &self.demo) {
            (Some(live), _) => live
                .dispatch("warroom/snapshot", json!({}), id, out)
                .ok()
                .flatten(),
            (None, Some(demo)) => demo
                .dispatch("warroom/snapshot", json!({}), id, out)
                .ok()
                .flatten(),
            _ => None,
        };
        let war: Option<aivory_protocol::WarRoom> =
            snapshot.and_then(|v| serde_json::from_value(v).ok());
        let pane = p
            .session
            .as_deref()
            .and_then(|s| war.as_ref().and_then(|w| w.session(s)).cloned());
        let provider = pane
            .as_ref()
            .map(|p| p.provider.clone())
            .or_else(|| p.provider.clone())
            .unwrap_or_else(|| "claude".to_string());
        let cwd = pane
            .as_ref()
            .and_then(|pane| {
                war.as_ref()?
                    .codebase(&pane.codebase)
                    .map(|c| c.path.clone())
            })
            .or_else(|| p.cwd.clone())
            .map(std::path::PathBuf::from)
            .filter(|c| c.is_dir());
        let roots = crate::complete::Roots {
            claude: &self.config.roots.claude,
            codex: &self.config.roots.codex,
            gemini: &self.config.roots.gemini,
        };
        Ok(json!(crate::complete::list(
            &p,
            &provider,
            cwd.as_deref(),
            &roots
        )))
    }

    /// The text of a `session/send` to a Codex session with `/prompts:<name>` expanded.
    fn expand_codex_prompt(&self, mut params: Value, id: &Value, out: &Outbox) -> Value {
        let Some(text) = params["text"]
            .as_str()
            .filter(|t| t.trim_start().starts_with("/prompts:"))
        else {
            return params;
        };
        let Some(session) = params["id"].as_str() else {
            return params;
        };
        let snapshot = match (&self.live, &self.demo) {
            (Some(live), _) => live
                .dispatch("warroom/snapshot", json!({}), id, out)
                .ok()
                .flatten(),
            _ => None,
        };
        let codex = snapshot
            .and_then(|v| serde_json::from_value::<aivory_protocol::WarRoom>(v).ok())
            .and_then(|w| w.session(session).map(|p| p.provider == "codex"))
            .unwrap_or(false);
        if codex
            && let Some(full) = crate::complete::expand_codex_prompt(text, &self.config.roots.codex)
        {
            params["text"] = json!(full);
        }
        params
    }

    /// `organizer/start`: a `session/new` in the first codebase (or the named one) with the
    /// organizer prompt of the mode. The agent calls this daemon through `aivoryd call`.
    fn organizer_start(
        &self,
        params: Value,
        id: &Value,
        out: &Outbox,
    ) -> Result<Option<Value>, RpcError> {
        let p: aivory_protocol::OrganizerStartParams = parse_params(params)?;
        let war = |m: &str, v: Value| -> Result<Option<Value>, RpcError> {
            match (&self.live, &self.demo) {
                (Some(live), _) => live.dispatch(m, v, id, out),
                (None, Some(demo)) => demo.dispatch(m, v, id, out),
                _ => Err(RpcError::new(
                    NOT_AVAILABLE,
                    "'organizer/start' needs the war-room engine".to_string(),
                )),
            }
        };
        let codebase = match p.codebase {
            Some(c) => c,
            None => war("warroom/snapshot", json!({}))?
                .and_then(|v| v["codebases"][0]["id"].as_str().map(str::to_string))
                .ok_or_else(|| RpcError::new(INVALID_STATE, "no codebase".to_string()))?,
        };
        let exe = crate::autostart::daemon_exe()
            .to_string_lossy()
            .into_owned();
        let new = aivory_protocol::rpc::NewSessionParams {
            codebase,
            part: None,
            title: Some(format!("Organizer · {}", p.mode.label())),
            prompt: crate::resources::organizer_prompt(p.mode, &exe),
            composer: p.composer.unwrap_or(aivory_protocol::ComposerPrefs {
                mode: aivory_protocol::Mode::Auto,
                provider: "claude".to_string(),
                model: String::new(),
                effort: aivory_protocol::Effort::Medium,
            }),
            task: None,
            attachments: Vec::new(),
            node: None,
        };
        war(
            "session/new",
            serde_json::to_value(new).unwrap_or(Value::Null),
        )
    }

    fn dispatch_core(
        &self,
        method: &str,
        params: Value,
        id: &Value,
        out: &Outbox,
    ) -> Result<Value, RpcError> {
        match method {
            "daemon/info" => {
                let _: Empty = parse_params(params)?;
                Ok(json!(DaemonInfo {
                    name: "aivoryd".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    protocol: "jsonrpc-2.0/ndjson".to_string(),
                    contract: aivory_protocol::PROTOCOL_VERSION.to_string(),
                    pid: std::process::id(),
                    demo: self.is_demo(),
                    host: crate::actions::host_name(),
                    cwd: crate::actions::working_dir(),
                }))
            }
            "daemon/shutdown" => {
                self.shutdown.store(true, Ordering::SeqCst);
                Ok(json!({ "ok": true }))
            }
            "media/get" => {
                let p: aivory_protocol::media::MediaGetParams = parse_params(params)?;
                Ok(json!(media_get(&p.src)?))
            }
            "media/put" => {
                let p: aivory_protocol::media::MediaPutParams = parse_params(params)?;
                let dir = aivory_sessions::cache_dir()
                    .map(|d| d.join("uploads"))
                    .ok_or_else(|| {
                        RpcError::new(
                            aivory_protocol::rpc::code::SERVER_ERROR,
                            "no cache folder".to_string(),
                        )
                    })?;
                Ok(json!(media_put(&dir, &p)?))
            }
            "sessions/list" => {
                let p: ListParams = parse_params(params)?;
                let (sessions, stats) = self.list(&ListFilter {
                    provider: p.provider,
                    cwd: p.cwd,
                    limit: p.limit,
                });
                Ok(json!({ "sessions": sessions, "stats": stats }))
            }
            "sessions/get" => {
                let p: GetParams = parse_params(params)?;
                Ok(serde_json::to_value(self.get(&p.id, p.provider)?).unwrap_or(Value::Null))
            }
            "sessions/resume" => {
                let p: ResumeParams = parse_params(params)?;
                if !p.extra_args.is_empty() {
                    // A client must not choose CLI flags such as a permission bypass.
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        "extra_args is only available on the command line, not over RPC",
                    ));
                }
                let (_, outcome) = self.resume(&p, &mut |summary, seq, event| {
                    let note = json!({
                        "jsonrpc": "2.0",
                        "method": "session/event",
                        "params": { "request_id": id, "provider": summary.provider, "session_id": summary.id, "seq": seq, "event": event },
                    });
                    if out.send(note).is_ok() { ControlFlow::Continue(()) } else { ControlFlow::Break(()) }
                })?;
                let status = if outcome.cancelled {
                    "cancelled"
                } else if outcome.completed {
                    "completed"
                } else {
                    "failed"
                };
                Ok(
                    json!({ "status": status, "exit_code": outcome.exit_code, "events": outcome.events }),
                )
            }
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("unknown method '{other}'"),
            )),
        }
    }
}

/// The methods that change the machine or its files: a paired phone cannot call them.
const LOCAL_ONLY: &[&str] = &[
    "peers/invite",
    "peers/join",
    "peers/list",
    "peers/check",
    "peers/remove",
    "peers/call",
    "peers/share",
    "peers/unshare",
    "peers/shares",
    "peers/vcs",
    "repos/add",
    "repos/remove",
    "project/new",
    "map/addPart",
    "providers/addAcp",
    "providers/removeAcp",
    "settings/importClaude",
    "daemon/restart",
    "theme/inspect",
    "theme/add",
    "theme/remove",
    "theme/checkUpdates",
    "theme/update",
    "theme/diff",
    "theme/job",
    "registry/add",
    "registry/remove",
    "registry/refresh",
];

impl Daemon {
    /// `machines/list`: this machine, then the mesh peers, each with its look.
    fn machines(&self, id: &Value, out: &Outbox) -> aivory_protocol::mesh::MachineList {
        use aivory_protocol::mesh::Machine;
        let look = self
            .war_call("settings/get", json!({}), id, out)
            .and_then(Result::ok)
            .flatten()
            .and_then(|v| {
                v["settings"]["display"]["look"]
                    .as_str()
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let node = self.mesh.as_ref().map(|m| m.node()).unwrap_or_default();
        let mut machines = vec![Machine {
            id: if node.is_empty() {
                "local".into()
            } else {
                node
            },
            name: crate::actions::host_name(),
            look,
            local: true,
            online: true,
        }];
        if let Some(mesh) = &self.mesh {
            let online = mesh.online();
            for p in mesh.list().peers {
                let name = if p.name.is_empty() {
                    p.id[..8.min(p.id.len())].to_string()
                } else {
                    p.name.clone()
                };
                machines.push(Machine {
                    online: online.contains(&p.id),
                    look: p.state.map(|s| s.look).unwrap_or_default(),
                    id: p.id,
                    name,
                    local: false,
                });
            }
        }
        aivory_protocol::mesh::MachineList { machines }
    }

    /// `peers/*`: the machine mesh (ADR 0026).
    fn peers(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        use aivory_protocol::mesh::{PeerJoinParams, PeerRef};
        let Some(mesh) = &self.mesh else {
            return Err(RpcError::new(
                NOT_AVAILABLE,
                format!(
                    "'{method}' needs the iroh link; start the daemon with `aivoryd serve --iroh`"
                ),
            ));
        };
        let failed = |e: String| RpcError::new(SERVER_ERROR, e);
        let value = match method {
            "peers/invite" => json!(mesh.invite().map_err(failed)?),
            "peers/join" => {
                let p: PeerJoinParams = parse_params(params)?;
                json!(mesh.join(&p.link).map_err(failed)?)
            }
            "peers/list" => {
                let _: Empty = parse_params(params)?;
                json!(mesh.list())
            }
            "peers/check" => {
                let p: PeerRef = parse_params(params)?;
                json!(mesh.check(&p.id).map_err(failed)?)
            }
            "peers/vcs" => json!(mesh.vcs()),
            "peers/share" | "peers/unshare" | "peers/shares" => {
                let files = self
                    .files
                    .as_ref()
                    .ok_or_else(|| RpcError::new(NOT_AVAILABLE, "this daemon has no file sync"))?;
                match method {
                    "peers/share" => {
                        let p: aivory_protocol::mesh::ShareParams = parse_params(params)?;
                        json!(files.share(&p).map_err(failed)?)
                    }
                    "peers/unshare" => {
                        let p: aivory_protocol::mesh::ShareRef = parse_params(params)?;
                        files.unshare(&p.id).map_err(failed)?;
                        json!({ "ok": true })
                    }
                    _ => json!(files.view()),
                }
            }
            "peers/call" => {
                let p: aivory_protocol::mesh::PeerCallParams = parse_params(params)?;
                mesh.call(&p.id, &p.method, p.params).map_err(failed)?
            }
            "peers/remove" => {
                let p: PeerRef = parse_params(params)?;
                mesh.remove(&p.id).map_err(failed)?;
                json!({ "ok": true })
            }
            _ => {
                return Err(RpcError::new(
                    METHOD_NOT_FOUND,
                    format!("unknown method '{method}'"),
                ));
            }
        };
        Ok(value)
    }

    /// `daemon/restart`: only the local daemon restarts. A remote daemon of the demo answers
    /// `ok` and changes nothing.
    fn restart(&self, params: Value) -> Result<Value, RpcError> {
        let p: DaemonRef = parse_params(params)?;
        let kind = match (&self.demo, &self.live) {
            (Some(demo), _) => demo.daemon_kind(&p.daemon),
            (None, _) => (p.daemon == "local").then_some(DaemonKind::Local),
        };
        match kind {
            None => Err(RpcError::not_found("daemon", &p.daemon)),
            Some(DaemonKind::Local) => {
                self.restart.store(true, Ordering::SeqCst);
                self.shutdown.store(true, Ordering::SeqCst);
                Ok(json!({ "ok": true }))
            }
            Some(_) => Ok(json!({ "ok": true })),
        }
    }

    /// `settings/importClaude`: reads `model` and `permissions.defaultMode` of the settings
    /// file of Claude Code, in the Claude folder of the configuration.
    fn import_claude(&self, params: Value) -> Result<Value, RpcError> {
        let _: Empty = parse_params(params)?;
        let home = &self.config.roots.claude;
        let report = match (&self.live, &self.demo) {
            (Some(live), _) => live.import_claude(home),
            (None, Some(demo)) => demo.import_claude(home),
            (None, None) => {
                return Err(RpcError::new(
                    NOT_AVAILABLE,
                    "'settings/importClaude' needs the war-room engine; start the daemon with `aivoryd serve`",
                ));
            }
        };
        serde_json::to_value(report).map_err(|e| RpcError::new(SERVER_ERROR, e.to_string()))
    }
}

/// Filter for [`Daemon::list`].
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub provider: Option<Provider>,
    pub cwd: Option<String>,
    pub limit: Option<usize>,
}

pub(crate) fn parse_params<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, RpcError> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params)
        .map_err(|e| RpcError::new(INVALID_PARAMS, format!("invalid params: {e}")))
}

fn error_response(id: &Value, e: &RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": e })
}

/// `media/get`: the bytes of an image, video, or PDF file. Only these media types: the call
/// reads no source code or other file.
fn media_get(src: &str) -> Result<aivory_protocol::media::MediaData, RpcError> {
    use aivory_protocol::media::{MEDIA_GET_MAX_BYTES, Media, MediaKind};
    use base64::Engine as _;
    let bad = |m: &str| RpcError::new(aivory_protocol::rpc::code::INVALID_PARAMS, m.to_string());
    let media = Media::of(src)
        .filter(|m| m.kind != MediaKind::Link && !src.starts_with("http"))
        .ok_or_else(|| bad("not an image, video, or PDF file"))?;
    let path = std::path::Path::new(src);
    if !path.is_absolute() {
        return Err(bad("the path is not absolute"));
    }
    let meta = std::fs::metadata(path).map_err(|_| bad("no such file"))?;
    if !meta.is_file() || meta.len() > MEDIA_GET_MAX_BYTES {
        return Err(bad("no file, or larger than 20 MB"));
    }
    let bytes = std::fs::read(path).map_err(|e| bad(&e.to_string()))?;
    Ok(aivory_protocol::media::MediaData {
        mime: media.mime.unwrap_or_default(),
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        size: meta.len(),
    })
}

/// `media/put`: store an image of a phone in `dir` under a unique name. Only png, jpeg, gif
/// and webp; at most [`aivory_protocol::media::MEDIA_GET_MAX_BYTES`].
fn media_put(
    dir: &std::path::Path,
    p: &aivory_protocol::media::MediaPutParams,
) -> Result<aivory_protocol::media::MediaStored, RpcError> {
    use aivory_protocol::media::{MEDIA_GET_MAX_BYTES, Media, MediaKind};
    use base64::Engine as _;
    let bad = |m: &str| RpcError::new(aivory_protocol::rpc::code::INVALID_PARAMS, m.to_string());
    let stem: String = p
        .name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let ok = Media::of(&stem)
        .filter(|m| m.kind == MediaKind::Image && m.mime.as_deref() != Some("image/svg+xml"));
    if ok.is_none() || stem.starts_with('.') {
        return Err(bad("not a png, jpeg, gif, or webp file name"));
    }
    if p.data.len() as u64 > MEDIA_GET_MAX_BYTES / 3 * 4 + 4 {
        return Err(bad("larger than 20 MB"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(p.data.as_bytes())
        .map_err(|_| bad("the data is not base64"))?;
    if bytes.is_empty() || bytes.len() as u64 > MEDIA_GET_MAX_BYTES {
        return Err(bad("empty, or larger than 20 MB"));
    }
    let internal =
        |e: std::io::Error| RpcError::new(aivory_protocol::rpc::code::SERVER_ERROR, e.to_string());
    std::fs::create_dir_all(dir).map_err(internal)?;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut path = dir.join(format!("{ms}-{stem}"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{ms}-{n}-{stem}"));
        n += 1;
    }
    std::fs::write(&path, &bytes).map_err(internal)?;
    Ok(aivory_protocol::media::MediaStored {
        path: path.to_string_lossy().into_owned(),
        size: bytes.len() as u64,
    })
}

#[cfg(test)]
mod media_get_tests {
    use super::{media_get, media_put};

    #[test]
    fn media_get_reads_media_files_only() {
        let dir = std::env::temp_dir().join(format!("aivoryd-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, [1u8, 2, 3]).unwrap();
        let src = dir.join("main.rs");
        std::fs::write(&src, "fn main() {}").unwrap();
        let got = media_get(&png.to_string_lossy()).unwrap();
        assert_eq!(got.mime, "image/png");
        assert_eq!(got.data, "AQID");
        assert_eq!(got.size, 3);
        assert!(media_get(&src.to_string_lossy()).is_err());
        assert!(media_get("a.png").is_err(), "a relative path");
        assert!(media_get(&dir.join("gone.png").to_string_lossy()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_put_stores_images_only() {
        use aivory_protocol::media::MediaPutParams;
        let dir = std::env::temp_dir().join(format!("aivoryd-put-{}", std::process::id()));
        let put = |name: &str, data: &str| {
            media_put(
                &dir,
                &MediaPutParams {
                    name: name.to_string(),
                    data: data.to_string(),
                },
            )
        };
        let got = put("../shot 1.PNG", "AQID").unwrap();
        assert_eq!(got.size, 3);
        let path = std::path::Path::new(&got.path);
        assert_eq!(path.parent(), Some(dir.as_path()));
        assert!(got.path.ends_with("-shot_1.PNG"), "{}", got.path);
        assert_eq!(std::fs::read(path).unwrap(), [1u8, 2, 3]);
        let again = put("../shot 1.PNG", "AQID").unwrap();
        assert_ne!(again.path, got.path);
        assert!(put("main.rs", "AQID").is_err());
        assert!(put("a.svg", "AQID").is_err());
        assert!(put("a.png", "not base64!").is_err());
        assert!(put("a.png", "").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
