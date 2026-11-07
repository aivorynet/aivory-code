//! `aivoryd serve --demo`: the war-room surface on the sample data of
//! [`aivory_protocol::demo`]. Answers, pauses, new sessions, tasks and settings change the data
//! in memory, and subscribers get every change as a `warroom/delta` notification. The demo
//! clock starts at [`DEMO_NOW`] and runs with the wall clock.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use aivory_protocol::coord::HitStatus;
use aivory_protocol::demo::{self, DEMO_NOW, Demo};
use aivory_protocol::rpc::{
    self, Ack, AutopilotPatch, CodebaseRef, Created, Delta, DeltaNote, DeviceRef, HandoffParams,
    HistoryParams, InboxAnswerParams, InboxListParams, InboxSnoozeParams, Method, NewSessionParams,
    RpcError, SendParams, SessionRef, SettingsSetParams, StartWhen, SubscribeParams, Subscription,
    TaskCreateParams, TaskListParams, TaskUpdateParams, code, method,
};
use aivory_protocol::{
    AcpAdded, AddAcpParams, AddPartParams, Autopilot, Codebase, ComposerPrefs, DaemonKind,
    DaemonScaleParams, Effort, HistoryPage, ImportReport, InboxFilter, InboxList, Line, MINUTE_MS,
    MapFile, Mode, Pairing, Pane, Part, PartStats, PinnedRepo, ProjectNewParams, RepoAddParams,
    RepoList, SessionDiff, SessionState, Task, TaskColumn, TaskList, UnixMs, WarRoom,
};
use serde_json::{Value, json};

use crate::actions;
use crate::outbox::Outbox;

/// Deltas that a client can catch up on after a reconnect.
const RING: usize = 1024;
const HISTORY_PAGE: u32 = 200;

type Reply = Result<Option<Value>, RpcError>;

#[derive(Debug)]
pub struct DemoBackend {
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    demo: Demo,
    started: Instant,
    epoch: u64,
    seq: u64,
    ring: VecDeque<DeltaNote>,
    subscribers: Vec<Outbox>,
    /// Inbox item id -> snoozed until.
    snoozed: HashMap<String, UnixMs>,
    next_id: u32,
    /// The mailboxes that a client changed (ping, override). Others come from the sample.
    mail: HashMap<String, Vec<aivory_protocol::coord::MailItem>>,
    /// The codebases of real folders (`repos/add`, `project/new`). The other codebases are
    /// sample data without a folder: `map/addPart` changes them in memory only.
    real: HashSet<String>,
    /// The last detection of each ACP agent that a client added.
    acp_seen: HashMap<String, (bool, Option<String>)>,
    /// What a human did about the radar pairs (ping, pause).
    radar_marks: HashMap<(String, String), HitStatus>,
}

