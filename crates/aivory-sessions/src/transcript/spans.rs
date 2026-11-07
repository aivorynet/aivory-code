//! The span trace of a native session: the waterfall and the DAG of the deep dive.
//!
//! Each provider reader turns its file into a [`Lane`] in one pass: the turns of one agent, and
//! in each turn the LLM calls and the tool calls. [`build`] joins the lane of the main agent
//! with the lanes of the sub-agents and gives the [`SessionTrace`].
//!
//! Rules of the tree:
//! - One `run` span holds one `turn` span per user prompt. A turn lasts from its prompt to the
//!   next prompt, or to the last entry of the file for the last turn.
//! - A turn holds `llm`, `tool` and `agent` spans. An `agent` span holds the `llm` and `tool`
//!   spans of its own file.
//! - A tool call without a result in a turn that is still open has no end and the state
//!   `working`. In a turn that is closed, the call ends with the turn and the state is `parked`.
//! - Spans come parents first. Siblings come by start time.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use aivory_protocol::{
    DagEdge, DagNode, SessionState, SessionStats, SessionTrace, Span, SpanKind, UnixMs,
};

use super::common::{ReadError, clean_text, one_line, truncate};
use super::{TranscriptError, WORKING_WINDOW_MS, claude, codex, gemini};
use crate::model::Provider;

/// Longest tool result in a span, in characters.
const OUTPUT_CHARS: usize = 300;
/// Longest title of a turn or a DAG node, in characters.
const TITLE_CHARS: usize = 60;
const MAIN_AGENT: &str = "main";
/// The agent type of a sub-agent whose meta file is missing.
pub(super) const NO_META_KIND: &str = "agent";

// ---- lane -----------------------------------------------------------------------------------

/// One call of the model.
struct Llm {
    start: UnixMs,
    end: UnixMs,
    model: String,
    tokens_in: u64,
    tokens_out: u64,
    /// The part of `tokens_in` that the cache served, when the transcript tells.
    cache_read: Option<u64>,
    /// The part of `tokens_in` that wrote the cache, when the transcript tells.
    cache_write: Option<u64>,
    input: String,
    text: Option<String>,
    calls: Vec<String>,
}

/// The call of a sub-agent tool.
pub(super) struct Spawn {
    pub kind: String,
    pub task: String,
}

/// One call of a tool.
struct Tool {
    name: String,
    model: String,
    start: UnixMs,
    end: Option<UnixMs>,
    input: String,
    output: String,
    failed: bool,
    spawn: Option<Spawn>,
}

enum Item {
    Llm(Llm),
    Tool(Tool),
}

struct Turn {
    start: UnixMs,
    /// The first line of the prompt.
    prompt: String,
    items: Vec<Item>,
    /// The last text of the agent in this turn.
    answer: Option<String>,
    model: String,
    last: UnixMs,
    ended: bool,
    errored: bool,
    has_llm: bool,
}

impl Turn {
    fn new(start: UnixMs, prompt: String, model: &str) -> Self {
        Self {
            start,
            prompt,
            items: Vec::new(),
            answer: None,
            model: model.to_string(),
            last: start,
            ended: false,
            errored: false,
            has_llm: false,
        }
    }
}

/// What one entry tells about an LLM call. Entries with the same `key` are one call.
#[derive(Default)]
pub(super) struct LlmUpdate<'a> {
    pub key: Option<&'a str>,
    pub model: Option<&'a str>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub text: Option<&'a str>,
    /// The names of the tools that the call asks for.
    pub calls: Vec<&'a str>,
}

/// The activity of one agent, in file order.
pub(super) struct Lane {
    turns: Vec<Turn>,
    first: Option<UnixMs>,
    last: Option<UnixMs>,
    /// The time of the last user side event: a prompt or a tool result. The next LLM call
    /// starts there.
    prev: Option<UnixMs>,
    fallback: UnixMs,
    model: String,
    /// The tool of the last result, for the input of the next LLM call.
    result_tool: String,
    tools: HashMap<String, (usize, usize)>,
    llms: HashMap<String, (usize, usize)>,
    /// Tool calls without a result. A sub-agent call does not count.
    open: usize,
}

