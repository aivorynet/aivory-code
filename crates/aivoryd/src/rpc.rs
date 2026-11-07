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
    /// `code/*` and `ledger/*` (demo or live).
    code: Option<std::sync::Arc<crate::code::CodeService>>,
    /// `resources/memory`: the RAM samples of the machine and the agents.
    resources: crate::resources::Sampler,
    /// `theme/*` and `registry/*` (ADR 0024).
    themes: std::sync::Arc<crate::theme::ThemeService>,
}

impl Daemon {
    pub fn new(config: Config) -> Self {
        let themes = std::sync::Arc::new(crate::theme::ThemeService::new(
            crate::theme::Store::for_cache_file(config.cache_file.as_deref()),
            std::sync::Arc::new(crate::theme::HttpFetcher::new()),
        ));
        Self {
            themes,
            config,
            shutdown: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            demo: None,
            live: None,
            pairings: None,
            code: None,
            resources: crate::resources::Sampler::default(),
        }
    }

    /// Replace the theme service (tests use a fake fetcher and a scratch folder).
    pub fn with_themes(mut self, themes: std::sync::Arc<crate::theme::ThemeService>) -> Self {
        self.themes = themes;
        self
    }

    /// A daemon that serves the war room on the native sessions of this machine.
    /// Let `devices/pair` and `devices/remove` use the remote endpoint.
    pub fn with_pairings(mut self, pairings: std::sync::Arc<crate::remote::Pairings>) -> Self {
        self.pairings = Some(pairings);
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
        let result = self.dispatch(method, req.params.unwrap_or(Value::Null), &id, out);
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
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "aivoryd".to_string());
        let new = aivory_protocol::rpc::NewSessionParams {
            codebase,
            part: None,
            title: Some(format!("Organizer · {}", p.mode.label())),
            prompt: crate::resources::organizer_prompt(p.mode, &exe),
            composer: aivory_protocol::ComposerPrefs {
                mode: aivory_protocol::Mode::Auto,
                provider: "claude".to_string(),
                model: String::new(),
                effort: aivory_protocol::Effort::Medium,
            },
            task: None,
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