impl Default for DemoBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DemoBackend {
    pub fn new() -> Self {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |d| d.as_millis() as u64);
        let mut demo = Demo::new();
        demo.war_room.epoch = epoch;
        Self {
            state: Mutex::new(State {
                demo,
                started: Instant::now(),
                epoch,
                seq: 0,
                ring: VecDeque::new(),
                subscribers: Vec::new(),
                snoozed: HashMap::new(),
                next_id: 1,
                mail: HashMap::new(),
                real: HashSet::new(),
                acp_seen: HashMap::new(),
                radar_marks: HashMap::new(),
            }),
        }
    }

    /// The methods of the war-room surface. `daemon/*` stays with the daemon.
    pub fn handles(method: &str) -> bool {
        rpc::METHODS.contains(&method)
            && !matches!(method, "daemon/info" | "daemon/shutdown" | "daemon/restart")
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `settings/importClaude`: the daemon reads `<claude_home>/settings.json`.
    pub fn import_claude(&self, claude_home: &std::path::Path) -> ImportReport {
        let mut st = self.lock();
        actions::import_claude(claude_home, &mut st.demo.settings.settings)
    }

    /// The kind of a daemon of the settings, or `None` for an unknown id.
    pub fn daemon_kind(&self, id: &str) -> Option<DaemonKind> {
        self.lock()
            .demo
            .settings
            .daemons
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.kind)
    }

    /// A revert of the demo ledger went through: the session shows a line for it.
    pub fn note_revert(&self, session: &str, turn: u32) {
        let mut st = self.lock();
        if st.demo.war_room.session(session).is_none() {
            return;
        }
        let lines = st.append(
            session,
            None,
            vec![Line::meta(format!("— reverted turn {turn} —"))],
        );
        st.publish(vec![lines]);
    }

    /// Handle one method. `Ok(None)`: the response went out already (`warroom/subscribe`).
    pub fn dispatch(&self, name: &str, params: Value, id: &Value, out: &Outbox) -> Reply {
        let mut st = self.lock();
        match name {
            "warroom/snapshot" => call::<method::WarroomSnapshot>(params, |_| Ok(st.snapshot())),
            "warroom/subscribe" => {
                let p: SubscribeParams = parse(params)?;
                st.subscribe(p, id, out)
            }
            "session/send" => call::<method::SessionSend>(params, |p| st.send(p)),
            "session/dispatch" => call::<method::SessionDispatch>(params, |p| st.dispatch_work(p)),
            "session/mode" => call::<method::SessionMode>(params, |p| {
                let mut pane = st.pane(&p.id)?;
                pane.composer.mode = p.mode;
                st.publish(vec![Delta::Session {
                    session: Box::new(pane),
                }]);
                Ok(Ack::OK)
            }),
            "session/answer" => call::<method::SessionAnswer>(params, |p| {
                let answer = answer_text(p.option.as_deref(), p.text.as_deref())?;
                st.answer(&p.id, Some(&p.ask), &answer).map(|()| Ack::OK)
            }),
            "session/pause" => call::<method::SessionPause>(params, |p| st.pause(&p)),
            "session/stop" => call::<method::SessionStop>(params, |p| {
                st.finish(&p, SessionState::Done, "— stopped by you —")
            }),
            "session/park" => call::<method::SessionPark>(params, |p| {
                st.finish(&p, SessionState::Parked, "— parked —")
            }),
            "session/resume" => call::<method::SessionResume>(params, |p| st.resume(&p)),
            "session/new" => call::<method::SessionNew>(params, |p| st.new_session(p)),
            "session/handoff" => call::<method::SessionHandoff>(params, |p| st.handoff(p)),
            "session/fork" => call::<method::SessionFork>(params, |p| st.fork(&p)),
            "session/history" => call::<method::SessionHistory>(params, |p| st.history(&p)),
            "session/spans" => call::<method::SessionSpans>(params, |p| {
                let pane = st.pane(&p.id)?;
                Ok(match st.demo.traces.get(&p.id) {
                    Some(t) => t.clone(),
                    None => demo::generic_trace(&pane, 0),
                })
            }),
            "session/diff" => call::<method::SessionDiffMethod>(params, |p| {
                st.pane(&p.id)?;
                Ok(st.demo.diffs.get(&p.id).cloned().unwrap_or(SessionDiff {
                    session: p.id,
                    files: Vec::new(),
                }))
            }),
            "autopilot/get" => {
                call::<method::AutopilotGet>(params, |_| Ok(st.demo.war_room.autopilot.clone()))
            }
            "autopilot/set" => call::<method::AutopilotSet>(params, |p| Ok(st.autopilot(&p))),
            "inbox/list" => call::<method::InboxListMethod>(params, |p| Ok(st.inbox(&p))),
            "inbox/answer" => call::<method::InboxAnswer>(params, |p| st.inbox_answer(&p)),
            "inbox/snooze" => call::<method::InboxSnooze>(params, |p| st.snooze(&p)),
            "tasks/list" => call::<method::TasksList>(params, |p| Ok(st.tasks(&p))),
            "tasks/create" => call::<method::TasksCreate>(params, |p| st.create_task(p)),
            "tasks/update" => call::<method::TasksUpdate>(params, |p| st.update_task(p)),
            "repos/list" => call::<method::ReposList>(params, |_| {
                Ok(RepoList {
                    codebases: st.demo.war_room.codebases.clone(),
                })
            }),
            "repos/get" => call::<method::ReposGet>(params, |p: CodebaseRef| st.repo_detail(&p.id)),
            "repos/map" => call::<method::ReposMap>(params, |p: CodebaseRef| {
                if st.real.contains(&p.id)
                    && let Some(cb) = st.demo.war_room.codebase(&p.id)
                {
                    return Ok(crate::repos::map(cb));
                }
                st.demo
                    .maps
                    .iter()
                    .find(|m| m.codebase == p.id)
                    .cloned()
                    .ok_or_else(|| RpcError::not_found("codebase", &p.id))
            }),
            "repos/add" => call::<method::ReposAdd>(params, |p| st.repos_add(p)),
            "repos/remove" => call::<method::ReposRemove>(params, |p| st.repos_remove(&p.id)),
            "project/new" => call::<method::ProjectNew>(params, |p| st.project_new(&p)),
            "map/addPart" => call::<method::MapAddPart>(params, |p| st.add_part(p)),
            "providers/addAcp" => call::<method::ProvidersAddAcp>(params, |p| st.acp_add(&p)),
            "providers/removeAcp" => {
                call::<method::ProvidersRemoveAcp>(params, |p| st.acp_remove(&p.id))
            }
            "daemon/scale" => call::<method::DaemonScale>(params, |p| st.scale(&p)),
            "claims/list" => call::<method::ClaimsList>(params, |p| {
                let mut list = demo::claims();
                list.claims.retain(|c| {
                    p.codebase.as_ref().is_none_or(|x| *x == c.codebase)
                        && p.session.as_ref().is_none_or(|x| *x == c.session)
                });
                Ok(list)
            }),
            "radar/list" => call::<method::RadarListMethod>(params, |p| {
                let mut radar = st.radar();
                radar
                    .hits
                    .retain(|h| p.codebase.as_ref().is_none_or(|x| *x == h.codebase));
                let ids: Vec<String> = radar
                    .hits
                    .iter()
                    .flat_map(|h| [h.a.clone(), h.b.clone()])
                    .collect();
                radar.sessions.retain(|n| ids.contains(&n.id));
                Ok(radar)
            }),
            "radar/ping" => call::<method::RadarPing>(params, |p| st.ping(&p)),
            "mail/send" => call::<method::MailSend>(params, |p| st.ping(&p)),
            "mail/list" => call::<method::MailListMethod>(params, |p| Ok(st.mail_list(&p.session))),
            "block/override" => call::<method::BlockOverride>(params, |p| {
                let mut resolved = 0;
                let mut items = st.mail_list(&p.session).items;
                for m in &mut items {
                    if m.kind == aivory_protocol::coord::PingKind::Block
                        && !m.resolved
                        && (p.keys.is_empty() || p.keys.contains(&m.key))
                    {
                        m.resolved = true;
                        resolved += 1;
                    }
                }
                st.mail.insert(p.session, items);
                Ok(aivory_protocol::coord::BlockOverrideResult { resolved })
            }),
            "settings/get" => call::<method::SettingsGet>(params, |_| Ok(st.demo.settings.clone())),
            "settings/set" => call::<method::SettingsSet>(params, |p| st.set_setting(p)),
            "devices/pair" => call::<method::DevicesPair>(params, |_| Ok(st.pair())),
            "devices/remove" => call::<method::DevicesRemove>(params, |p: DeviceRef| {
                let devices = &mut st.demo.settings.devices;
                let before = devices.len();
                devices.retain(|d| d.id != p.id);
                if devices.len() == before {
                    return Err(RpcError::not_found("device", &p.id));
                }
                Ok(Ack::OK)
            }),
            other => Err(RpcError::new(
                code::METHOD_NOT_FOUND,
                format!("unknown method '{other}'"),
            )),
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params)
        .map_err(|e| RpcError::new(code::INVALID_PARAMS, format!("invalid params: {e}")))
}