impl Lane {
    /// `fallback` is the time of an entry that has no time.
    pub fn new(fallback: UnixMs) -> Self {
        Self {
            turns: Vec::new(),
            first: None,
            last: None,
            prev: None,
            fallback,
            model: String::new(),
            result_tool: String::new(),
            tools: HashMap::new(),
            llms: HashMap::new(),
            open: 0,
        }
    }

    /// The time of an entry: its own time, else the time of the last entry.
    pub fn at(&self, time: Option<UnixMs>) -> UnixMs {
        time.or(self.last).unwrap_or(self.fallback)
    }

    fn start(&self) -> UnixMs {
        self.first.unwrap_or(self.fallback)
    }

    fn seen(&mut self, at: UnixMs) {
        self.first = Some(self.first.map_or(at, |f| f.min(at)));
        self.last = Some(self.last.map_or(at, |l| l.max(at)));
        if let Some(turn) = self.turns.last_mut() {
            turn.last = turn.last.max(at);
        }
    }

    fn mark(&mut self, at: UnixMs) {
        self.prev = Some(self.prev.map_or(at, |p| p.max(at)));
    }

    fn ensure_turn(&mut self, at: UnixMs) -> usize {
        if self.turns.is_empty() {
            self.turns.push(Turn::new(at, String::new(), &self.model));
        }
        self.turns.len() - 1
    }

    /// The turn that holds a time: the last turn that started at or before it.
    fn turn_at(&self, at: UnixMs) -> usize {
        self.turns
            .partition_point(|t| t.start <= at)
            .saturating_sub(1)
    }

    pub fn set_model(&mut self, model: &str) {
        if !model.is_empty() {
            self.model = model.to_string();
        }
    }

    /// The tokens of an LLM call that an earlier entry opened.
    pub fn llm_tokens(
        &mut self,
        key: &str,
        tokens_in: u64,
        tokens_out: u64,
        cache_read: Option<u64>,
    ) {
        if let Some(&(ti, ii)) = self.llms.get(key)
            && let Item::Llm(l) = &mut self.turns[ti].items[ii]
        {
            l.tokens_in = tokens_in;
            l.tokens_out = tokens_out;
            l.cache_read = cache_read;
        }
    }

    /// A user prompt opens a turn.
    pub fn prompt(&mut self, at: UnixMs, text: &str) {
        self.seen(at);
        self.mark(at);
        self.result_tool.clear();
        self.turns.push(Turn::new(at, one_line(text), &self.model));
    }

    /// The agent ended the turn, or the user stopped it.
    pub fn finish(&mut self) {
        if let Some(t) = self.turns.last_mut() {
            t.ended = true;
        }
    }

    /// The agent works again.
    pub fn resume(&mut self) {
        if let Some(t) = self.turns.last_mut() {
            t.ended = false;
            t.errored = false;
        }
    }

    /// The turn stopped on an error.
    pub fn fail(&mut self, at: UnixMs) {
        self.seen(at);
        let i = self.ensure_turn(at);
        let t = &mut self.turns[i];
        t.errored = true;
        t.ended = false;
    }