/// Parse the params of `M`, run `f`, serialize the result.
fn call<M: Method>(
    params: Value,
    f: impl FnOnce(M::Params) -> Result<M::Result, RpcError>,
) -> Reply {
    let result = f(parse(params)?)?;
    serde_json::to_value(result)
        .map(Some)
        .map_err(|e| RpcError::new(code::SERVER_ERROR, e.to_string()))
}

fn answer_text(option: Option<&str>, text: Option<&str>) -> Result<String, RpcError> {
    option
        .or(text)
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "give an option or a text"))
}

fn invalid_state(message: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_STATE, message)
}

impl State {
    fn now(&self) -> UnixMs {
        DEMO_NOW + self.started.elapsed().as_millis() as i64
    }

    fn snapshot(&self) -> WarRoom {
        let mut view = self.demo.war_room.clone();
        view.now = self.now();
        view
    }

    /// Apply the deltas to the view, keep them for catch-up, and send them to the subscribers.
    fn publish(&mut self, deltas: Vec<Delta>) {
        let touches = deltas
            .iter()
            .any(|d| matches!(d, Delta::Session { .. } | Delta::SessionRemoved { .. }));
        self.publish_each(deltas);
        if touches {
            let board = self.demo.war_room.compute_board();
            if board != self.demo.war_room.board {
                self.publish_each(vec![Delta::Board { board }]);
            }
        }
    }

    fn publish_each(&mut self, deltas: Vec<Delta>) {
        for delta in deltas {
            self.seq += 1;
            self.demo.war_room.apply(self.seq, &delta);
            let note = DeltaNote {
                seq: self.seq,
                delta,
            };
            let message = json!({ "jsonrpc": "2.0", "method": rpc::WARROOM_DELTA, "params": note });
            self.subscribers.retain(|o| o.try_send(message.clone()));
            self.ring.push_back(note);
            if self.ring.len() > RING {
                self.ring.pop_front();
            }
        }
    }

    /// The deltas after `cursor`, or `None` when the ring no longer holds all of them.
    fn since(&self, cursor: u64) -> Option<Vec<DeltaNote>> {
        let oldest = self.ring.front().map_or(self.seq + 1, |n| n.seq);
        if cursor > self.seq || cursor + 1 < oldest {
            return None;
        }
        Some(
            self.ring
                .iter()
                .filter(|n| n.seq > cursor)
                .cloned()
                .collect(),
        )
    }

    /// Send the response under the lock, so that no delta can come before it.
    fn subscribe(&mut self, p: SubscribeParams, id: &Value, out: &Outbox) -> Reply {
        let catch_up = match (p.cursor, p.epoch) {
            (Some(cursor), Some(epoch)) if epoch == self.epoch => self.since(cursor),
            _ => None,
        };
        let result = Subscription {
            epoch: self.epoch,
            cursor: self.seq,
            snapshot: catch_up.is_none().then(|| self.snapshot()),
            deltas: catch_up.unwrap_or_default(),
        };
        if !id.is_null() && !out.try_send(json!({ "jsonrpc": "2.0", "id": id, "result": result })) {
            return Err(RpcError::new(code::SERVER_ERROR, "the connection is busy"));
        }
        self.subscribers.push(out.clone());
        Ok(None)
    }

    fn pane(&self, id: &str) -> Result<Pane, RpcError> {
        self.demo
            .war_room
            .session(id)
            .cloned()
            .ok_or_else(|| RpcError::new(code::SESSION_NOT_FOUND, format!("no session '{id}'")))
    }

    /// Append lines to a transcript. The delta updates the pane tail.
    fn append(&mut self, id: &str, subagent: Option<&str>, lines: Vec<Line>) -> Delta {
        let name = subagent.map(|s| s.strip_prefix(&format!("{id}-")).unwrap_or(s).to_string());
        let key = match &name {
            Some(n) => format!("{id}/{n}"),
            None => id.to_string(),
        };
        let history = self.demo.histories.entry(key).or_default();
        history.extend(lines.iter().cloned());
        Delta::Lines {
            id: id.to_string(),
            subagent: name,
            lines,
            total: history.len() as u32,
        }
    }

    fn counts(&self) -> Delta {
        let now = self.now();
        Delta::Counts {
            queued: self
                .demo
                .tasks
                .iter()
                .filter(|t| t.column == TaskColumn::Planned)
                .count() as u32,
            inbox_open: self
                .demo
                .inbox
                .items
                .iter()
                .filter(|i| self.snoozed.get(&i.id).is_none_or(|&until| until <= now))
                .count() as u32,
        }
    }

    /// Move the inbox item of an ask to Done.
    fn close_inbox_item(&mut self, ask: &str, result: String) {
        let items = &mut self.demo.inbox.items;
        if let Some(pos) = items.iter().position(|i| i.ask.as_deref() == Some(ask)) {
            let mut item = items.remove(pos);
            item.result = Some(result);
            self.demo.inbox.done.insert(0, item);
        }
    }

    /// The task of a session goes on after an answer.
    fn task_continues(&mut self, task: Option<&str>) {
        if let Some(t) = self
            .demo
            .tasks
            .iter_mut()
            .find(|t| Some(t.id.as_str()) == task)
            && t.column == TaskColumn::Waiting
        {
            t.column = TaskColumn::Working;
            t.waiting_for = None;
            t.steps = demo::task_steps(t.playbook, t.column, t.step, false);
        }
    }

    fn answer(&mut self, id: &str, ask_id: Option<&str>, answer: &str) -> Result<(), RpcError> {
        let mut pane = self.pane(id)?;
        let Some(ask) = pane.ask.take() else {
            return Err(RpcError::new(
                code::STALE_ASK,
                "the session has no open ask",
            ));
        };
        if ask_id.is_some_and(|a| a != ask.id) {
            return Err(RpcError::new(code::STALE_ASK, "the ask is no longer open"));
        }
        pane.state = SessionState::Working;
        pane.since = self.now();
        let task = pane.task.clone();
        let lines = self.append(
            id,
            None,
            vec![Line::user(answer), Line::agent(demo::answer_reply(answer))],
        );
        self.close_inbox_item(&ask.id, demo::inbox_result(answer));
        self.task_continues(task.as_deref());
        let counts = self.counts();
        self.publish(vec![
            Delta::Session {
                session: Box::new(pane),
            },
            lines,
            counts,
        ]);
        Ok(())
    }

    fn send(&mut self, p: SendParams) -> Result<Ack, RpcError> {
        let mut pane = self.pane(&p.id)?;
        if p.subagent.is_none() && pane.ask.is_some() {
            // Text in the composer of a pane with an ask is an own answer.
            self.answer(&p.id, None, &p.text)?;
            return Ok(Ack::OK);
        }
        let mut deltas = Vec::new();
        if p.subagent.is_none() && !matches!(pane.state, SessionState::Working) {
            pane.state = SessionState::Working;
            pane.since = self.now();
        }
        if let Some(c) = p.composer {
            pane.composer = c;
        }
        deltas.push(Delta::Session {
            session: Box::new(pane),
        });
        deltas.push(self.append(
            &p.id,
            p.subagent.as_deref(),
            vec![
                Line::user(&p.text),
                Line::agent(demo::answer_reply(&p.text)),
            ],
        ));
        self.publish(deltas);
        Ok(Ack::OK)
    }

    fn pause(&mut self, p: &SessionRef) -> Result<Ack, RpcError> {
        let mut pane = self.pane(&p.id)?;
        if pane.state != SessionState::Working {
            return Err(invalid_state("only a working session can pause"));
        }
        pane.state = SessionState::Idle;
        pane.since = self.now();
        self.mark_radar(&p.id, HitStatus::Paused);
        let lines = self.append(&p.id, None, vec![Line::meta("— paused by you —")]);
        self.publish(vec![
            Delta::Session {
                session: Box::new(pane),
            },
            lines,
        ]);
        Ok(Ack::OK)
    }

    /// Stop or park a session. An open ask closes with it.
    fn finish(&mut self, p: &SessionRef, to: SessionState, note: &str) -> Result<Ack, RpcError> {
        let mut pane = self.pane(&p.id)?;
        if pane.state == to {
            return Err(invalid_state(format!(
                "the session is {} already",
                to.label()
            )));
        }
        if let Some(ask) = pane.ask.take() {
            self.close_inbox_item(&ask.id, to.label().to_string());
        }
        pane.state = to;
        pane.since = self.now();
        let lines = self.append(&p.id, None, vec![Line::meta(note)]);
        let counts = self.counts();
        self.publish(vec![
            Delta::Session {
                session: Box::new(pane),
            },
            lines,
            counts,
        ]);
        Ok(Ack::OK)
    }

    fn resume(&mut self, p: &SessionRef) -> Result<Ack, RpcError> {
        let mut pane = self.pane(&p.id)?;
        if !matches!(
            pane.state,
            SessionState::Parked | SessionState::Done | SessionState::Idle
        ) {
            return Err(invalid_state(
                "only a parked, done or idle session can resume",
            ));
        }
        pane.state = SessionState::Working;
        pane.since = self.now();
        self.radar_marks
            .retain(|(a, b), s| !(*s == HitStatus::Paused && (a == &p.id || b == &p.id)));
        let lines = self.append(
            &p.id,
            None,
            vec![
                Line::user("Resume"),
                Line::agent(demo::answer_reply("Resume")),
            ],
        );
        self.publish(vec![
            Delta::Session {
                session: Box::new(pane),
            },
            lines,
        ]);
        Ok(Ack::OK)
    }

    fn next_session_id(&mut self) -> String {
        let id = format!("n{:03}", self.next_id);
        self.next_id += 1;
        id
    }

    /// A new session of an existing task shows on the task card. `tasks/create` links its own
    /// first session, because the task does not exist yet when the session starts.
    fn link_task_session(&mut self, task: Option<&str>, pane: &Pane) {
        let Some(task) = self
            .demo
            .tasks
            .iter_mut()
            .find(|t| Some(t.id.as_str()) == task)
        else {
            return;
        };
        let main = task.sessions.is_empty();
        task.sessions.push(aivory_protocol::TaskSession {
            session: pane.id.clone(),
            title: pane.title.clone(),
            role: format!("{} · {}", if main { "main" } else { "sub" }, pane.model),
            state: pane.state,
        });
        if !task.agents.contains(&pane.provider) {
            task.agents.push(pane.provider.clone());
        }
        task.started.get_or_insert(pane.since);
    }

    fn create_session(&mut self, p: NewSessionParams) -> Result<String, RpcError> {
        self.create_session_with(p, Vec::new())
    }

    /// Start a session. `carried` lines (the history of a fork) come before its first lines.
    fn create_session_with(
        &mut self,
        p: NewSessionParams,
        carried: Vec<Line>,
    ) -> Result<String, RpcError> {
        let cb = self
            .demo
            .war_room
            .codebase(&p.codebase)
            .ok_or_else(|| RpcError::not_found("codebase", &p.codebase))?;
        let part = match p.part {
            Some(part) if cb.parts.iter().any(|x| x.name == part) => part,
            Some(part) => return Err(RpcError::not_found("part", &part)),
            None => cb.parts.first().map(|x| x.name.clone()).unwrap_or_default(),
        };
        let id = self.next_session_id();
        let title = p
            .title
            .unwrap_or_else(|| p.prompt.chars().take(48).collect::<String>());
        let pane = Pane {
            id: id.clone(),
            title,
            codebase: p.codebase.clone(),
            part: part.clone(),
            provider: p.composer.provider.clone(),
            model: p.composer.model.clone(),
            state: SessionState::Working,
            since: self.now(),
            cost: 0.0,
            tokens: 0,
            controlled: true,
            context_pct: 0,
            prompt: p.prompt.clone(),
            ask: None,
            subagents: Vec::new(),
            tail: Vec::new(),
            lines_total: 0,
            composer: p.composer.clone(),
            task: p.task.clone(),
        };
        self.link_task_session(p.task.as_deref(), &pane);
        let mut first = carried;
        first.extend([
            Line::meta(format!(
                "{} {} · {} · session {id}",
                p.composer.provider, p.composer.model, p.codebase
            )),
            Line::user(&p.prompt),
            Line::agent(format!("I start with the code in part \"{part}\".")),
        ]);
        let lines = self.append(&id, None, first);
        self.publish(vec![
            Delta::Session {
                session: Box::new(pane),
            },
            lines,
        ]);
        Ok(id)
    }