    /// One entry of an LLM call.
    pub fn llm(&mut self, at: UnixMs, u: &LlmUpdate) {
        self.seen(at);
        if let Some(model) = u.model.filter(|m| !m.is_empty()) {
            self.model = model.to_string();
        }
        let turn_index = self.ensure_turn(at);
        let text = u.text.map(one_line).filter(|t| !t.is_empty());
        if let Some(&(ti, ii)) = u.key.and_then(|k| self.llms.get(k))
            && let Item::Llm(l) = &mut self.turns[ti].items[ii]
        {
            l.end = l.end.max(at);
            l.tokens_in = u.tokens_in.unwrap_or(l.tokens_in);
            l.tokens_out = u.tokens_out.unwrap_or(l.tokens_out);
            l.cache_read = u.cache_read.or(l.cache_read);
            l.cache_write = u.cache_write.or(l.cache_write);
            if l.text.is_none() {
                l.text.clone_from(&text);
            }
            l.calls.extend(u.calls.iter().map(|c| (*c).to_string()));
            self.turns[ti].answer = text.or(self.turns[ti].answer.take());
            return;
        }
        let turn = &mut self.turns[turn_index];
        let input = if turn.has_llm {
            if self.result_tool.is_empty() {
                "tool result".to_string()
            } else {
                format!("{} result", self.result_tool)
            }
        } else {
            turn.prompt.clone()
        };
        turn.has_llm = true;
        turn.model.clone_from(&self.model);
        turn.answer = text.clone().or(turn.answer.take());
        let start = self.prev.map_or(at, |p| p.min(at));
        turn.items.push(Item::Llm(Llm {
            start,
            end: at,
            model: self.model.clone(),
            tokens_in: u.tokens_in.unwrap_or(0),
            tokens_out: u.tokens_out.unwrap_or(0),
            cache_read: u.cache_read,
            cache_write: u.cache_write,
            input,
            text,
            calls: u.calls.iter().map(|c| (*c).to_string()).collect(),
        }));
        if let Some(k) = u.key {
            self.llms
                .insert(k.to_string(), (turn_index, turn.items.len() - 1));
        }
    }

    /// A tool call. A call with an `id` that the lane knows is a copy and is dropped.
    pub fn tool_start(
        &mut self,
        at: UnixMs,
        id: Option<&str>,
        name: &str,
        input: String,
        spawn: Option<Spawn>,
    ) {
        self.seen(at);
        if id.is_some_and(|id| self.tools.contains_key(id)) {
            return;
        }
        let ti = self.ensure_turn(at);
        if spawn.is_none() {
            self.open += 1;
        }
        let turn = &mut self.turns[ti];
        turn.items.push(Item::Tool(Tool {
            name: name.to_string(),
            model: self.model.clone(),
            start: at,
            end: None,
            input,
            output: String::new(),
            failed: false,
            spawn,
        }));
        if let Some(id) = id {
            self.tools
                .insert(id.to_string(), (ti, turn.items.len() - 1));
        }
    }

    /// A tool call without a result: it ends where it starts.
    pub fn tool_instant(&mut self, at: UnixMs, name: &str, input: String) {
        self.tool_start(at, None, name, input, None);
        self.open = self.open.saturating_sub(1);
        if let Some(Item::Tool(t)) = self.turns.last_mut().and_then(|t| t.items.last_mut()) {
            t.end = Some(at);
        }
    }

    /// The result of a tool call. `at` is the end time.
    pub fn tool_end(&mut self, at: UnixMs, id: &str, output: &str, failed: bool) {
        self.seen(at);
        self.mark(at);
        let Some(&(ti, ii)) = self.tools.get(id) else {
            return;
        };
        let Item::Tool(t) = &mut self.turns[ti].items[ii] else {
            return;
        };
        if t.end.is_some() {
            return;
        }
        t.end = Some(at.max(t.start));
        t.output = head(output);
        t.failed = failed;
        if t.spawn.is_none() {
            self.open = self.open.saturating_sub(1);
        }
        self.result_tool.clone_from(&t.name);
    }

    fn last_turn(&self) -> Option<&Turn> {
        self.turns.last()
    }

    fn ended(&self) -> bool {
        self.last_turn().is_some_and(|t| t.ended)
    }

    fn errored(&self) -> bool {
        self.last_turn().is_some_and(|t| t.errored)
    }

    fn is_fresh(&self, now: UnixMs) -> bool {
        now.saturating_sub(self.last.unwrap_or(self.fallback)) < WORKING_WINDOW_MS
    }
}

/// The first `OUTPUT_CHARS` characters of a tool result, without escape sequences.
pub(super) fn head(text: &str) -> String {
    let prefix: String = text.chars().take(OUTPUT_CHARS * 4).collect();
    clean_text(&prefix).chars().take(OUTPUT_CHARS).collect()
}

/// The tool id of a tool name, for example `bash` or `file_edit`.
fn tool_id(name: &str) -> String {
    match name {
        "Bash" | "PowerShell" | "shell" | "shell_command" | "run_shell_command"
        | "exec_command" => "bash".into(),
        "Read" | "NotebookRead" | "read_file" | "read_many_files" => "file_read".into(),
        "Edit" | "MultiEdit" | "NotebookEdit" | "replace" | "apply_patch" => "file_edit".into(),
        "Write" | "write_file" => "file_write".into(),
        "Grep"
        | "Glob"
        | "LS"
        | "ToolSearch"
        | "glob"
        | "list_directory"
        | "search_file_content" => "search".into(),
        "WebFetch" | "WebSearch" | "web_search" | "web_fetch" | "google_web_search" => "web".into(),
        "Agent" | "Task" | "spawn_agent" => "agent".into(),
        other => other.to_lowercase(),
    }
}

// ---- sub-agents -----------------------------------------------------------------------------

/// The lane of a sub-agent and what the meta file says about it.
pub(super) struct AgentLane {
    pub id: String,
    /// The agent type, for example `Explore`.
    pub kind: String,
    /// The short task, from the meta file.
    pub task: String,
    /// The id of the tool call that started the agent.
    pub tool_use_id: Option<String>,
    pub finished: bool,
    pub lane: Lane,
}

impl AgentLane {
    /// The sub-agent has no meta file: an old Claude version wrote it.
    fn has_no_meta(&self) -> bool {
        self.kind == NO_META_KIND
    }

    /// The end and the state of the agent. An agent that the meta file does not call finished,
    /// and that did not end, is open while it has a tool call without a result, or while its
    /// file changed lately.
    fn status(&self, now: UnixMs) -> (Option<UnixMs>, SessionState) {
        let lane = &self.lane;
        let last = lane.last.unwrap_or(lane.fallback);
        if self.finished {
            (Some(last), SessionState::Done)
        } else if lane.errored() {
            (Some(last), SessionState::Error)
        } else if lane.ended() {
            (Some(last), SessionState::Done)
        } else if lane.open > 0 || lane.is_fresh(now) {
            (None, SessionState::Working)
        } else {
            (Some(last), SessionState::Parked)
        }
    }
}

// ---- entry ----------------------------------------------------------------------------------

/// Build the span trace of a session file. The main file and each sub-agent file are read
/// once.
pub(super) fn trace(
    provider: Provider,
    path: &Path,
    session_id: &str,
    now: UnixMs,
) -> Result<SessionTrace, TranscriptError> {
    let fallback = claude::mtime_ms(path).unwrap_or(now);
    let (main, agents) =
        read(provider, path, fallback).map_err(|e| TranscriptError::new(path, e))?;
    Ok(build(session_id, main, agents, now))
}

fn read(
    provider: Provider,
    path: &Path,
    fallback: UnixMs,
) -> Result<(Lane, Vec<AgentLane>), ReadError> {
    let reader =
        || -> Result<_, ReadError> { Ok(BufReader::with_capacity(256 * 1024, File::open(path)?)) };
    match provider {
        Provider::Claude => claude::lanes(path, fallback),
        Provider::Codex => Ok((codex::lane(reader()?, fallback)?, Vec::new())),
        Provider::Gemini => Ok((
            gemini::lane(reader()?, gemini::is_document(path), fallback)?,
            Vec::new(),
        )),
    }
}

// ---- build ----------------------------------------------------------------------------------

/// An item that a span holds.
enum Child<'a> {
    Llm(&'a Llm),
    Tool(&'a Tool),
    /// A sub-agent call that no sub-agent file answers.
    Spawn(&'a Tool, &'a Spawn),
    Agent(usize),
}

/// Where a sub-agent sits in the main lane.
struct Home {
    turn: usize,
    start: UnixMs,
}

/// The facts that the whole build needs.
struct Ctx<'a> {
    agents: &'a [AgentLane],
    homes: &'a [Home],
    status: &'a [(Option<UnixMs>, SessionState)],
    now: UnixMs,
}

/// The span that holds the items.
struct Frame<'a> {
    parent: &'a str,
    agent: &'a str,
    /// A tool call without a result is still running.
    tools_open: bool,
    /// The end of the span, for a tool call that never got a result.
    end: Option<UnixMs>,
}