    /// `session/dispatch`: start a session when the autopilot has a free slot, else queue a task.
    fn dispatch_work(&mut self, p: rpc::DispatchParams) -> Result<rpc::Dispatched, RpcError> {
        let prompt = p.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(RpcError::new(code::INVALID_PARAMS, "the prompt is empty"));
        }
        let provider = p.agent.clone().unwrap_or_else(|| "claude".to_string());
        let cb = match &p.codebase {
            Some(id) => self
                .demo
                .war_room
                .codebase(id)
                .ok_or_else(|| RpcError::not_found("codebase", id))?,
            None => self
                .demo
                .war_room
                .codebases
                .first()
                .ok_or_else(|| invalid_state("no codebase"))?,
        };
        let codebase = cb.id.clone();
        let part = p
            .part
            .clone()
            .or_else(|| cb.parts.first().map(|x| x.name.clone()))
            .unwrap_or_default();
        if !self.demo.war_room.has_free_slot(&codebase) {
            let task = self.create_task(TaskCreateParams {
                title: prompt.chars().take(48).collect(),
                codebase,
                part,
                playbook: aivory_protocol::Playbook::Feature,
                prompt,
                when: StartWhen::Queue,
                provider: Some(provider),
                mode: None,
                priority: None,
            })?;
            return Ok(rpc::Dispatched {
                session: None,
                task: Some(task.id),
            });
        }
        let prefs = self
            .demo
            .settings
            .settings
            .providers
            .get(&provider)
            .cloned();
        let composer = ComposerPrefs {
            mode: prefs.as_ref().map_or(Mode::Edit, |x| x.mode),
            model: p
                .model
                .clone()
                .unwrap_or_else(|| self.default_model(&provider)),
            effort: p.effort.unwrap_or(prefs.map_or(Effort::High, |x| x.effort)),
            provider,
        };
        let session = self.create_session(NewSessionParams {
            codebase,
            part: Some(part),
            title: None,
            prompt,
            composer,
            task: None,
        })?;
        Ok(rpc::Dispatched {
            session: Some(session),
            task: None,
        })
    }

    fn new_session(&mut self, p: NewSessionParams) -> Result<Created, RpcError> {
        self.create_session(p).map(|id| Created { id })
    }

    /// Continue a session with another provider; the old session parks.
    fn handoff(&mut self, p: HandoffParams) -> Result<Created, RpcError> {
        let old = self.pane(&p.id)?;
        let model = p
            .model
            .clone()
            .unwrap_or_else(|| self.default_model(&p.provider));
        let id = self.create_session(NewSessionParams {
            codebase: old.codebase.clone(),
            part: Some(old.part.clone()),
            title: Some(old.title.clone()),
            prompt: format!("Continue session {}: {}", old.id, old.prompt),
            composer: ComposerPrefs {
                provider: p.provider.clone(),
                model,
                ..old.composer.clone()
            },
            task: old.task.clone(),
        })?;
        if old.state != SessionState::Parked {
            self.finish(
                &SessionRef { id: old.id.clone() },
                SessionState::Parked,
                &format!("— handed off to {} · session {id} —", p.provider),
            )?;
        }
        Ok(Created { id })
    }

    /// The turn that `at` of a fork names: a turn number, or a span id (`s3`) of the trace.
    /// `None`: the end of the session.
    fn fork_turn(&self, old: &Pane, at: Option<&str>) -> Result<Option<u32>, RpcError> {
        let Some(at) = at.map(str::trim).filter(|a| !a.is_empty()) else {
            return Ok(None);
        };
        if let Ok(n) = at.parse::<u32>() {
            return Ok(Some(n));
        }
        let trace = match self.demo.traces.get(&old.id) {
            Some(t) => t.clone(),
            None => demo::generic_trace(old, 0),
        };
        if !trace.spans.iter().any(|s| s.id == at) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("the session has no span '{at}'"),
            ));
        }
        Ok(trace.turn_of_span(at))
    }

    fn fork(&mut self, p: &rpc::ForkParams) -> Result<Created, RpcError> {
        let old = self.pane(&p.id)?;
        let turn = self.fork_turn(&old, p.at.as_deref())?;
        // The lines of the first `turn` turns come along (a turn starts at a user line).
        let carried: Vec<Line> = match (turn, self.demo.histories.get(&old.id)) {
            (Some(n), Some(history)) => {
                let starts: Vec<usize> = history
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.kind == aivory_protocol::LineKind::User)
                    .map(|(i, _)| i)
                    .collect();
                let end = starts.get(n as usize).copied().unwrap_or(history.len());
                history[..end].to_vec()
            }
            _ => Vec::new(),
        };
        let title = match turn {
            Some(n) => format!("Fork of {} · turn {n}", old.title),
            None => format!("Fork of {}", old.title),
        };
        let id = self.create_session_with(
            NewSessionParams {
                codebase: old.codebase.clone(),
                part: Some(old.part.clone()),
                title: Some(title),
                prompt: p.prompt.clone().unwrap_or_else(|| match turn {
                    Some(n) => format!("Continue from session {} at turn {n}.", old.id),
                    None => format!("Continue from session {}.", old.id),
                }),
                composer: old.composer.clone(),
                task: old.task.clone(),
            },
            carried,
        )?;
        Ok(Created { id })
    }

    fn default_model(&self, provider: &str) -> String {
        self.demo
            .settings
            .settings
            .providers
            .get(provider)
            .map(|p| p.model.clone())
            .unwrap_or_else(|| "default".to_string())
    }

    fn history(&self, p: &HistoryParams) -> Result<HistoryPage, RpcError> {
        self.pane(&p.id)?;
        let lines = self
            .demo
            .history(&p.id, p.subagent.as_deref())
            .ok_or_else(|| RpcError::not_found("subagent", p.subagent.as_deref().unwrap_or("")))?;
        let total = lines.len() as u32;
        let end = p.before.unwrap_or(total).min(total);
        let start = end.saturating_sub(p.limit.unwrap_or(HISTORY_PAGE));
        Ok(HistoryPage {
            session: p.id.clone(),
            subagent: p.subagent.clone(),
            start,
            total,
            lines: lines[start as usize..end as usize].to_vec(),
        })
    }

    fn autopilot(&mut self, p: &AutopilotPatch) -> aivory_protocol::Autopilot {
        let mut a = self.demo.war_room.autopilot.clone();
        a.patch(p);
        self.demo.settings.settings.autopilot = a.clone();
        self.publish(vec![Delta::Autopilot {
            autopilot: a.clone(),
        }]);
        a
    }

    fn inbox(&self, p: &InboxListParams) -> InboxList {
        let now = self.now();
        let filter = p.filter.unwrap_or(InboxFilter::All);
        InboxList {
            items: self
                .demo
                .inbox
                .items
                .iter()
                .filter(|i| filter.matches(i.kind))
                .filter(|i| self.snoozed.get(&i.id).is_none_or(|&until| until <= now))
                .cloned()
                .collect(),
            done: self.demo.inbox.done.clone(),
        }
    }

    fn inbox_answer(
        &mut self,
        p: &InboxAnswerParams,
    ) -> Result<aivory_protocol::InboxItem, RpcError> {
        let answer = answer_text(p.option.as_deref(), p.text.as_deref())?;
        let item = self
            .demo
            .inbox
            .items
            .iter()
            .find(|i| i.id == p.id)
            .cloned()
            .ok_or_else(|| RpcError::not_found("inbox item", &p.id))?;
        let session_ask = self
            .demo
            .war_room
            .session(&item.session)
            .and_then(|s| s.ask.as_ref())
            .map(|a| a.id.clone());
        match &item.ask {
            Some(ask) if session_ask.as_ref() == Some(ask) => {
                self.answer(&item.session, Some(ask), &answer)?;
            }
            _ => {
                let items = &mut self.demo.inbox.items;
                let mut done = items.remove(items.iter().position(|i| i.id == p.id).unwrap_or(0));
                done.result = Some(demo::inbox_result(&answer));
                self.demo.inbox.done.insert(0, done);
                let counts = self.counts();
                self.publish(vec![counts]);
            }
        }
        self.snoozed.remove(&p.id);
        self.demo
            .inbox
            .done
            .iter()
            .find(|i| i.id == p.id)
            .cloned()
            .ok_or_else(|| RpcError::not_found("inbox item", &p.id))
    }

    fn snooze(&mut self, p: &InboxSnoozeParams) -> Result<Ack, RpcError> {
        let until = self.now() + i64::from(p.minutes) * MINUTE_MS;
        let ids: Vec<String> = match &p.id {
            Some(id) if self.demo.inbox.items.iter().any(|i| &i.id == id) => vec![id.clone()],
            Some(id) => return Err(RpcError::not_found("inbox item", id)),
            None => self.demo.inbox.items.iter().map(|i| i.id.clone()).collect(),
        };
        for id in ids {
            self.snoozed.insert(id, until);
        }
        let counts = self.counts();
        self.publish(vec![counts]);
        Ok(Ack::OK)
    }

    fn tasks(&self, p: &TaskListParams) -> TaskList {
        TaskList {
            tasks: self
                .demo
                .tasks
                .iter()
                .filter(|t| p.codebase.as_ref().is_none_or(|c| &t.codebase == c))
                .cloned()
                .collect(),
        }
    }

    fn create_task(&mut self, p: TaskCreateParams) -> Result<Task, RpcError> {
        if self.demo.war_room.codebase(&p.codebase).is_none() {
            return Err(RpcError::not_found("codebase", &p.codebase));
        }
        let id = format!("t{}", self.demo.tasks.len() + 1);
        let provider = p.provider.clone().unwrap_or_else(|| "claude".to_string());
        let prefs = self
            .demo
            .settings
            .settings
            .providers
            .get(&provider)
            .cloned();
        let column = match p.when {
            StartWhen::Queue => TaskColumn::Planned,
            StartWhen::Now | StartWhen::PlanOnly => TaskColumn::Working,
        };
        let mut task = Task {
            id: id.clone(),
            column,
            title: p.title.clone(),
            codebase: p.codebase.clone(),
            part: p.part.clone(),
            playbook: p.playbook,
            step: 0,
            waiting_for: None,
            agents: Vec::new(),
            prompt: p.prompt.clone(),
            priority: Default::default(),
            created_at: 0,
            started: None,
            cost: 0.0,
            sessions: Vec::new(),
            gates: Vec::new(),
            steps: demo::task_steps(p.playbook, column, 0, false),
        };
        if column == TaskColumn::Working {
            let mode = match p.when {
                StartWhen::PlanOnly => Mode::Plan,
                _ => p
                    .mode
                    .or(prefs.as_ref().map(|x| x.mode))
                    .unwrap_or(Mode::Edit),
            };
            let composer = ComposerPrefs {
                mode,
                provider: provider.clone(),
                model: self.default_model(&provider),
                effort: prefs.map_or(Effort::High, |x| x.effort),
            };
            let session = self.create_session(NewSessionParams {
                codebase: p.codebase,
                part: Some(p.part),
                title: Some(p.title.clone()),
                prompt: p.prompt,
                composer: composer.clone(),
                task: Some(id.clone()),
            })?;
            task.started = Some(self.now());
            task.agents = vec![provider];
            task.sessions = vec![aivory_protocol::TaskSession {
                session,
                title: p.title,
                role: format!("main · {}", composer.model),
                state: SessionState::Working,
            }];
        }
        self.demo.tasks.push(task.clone());
        let counts = self.counts();
        self.publish(vec![counts]);
        Ok(task)
    }

    fn update_task(&mut self, p: TaskUpdateParams) -> Result<Task, RpcError> {
        let task = self
            .demo
            .tasks
            .iter_mut()
            .find(|t| t.id == p.id)
            .ok_or_else(|| RpcError::not_found("task", &p.id))?;
        if let Some(c) = p.column {
            if !task.column.can_move_to(c) {
                return Err(invalid_state(task.column.refusal(c)));
            }
            task.column = c;
            if c == TaskColumn::Done {
                task.step = 5;
            }
            if c != TaskColumn::Waiting {
                task.waiting_for = None;
            }
        }
        if let Some(v) = p.title {
            task.title = v;
        }
        if let Some(v) = p.prompt {
            task.prompt = v;
        }
        if let Some(v) = p.part {
            task.part = v;
        }
        task.steps = demo::task_steps(
            task.playbook,
            task.column,
            task.step,
            task.waiting_for.is_some(),
        );
        let task = task.clone();
        let counts = self.counts();
        self.publish(vec![counts]);
        Ok(task)
    }

    fn set_setting(&mut self, p: SettingsSetParams) -> Result<aivory_protocol::Settings, RpcError> {
        let settings = &mut self.demo.settings.settings;
        settings
            .set_key(&p.key, p.value)
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e))?;
        let settings = settings.clone();
        if p.key.starts_with("autopilot.") {
            self.publish(vec![Delta::Autopilot {
                autopilot: settings.autopilot.clone(),
            }]);
        }
        Ok(settings)
    }

    /// The mailbox of a session: the changed one, or the sample.
    fn mail_list(&self, session: &str) -> aivory_protocol::coord::MailList {
        match self.mail.get(session) {
            Some(items) => aivory_protocol::coord::MailList {
                session: session.to_string(),
                items: items.clone(),
            },
            None => demo::mail(session),
        }
    }

    /// A ping of the client (`radar/ping`, `mail/send`): a new item in the mailbox of the
    /// receiver. A block needs a key.
    fn ping(&mut self, p: &aivory_protocol::coord::RadarPingParams) -> Result<Ack, RpcError> {
        use aivory_protocol::coord::{MailItem, PingKind};
        if p.to.trim().is_empty() || p.text.trim().is_empty() {
            return Err(RpcError::new(code::INVALID_PARAMS, "give `to` and `text`"));
        }
        if p.kind == PingKind::Block && p.keys.is_empty() {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "a block needs at least one key",
            ));
        }
        let mut items = self.mail_list(&p.to).items;
        let now = self.now();
        let mut keys = p.keys.clone();
        if keys.is_empty() || p.kind != PingKind::Block {
            keys.truncate(1);
        }
        if keys.is_empty() {
            keys.push(String::new());
        }
        for key in keys {
            let id = items.iter().map(|m| m.id).max().unwrap_or(0) + 1;
            items.push(MailItem {
                id,
                from: p.from.clone().unwrap_or_else(|| "you".into()),
                kind: p.kind,
                key,
                text: p.text.clone(),
                created_at: now,
                delivered_at: None,
                resolved: false,
                count: 1,
                digest: false,
            });
        }
        self.mail.insert(p.to.clone(), items);
        self.mark_radar(&p.to, HitStatus::Pinged);
        Ok(Ack::OK)
    }

    /// The radar of the sample with the marks of the human and the states of the panes.
    fn radar(&self) -> aivory_protocol::coord::RadarList {
        let mut radar = demo::radar();
        for hit in &mut radar.hits {
            hit.status = self
                .radar_marks
                .get(&(hit.a.clone(), hit.b.clone()))
                .copied();
        }
        for node in &mut radar.sessions {
            if let Some(pane) = self.demo.war_room.session(&node.id) {
                node.state = pane.state;
            }
        }
        // The pairs of a paused session go to the end. The sort keeps the risk order.
        radar
            .hits
            .sort_by_key(|h| h.status == Some(HitStatus::Paused));
        radar
    }

    /// Mark the pairs of `session` in the demo radar. A pause stays over a later ping.
    fn mark_radar(&mut self, session: &str, status: HitStatus) {
        for hit in demo::radar().hits {
            if hit.a != session && hit.b != session {
                continue;
            }
            let mark = self.radar_marks.entry((hit.a, hit.b)).or_insert(status);
            if status == HitStatus::Paused {
                *mark = status;
            }
        }
    }

    /// The detail of a codebase. A codebase of a real folder reads the folder.
    fn repo_detail(&self, id: &str) -> Result<aivory_protocol::RepoDetail, RpcError> {
        if self.real.contains(id)
            && let Some(cb) = self.demo.war_room.codebase(id)
        {
            return Ok(crate::repos::detail(cb, &self.demo.war_room.sessions, &[]));
        }
        self.demo
            .repos
            .iter()
            .find(|r| r.codebase.id == id)
            .cloned()
            .ok_or_else(|| RpcError::not_found("codebase", id))
    }

    /// Register a folder: a pin in the settings, a codebase in the view.
    fn pin(&mut self, cb: &Codebase) {
        self.real.insert(cb.id.clone());
        let pinned = &mut self.demo.settings.settings.pinned;
        if !pinned.iter().any(|x| x.id == cb.id) {
            pinned.push(PinnedRepo {
                id: cb.id.clone(),
                path: cb.path.clone(),
            });
        }
    }

    fn repos_add(&mut self, p: RepoAddParams) -> Result<Codebase, RpcError> {
        let root = actions::git_root(&p.path)?;
        let text = root.display().to_string();
        let known = self
            .demo
            .war_room
            .codebases
            .iter()
            .find(|c| actions::same_path(&c.path, &text))
            .cloned();
        if let Some(cb) = known {
            self.pin(&cb);
            return Ok(cb);
        }
        let id = actions::unique_id(&aivory_map::codebase_id(&root), |id| {
            self.demo.war_room.codebase(id).is_some()
        });
        let cb = actions::codebase_of(&root, id, "desktop");
        self.pin(&cb);
        self.publish(vec![Delta::Codebase {
            codebase: cb.clone(),
        }]);
        Ok(cb)
    }

    fn repos_remove(&mut self, id: &str) -> Result<Ack, RpcError> {
        let pinned = &mut self.demo.settings.settings.pinned;
        let Some(pos) = pinned.iter().position(|x| x.id == id) else {
            return Err(if self.demo.war_room.codebase(id).is_some() {
                invalid_state(format!(
                    "'{id}' has no pin: sessions show it, and `repos/remove` removes only pinned codebases"
                ))
            } else {
                RpcError::not_found("codebase", id)
            });
        };
        pinned.remove(pos);
        self.real.remove(id);
        if !self.demo.war_room.sessions.iter().any(|s| s.codebase == id) {
            self.publish(vec![Delta::CodebaseRemoved { id: id.to_string() }]);
        }
        Ok(Ack::OK)
    }

    fn project_new(&mut self, p: &ProjectNewParams) -> Result<Codebase, RpcError> {
        let root = actions::create_project(p)?;
        self.repos_add(RepoAddParams {
            path: root.display().to_string(),
        })
    }

    fn add_part(&mut self, p: AddPartParams) -> Result<MapFile, RpcError> {
        let cb = self
            .demo
            .war_room
            .codebase(&p.codebase)
            .cloned()
            .ok_or_else(|| RpcError::not_found("codebase", &p.codebase))?;
        let part = Part {
            name: p.name.trim().to_string(),
            paths: p.paths.iter().map(|x| x.trim().to_string()).collect(),
        };
        let map = if self.real.contains(&cb.id) {
            aivory_map::add_part(std::path::Path::new(&cb.path), &p.name, &p.paths)
                .map_err(actions::map_error)?
        } else {
            // A sample codebase has no folder: change the sample map in memory.
            if cb.parts.iter().any(|x| x.name == part.name) {
                return Err(RpcError::new(
                    code::INVALID_PARAMS,
                    format!("part {:?} exists already", part.name),
                ));
            }
            let (version, author) = self
                .demo
                .maps
                .iter()
                .find(|m| m.codebase == cb.id)
                .map_or((1, None), |m| (m.version, m.author.clone()));
            let mut parts = cb.parts.clone();
            parts.push(part.clone());
            aivory_map::build(&cb.id, version, self.now(), author.as_deref(), parts)
                .map_err(actions::map_error)?
        };
        let mut cb = cb;
        cb.parts = map.parts.clone();
        match self.demo.maps.iter_mut().find(|m| m.codebase == cb.id) {
            Some(m) => *m = map.clone(),
            None => self.demo.maps.push(map.clone()),
        }
        if let Some(detail) = self.demo.repos.iter_mut().find(|r| r.codebase.id == cb.id) {
            detail.codebase = cb.clone();
            if !detail.parts.iter().any(|x| x.name == part.name) {
                detail.parts.push(PartStats {
                    name: part.name.clone(),
                    paths: part.paths.clone(),
                    sessions: Vec::new(),
                    claims: 0,
                    last_change: None,
                    lines: 0,
                });
            }
        }
        self.publish(vec![Delta::Codebase { codebase: cb }]);
        Ok(map)
    }

    fn acp_add(&mut self, p: &AddAcpParams) -> Result<AcpAdded, RpcError> {
        let providers = &self.demo.settings.providers;
        let agent = actions::acp_agent(p, |id| providers.iter().any(|x| x.id == id))?;
        let seen = actions::acp_detect(&agent);
        let card = actions::acp_card(&agent, Some(&seen));
        let settings = &mut self.demo.settings;
        settings
            .settings
            .providers
            .insert(agent.id.clone(), actions::acp_prefs());
        settings.settings.acp.push(agent.clone());
        settings.providers.push(card.clone());
        self.acp_seen.insert(agent.id, seen.clone());
        Ok(AcpAdded {
            provider: card,
            found: seen.0,
            version: seen.1,
        })
    }

    fn acp_remove(&mut self, id: &str) -> Result<Ack, RpcError> {
        let settings = &mut self.demo.settings;
        let Some(pos) = settings.settings.acp.iter().position(|a| a.id == id) else {
            return Err(RpcError::not_found("ACP agent", id));
        };
        settings.settings.acp.remove(pos);
        settings.settings.providers.remove(id);
        settings.providers.retain(|p| p.id != id);
        self.acp_seen.remove(id);
        Ok(Ack::OK)
    }

    fn scale(&mut self, p: &DaemonScaleParams) -> Result<Autopilot, RpcError> {
        if !self.demo.settings.daemons.iter().any(|d| d.id == p.daemon) {
            return Err(RpcError::not_found("daemon", &p.daemon));
        }
        let (lo, hi) = Autopilot::MAX_RUNNING;
        if !(lo..=hi).contains(&p.max_running) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("maxRunning is from {lo} to {hi}"),
            ));
        }
        Ok(self.autopilot(&AutopilotPatch {
            max_running: Some(p.max_running),
            ..AutopilotPatch::default()
        }))
    }

    fn pair(&mut self) -> Pairing {
        let code = format!("AIV-{:04}", 4820 + self.next_id);
        self.next_id += 1;
        Pairing {
            url: format!("aivory://pair?host=desktop&code={code}"),
            code,
            expires: self.now() + 10 * MINUTE_MS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_of_a_planned_task_shows_on_the_task() {
        let backend = DemoBackend::new();
        let mut st = backend.lock();
        let task = st
            .demo
            .tasks
            .iter()
            .find(|t| t.column == TaskColumn::Planned && t.sessions.is_empty())
            .cloned()
            .expect("a planned task without a session");
        let composer = ComposerPrefs {
            mode: Mode::Edit,
            provider: "claude".to_string(),
            model: "opus-5.5".to_string(),
            effort: Effort::High,
        };
        let id = st
            .create_session(NewSessionParams {
                codebase: task.codebase.clone(),
                part: Some(task.part.clone()),
                title: Some(task.title.clone()),
                prompt: task.prompt.clone(),
                composer,
                task: Some(task.id.clone()),
            })
            .unwrap();
        let after = st.demo.tasks.iter().find(|t| t.id == task.id).unwrap();
        assert_eq!(after.sessions.len(), 1);
        assert_eq!(after.sessions[0].session, id);
        assert!(after.sessions[0].role.starts_with("main"));
        assert!(after.started.is_some());
    }
}