struct Builder {
    spans: Vec<Span>,
    /// The DAG nodes of the sub-agents, with the start time that orders them.
    nodes: Vec<(UnixMs, DagNode)>,
    tool_calls: u32,
}

fn kids_of<'a>(items: &'a [Item], skip: impl Fn(usize) -> bool) -> Vec<(UnixMs, Child<'a>)> {
    let mut kids = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Llm(l) => kids.push((l.start, Child::Llm(l))),
            Item::Tool(t) => match &t.spawn {
                Some(_) if skip(i) => {}
                Some(s) => kids.push((t.start, Child::Spawn(t, s))),
                None => kids.push((t.start, Child::Tool(t))),
            },
        }
    }
    kids
}

/// The end and the state of a tool call.
fn tool_status(t: &Tool, frame: &Frame) -> (Option<UnixMs>, SessionState) {
    match t.end {
        Some(end) => (
            Some(end),
            if t.failed {
                SessionState::Error
            } else {
                SessionState::Done
            },
        ),
        None if frame.tools_open => (None, SessionState::Working),
        None => (
            Some(frame.end.unwrap_or(t.start).max(t.start)),
            SessionState::Parked,
        ),
    }
}

fn span(kind: SpanKind, name: String, agent: &str, parent: Option<&str>) -> Span {
    Span {
        id: String::new(),
        parent: parent.map(str::to_string),
        kind,
        name,
        agent: agent.to_string(),
        model: String::new(),
        start: 0,
        end: None,
        state: SessionState::Done,
        tokens_in: 0,
        tokens_out: 0,
        tokens_cache_read: None,
        tokens_cache_write: None,
        cost: 0.0,
        tool: None,
        input: String::new(),
        output: String::new(),
    }
}

impl Builder {
    /// Add a span and give it the next id.
    fn push(&mut self, mut s: Span) -> usize {
        s.id = format!("s{}", self.spans.len() + 1);
        self.spans.push(s);
        self.spans.len() - 1
    }

    fn id(&self, index: usize) -> String {
        self.spans[index].id.clone()
    }

    /// Add the spans of the items, siblings by start time. Gives the tokens of the items.
    fn items(&mut self, mut kids: Vec<(UnixMs, Child)>, frame: &Frame, ctx: &Ctx) -> (u64, u64) {
        kids.sort_by_key(|k| k.0);
        let (mut tokens_in, mut tokens_out) = (0, 0);
        for (_, kid) in kids {
            match kid {
                Child::Llm(l) => {
                    let mut s = span(
                        SpanKind::Llm,
                        "chat".into(),
                        frame.agent,
                        Some(frame.parent),
                    );
                    s.model.clone_from(&l.model);
                    s.start = l.start;
                    s.end = Some(l.end.max(l.start));
                    s.tokens_in = l.tokens_in;
                    s.tokens_out = l.tokens_out;
                    s.tokens_cache_read = l.cache_read;
                    s.tokens_cache_write = l.cache_write;
                    s.input.clone_from(&l.input);
                    s.output = llm_output(l);
                    tokens_in += l.tokens_in;
                    tokens_out += l.tokens_out;
                    self.push(s);
                }
                Child::Tool(t) => {
                    let (end, state) = tool_status(t, frame);
                    let mut s = span(
                        SpanKind::Tool,
                        t.name.clone(),
                        frame.agent,
                        Some(frame.parent),
                    );
                    s.model.clone_from(&t.model);
                    s.start = t.start;
                    s.end = end;
                    s.state = state;
                    s.tool = Some(tool_id(&t.name));
                    s.input.clone_from(&t.input);
                    s.output.clone_from(&t.output);
                    self.tool_calls += 1;
                    self.push(s);
                }
                Child::Spawn(t, sp) => self.spawn(t, sp, frame, ctx.now),
                Child::Agent(i) => {
                    let (tin, tout) = self.agent(i, frame, ctx);
                    tokens_in += tin;
                    tokens_out += tout;
                }
            }
        }
        (tokens_in, tokens_out)
    }

    /// The span of a sub-agent call that has no sub-agent file.
    fn spawn(&mut self, t: &Tool, sp: &Spawn, frame: &Frame, now: UnixMs) {
        let (end, state) = tool_status(t, frame);
        let mut s = span(
            SpanKind::Agent,
            format!("invoke {}", sp.kind),
            &sp.kind,
            Some(frame.parent),
        );
        s.model.clone_from(&t.model);
        s.start = t.start;
        s.end = end;
        s.state = state;
        s.input.clone_from(&sp.task);
        s.output.clone_from(&t.output);
        let index = self.push(s);
        let id = self.id(index);
        self.node(&sp.kind, &sp.task, (t.start, end, state), now, id);
    }

    /// The span of a sub-agent with its own spans. Gives its tokens.
    fn agent(&mut self, i: usize, frame: &Frame, ctx: &Ctx) -> (u64, u64) {
        let a = &ctx.agents[i];
        let (end, state) = ctx.status[i];
        let start = ctx.homes[i].start;
        let end = end.map(|e| e.max(start));
        let mut s = span(
            SpanKind::Agent,
            format!("invoke {}", a.kind),
            &a.kind,
            Some(frame.parent),
        );
        s.model.clone_from(&a.lane.model);
        s.start = start;
        s.end = end;
        s.state = state;
        s.input = if a.task.is_empty() {
            a.lane
                .turns
                .first()
                .map(|t| t.prompt.clone())
                .unwrap_or_default()
        } else {
            a.task.clone()
        };
        s.output = a
            .lane
            .turns
            .iter()
            .rev()
            .find_map(|t| t.answer.clone())
            .unwrap_or_default();
        let index = self.push(s);
        let id = self.id(index);
        let inner = Frame {
            parent: &id,
            agent: &a.kind,
            tools_open: end.is_none(),
            end,
        };
        let kids = a
            .lane
            .turns
            .iter()
            .flat_map(|t| kids_of(&t.items, |_| false))
            .collect();
        let (tokens_in, tokens_out) = self.items(kids, &inner, ctx);
        self.spans[index].tokens_in = tokens_in;
        self.spans[index].tokens_out = tokens_out;
        let task = self.spans[index].input.clone();
        self.node(&a.kind, &task, (start, end, state), ctx.now, id);
        (tokens_in, tokens_out)
    }

    /// The DAG node of a sub-agent span. `run` is the start, the end and the state.
    fn node(
        &mut self,
        agent: &str,
        task: &str,
        run: (UnixMs, Option<UnixMs>, SessionState),
        now: UnixMs,
        span: String,
    ) {
        let (start, end, state) = run;
        let title = if task.is_empty() { agent } else { task };
        self.nodes.push((
            start,
            DagNode {
                id: String::new(),
                title: truncate(title, TITLE_CHARS),
                agent: agent.to_string(),
                state,
                duration_ms: Some(duration(start, end, now)),
                span: Some(span),
            },
        ));
    }
}

fn duration(start: UnixMs, end: Option<UnixMs>, now: UnixMs) -> u64 {
    u64::try_from(end.unwrap_or(now).saturating_sub(start)).unwrap_or(0)
}

/// What the model said in a call: its text, else its tool calls.
fn llm_output(l: &Llm) -> String {
    if let Some(t) = &l.text {
        t.clone()
    } else if l.calls.is_empty() {
        "thinking".to_string()
    } else {
        l.calls.join(", ")
    }
}

/// Join the lane of the main agent and the lanes of the sub-agents into the trace.
pub(super) fn build(
    session: &str,
    mut main: Lane,
    mut agents: Vec<AgentLane>,
    now: UnixMs,
) -> SessionTrace {
    agents.sort_by(|a, b| {
        a.lane
            .start()
            .cmp(&b.lane.start())
            .then_with(|| a.id.cmp(&b.id))
    });
    if main.turns.is_empty()
        && let Some(a) = agents.first()
    {
        main.ensure_turn(a.lane.start());
    }

    // The call of each sub-agent: the call that its meta file names, else the nearest earlier
    // call of the same type that no other sub-agent claimed.
    let mut matched: HashSet<(usize, usize)> = HashSet::new();
    let mut calls: Vec<Option<(usize, usize)>> = agents
        .iter()
        .map(|a| {
            let call = a
                .tool_use_id
                .as_deref()
                .and_then(|id| main.tools.get(id))
                .copied()
                .filter(|&(ti, ii)| {
                    matches!(&main.turns[ti].items[ii], Item::Tool(t) if t.spawn.is_some())
                });
            matched.extend(call);
            call
        })
        .collect();
    for (ai, a) in agents.iter_mut().enumerate() {
        if calls[ai].is_some() {
            continue;
        }
        let start = a.lane.start();
        let best = main
            .turns
            .iter()
            .enumerate()
            .flat_map(|(ti, t)| {
                t.items
                    .iter()
                    .enumerate()
                    .map(move |(ii, item)| (ti, ii, item))
            })
            .filter_map(|(ti, ii, item)| match item {
                Item::Tool(t) if t.start <= start && !matched.contains(&(ti, ii)) => {
                    t.spawn.as_ref().map(|sp| (ti, ii, t.start, sp))
                }
                _ => None,
            })
            .filter(|(.., sp)| a.has_no_meta() || sp.kind == a.kind)
            .max_by_key(|&(_, _, at, _)| at);
        if let Some((ti, ii, _, sp)) = best {
            if a.has_no_meta() {
                a.kind.clone_from(&sp.kind);
                a.task.clone_from(&sp.task);
            }
            matched.insert((ti, ii));
            calls[ai] = Some((ti, ii));
        }
    }
    // The turn of each sub-agent: the turn of its call, else the turn that holds its start.
    let homes: Vec<Home> = agents
        .iter()
        .zip(&calls)
        .map(|(a, call)| {
            let start = a.lane.start();
            let call_start = call.and_then(|(ti, ii)| match &main.turns[ti].items[ii] {
                Item::Tool(t) => Some((ti, t.start)),
                Item::Llm(_) => None,
            });
            match call_start {
                Some((turn, at)) => Home {
                    turn,
                    start: at.min(start),
                },
                None => Home {
                    turn: main.turn_at(start),
                    start,
                },
            }
        })
        .collect();
    let status: Vec<_> = agents.iter().map(|a| a.status(now)).collect();
    let ctx = Ctx {
        agents: &agents,
        homes: &homes,
        status: &status,
        now,
    };

    let mut b = Builder {
        spans: Vec::new(),
        nodes: Vec::new(),
        tool_calls: 0,
    };
    let run_name = format!("session {}", session.chars().take(8).collect::<String>());
    let run = b.push(span(SpanKind::Run, run_name, MAIN_AGENT, None));
    let run_id = b.id(run);
    let last_index = main.turns.len().saturating_sub(1);
    let mut turn_ends: Vec<UnixMs> = Vec::new();
    let mut last_state = SessionState::Parked;
    let mut run_open = agents_open(&status);
    let (mut run_in, mut run_out) = (0, 0);
    for (ti, turn) in main.turns.iter().enumerate() {
        let is_last = ti == last_index;
        let open_agent = (0..agents.len()).any(|ai| homes[ai].turn == ti && status[ai].0.is_none());
        let pending = turn
            .items
            .iter()
            .any(|i| matches!(i, Item::Tool(t) if t.end.is_none() && t.spawn.is_none()));
        let turn_open = is_last
            && !turn.errored
            && ((!turn.ended && (pending || main.is_fresh(now))) || open_agent);
        let mut end = if turn_open {
            None
        } else if is_last {
            let agent_ends = (0..agents.len())
                .filter(|&ai| homes[ai].turn == ti)
                .filter_map(|ai| status[ai].0);
            Some(agent_ends.fold(turn.last, UnixMs::max))
        } else {
            Some(main.turns[ti + 1].start)
        };
        end = end.map(|e| e.max(turn.start));
        let state = if turn.errored {
            SessionState::Error
        } else if turn_open {
            SessionState::Working
        } else if turn.ended || !is_last {
            SessionState::Done
        } else {
            SessionState::Parked
        };
        let name = if turn.prompt.is_empty() {
            format!("turn {}", ti + 1)
        } else {
            format!("turn {} · {}", ti + 1, truncate(&turn.prompt, TITLE_CHARS))
        };
        let mut s = span(SpanKind::Turn, name, MAIN_AGENT, Some(&run_id));
        s.model.clone_from(&turn.model);
        s.start = turn.start;
        s.end = end;
        s.state = state;
        s.input.clone_from(&turn.prompt);
        s.output = turn.answer.clone().unwrap_or_default();
        let index = b.push(s);
        let turn_id = b.id(index);

        let mut kids = kids_of(&turn.items, |ii| matched.contains(&(ti, ii)));
        for (ai, home) in homes.iter().enumerate() {
            if home.turn == ti {
                kids.push((home.start, Child::Agent(ai)));
            }
        }
        let frame = Frame {
            parent: &turn_id,
            agent: MAIN_AGENT,
            tools_open: is_last && !turn.errored && !turn.ended,
            end,
        };
        let (tin, tout) = b.items(kids, &frame, &ctx);
        b.spans[index].tokens_in = tin;
        b.spans[index].tokens_out = tout;
        run_in += tin;
        run_out += tout;
        turn_ends.push(end.unwrap_or(now));
        if is_last {
            run_open |= turn_open;
            last_state = state;
        }
    }

    // The run.
    let first_start = main
        .turns
        .first()
        .map_or_else(|| main.start(), |t| t.start)
        .min(homes.iter().map(|h| h.start).min().unwrap_or(UnixMs::MAX));
    let tail_end = turn_ends
        .last()
        .copied()
        .unwrap_or_else(|| main.last.unwrap_or(main.fallback));
    let agent_end = status.iter().filter_map(|s| s.0).max().unwrap_or(tail_end);
    let run_end = (!run_open).then(|| tail_end.max(agent_end).max(first_start));
    let turns = u32::try_from(main.turns.len()).unwrap_or(u32::MAX);
    let subagents = u32::try_from(b.nodes.len()).unwrap_or(u32::MAX);
    let run_state = if run_open {
        SessionState::Working
    } else {
        last_state
    };
    let first_prompt = main
        .turns
        .iter()
        .map(|t| t.prompt.as_str())
        .find(|p| !p.is_empty())
        .unwrap_or_default()
        .to_string();
    {
        let s = &mut b.spans[run];
        s.model.clone_from(&main.model);
        s.start = first_start;
        s.end = run_end;
        s.state = run_state;
        s.tokens_in = run_in;
        s.tokens_out = run_out;
        s.input.clone_from(&first_prompt);
        s.output = format!(
            "{turns} turns · {} tool calls · {subagents} subagents",
            b.tool_calls
        );
    }

    // The DAG: the main agent, and an edge to each sub-agent in the order of the start.
    let mut sub_nodes = std::mem::take(&mut b.nodes);
    sub_nodes.sort_by_key(|n| n.0);
    let main_title = if first_prompt.is_empty() {
        format!("session {}", session.chars().take(8).collect::<String>())
    } else {
        truncate(&first_prompt, TITLE_CHARS)
    };
    let mut nodes = vec![DagNode {
        id: "n1".to_string(),
        title: main_title,
        agent: MAIN_AGENT.to_string(),
        state: run_state,
        duration_ms: Some(duration(first_start, run_end, now)),
        span: Some(run_id),
    }];
    let mut edges = Vec::with_capacity(sub_nodes.len());
    for (i, (_, mut node)) in sub_nodes.into_iter().enumerate() {
        node.id = format!("n{}", i + 2);
        edges.push(DagEdge {
            from: "n1".to_string(),
            to: node.id.clone(),
        });
        nodes.push(node);
    }
    SessionTrace {
        session: session.to_string(),
        stats: SessionStats {
            subagents,
            turns,
            tool_calls: b.tool_calls,
            cost: 0.0,
            context_pct: 0,
        },
        spans: b.spans,
        nodes,
        edges,
    }
}

fn agents_open(status: &[(Option<UnixMs>, SessionState)]) -> bool {
    status.iter().any(|s| s.0.is_none())
}
