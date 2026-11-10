//! The Board skin of the war room: the orchestration board of the Minimal look (ADR 0022).
//! Codebase modules on top, one ribbon per live session from its module down to its gate,
//! and four gates (Needs you, Working, Blocked, Idle). The cards of the gates follow the
//! density tiers. The right sidebar shows the Overview, or the session panel after a click.

mod header;
mod parts;
mod side;
mod stream;
mod strip;

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::{AskKind, Line, LineKind, Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{
    AnyElement, Bounds, Context, Div, Entity, FontWeight, Pixels, Subscription, Window, div, px,
    rgb,
};
use gpui_base::input::{InputEvent, InputState};

use crate::actions::Action;
use crate::theme::*;
use crate::ui;
use crate::warroom::{WarRoomEvent, WarRoomScreen};

/// The gap between cards.
pub(super) const GAP: f32 = 12.;
/// The most modules in the top row.
const MAX_MODULES: usize = 6;
/// Bars per module and the seconds between two samples.
const BARS: usize = 26;
const SAMPLE_SECS: u32 = 3;
/// The longest list of the run log and how long an event tag lives.
const LOG_MAX: usize = 40;
const TAG_LIFE: Duration = Duration::from_millis(2600);

/// The gates in screen order.
pub(super) const NEEDS: usize = 0;
pub(super) const WORKING: usize = 1;
pub(super) const BLOCKED: usize = 2;
pub(super) const IDLE: usize = 3;

/// What the Minimal skin shows in its center.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::warroom) enum BoardView {
    /// The orchestration board.
    #[default]
    Board,
    /// The pane grid of war room v2.
    Panes,
}

/// The width class of the window (ADR 0022, section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::warroom) enum WidthClass {
    /// Under 960 px: no sidebar; the sidebar opens as an overlay.
    Compact,
    /// 960 to 1439 px: a sidebar of 320 px.
    Regular,
    /// 1440 px and more: a sidebar of 360 px.
    Wide,
}

/// The width class of a window width in px.
pub(in crate::warroom) fn width_class(window: f32) -> WidthClass {
    if window < 960. {
        WidthClass::Compact
    } else if window < 1440. {
        WidthClass::Regular
    } else {
        WidthClass::Wide
    }
}

/// The width of the board sidebar: docked in Regular and Wide, an overlay in Compact.
pub(in crate::warroom) fn side_width(class: WidthClass) -> f32 {
    match class {
        WidthClass::Wide => 360.,
        WidthClass::Regular | WidthClass::Compact => 320.,
    }
}

/// The density tier of the gates, from the number of live sessions (ADR 0022, section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tier {
    /// 1: one large card with the live tail and the ask.
    One,
    /// 2-4: large cards.
    Large,
    /// 5-12: compact cards.
    Compact,
    /// 13-30: one-line rows, grouped by codebase.
    Rows,
    /// 31+: rows; only the groups with an ask or an error are open.
    Closed,
}

/// The tier of `live` sessions.
pub(super) fn tier(live: usize) -> Tier {
    match live {
        0 | 1 => Tier::One,
        2..=4 => Tier::Large,
        5..=12 => Tier::Compact,
        13..=30 => Tier::Rows,
        _ => Tier::Closed,
    }
}

/// An event tag that floats over a ribbon for a moment.
struct Tag {
    session: String,
    text: String,
    at: Instant,
}

/// One line of the run log.
pub(super) struct LogEntry {
    /// `HH:MM:SS`, empty for a line that was already there at the start.
    pub ts: String,
    pub session: String,
    pub text: String,
}

/// The state of the Board skin that lives between frames.
pub(in crate::warroom) struct BoardState {
    pub view: BoardView,
    /// The flow along the lines of working sessions. On by default; the Motion switch stops it.
    pub motion: bool,
    /// Only this codebase (a click on its module).
    pub module: Option<String>,
    /// The module under the pointer: its ribbons come forward.
    pub hover: Option<String>,
    /// Compact width: the sidebar shows as an overlay.
    pub side_open: bool,
    /// Codebase groups that the user opened or closed (`gate:codebase`).
    pub toggled: HashSet<String>,
    /// The place of the ask slider in the Needs you gate.
    pub ask_ix: usize,
    /// The own-answer field of the ask slider and the ask it answers (session, ask).
    pub answer: Option<Entity<InputState>>,
    pub answer_events: Option<Subscription>,
    pub answer_for: Option<(String, String)>,
    /// The row under the pointer: it shows its quick actions.
    pub hover_row: Option<String>,
    /// The line under the pointer: session and the pointer place in the band.
    pub hover_line: Option<(String, f32, f32)>,
    /// The bounds of the line band at the last paint (for the pointer tests).
    pub band: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub dispatch: Option<Entity<InputState>>,
    pub dispatch_events: Option<Subscription>,
    /// The scroll of each gate column: a column with more rows below shows a fade and a hint.
    pub gate_scroll: [gpui::ScrollHandle; 4],
    /// The scroll of the context of the ask card.
    pub ask_scroll: gpui::ScrollHandle,
    /// The scroll of the overview sidebar.
    pub side_scroll: gpui::ScrollHandle,
    /// Activity per codebase, oldest first.
    bars: HashMap<String, VecDeque<f32>>,
    last_sample: u32,
    /// Transcript lines per session at the last frame.
    seen: HashMap<String, u32>,
    tags: Vec<Tag>,
    pub(super) log: VecDeque<LogEntry>,
    events: VecDeque<Instant>,
    started: Instant,
}

impl Default for BoardState {
    fn default() -> Self {
        Self {
            view: BoardView::Board,
            motion: true,
            module: None,
            hover: None,
            side_open: false,
            toggled: HashSet::new(),
            ask_ix: 0,
            answer: None,
            answer_events: None,
            answer_for: None,
            hover_row: None,
            hover_line: None,
            band: Rc::new(Cell::new(None)),
            dispatch: None,
            dispatch_events: None,
            gate_scroll: Default::default(),
            ask_scroll: Default::default(),
            side_scroll: Default::default(),
            bars: HashMap::new(),
            last_sample: 0,
            seen: HashMap::new(),
            tags: Vec::new(),
            log: VecDeque::new(),
            events: VecDeque::new(),
            started: Instant::now(),
        }
    }
}

/// The live sessions in the four gates, in screen order.
pub(super) struct Gates<'a> {
    pub lists: [Vec<&'a Pane>; 4],
    /// The modules of the top row.
    pub mods: Vec<String>,
}

impl Gates<'_> {
    /// The session ids in the order of the screen: left gate first, top card first.
    pub fn order(&self) -> Vec<String> {
        self.lists
            .iter()
            .flat_map(|g| g.iter().map(|s| s.id.clone()))
            .collect()
    }

    /// The number of sessions in the gates.
    pub fn live(&self) -> usize {
        self.lists.iter().map(Vec::len).sum()
    }

    /// The session count of each gate.
    pub fn counts(&self) -> [usize; 4] {
        [
            self.lists[NEEDS].len(),
            self.lists[WORKING].len(),
            self.lists[BLOCKED].len(),
            self.lists[IDLE].len(),
        ]
    }
}

/// The place of a codebase in the module row; a codebase without a module goes last.
pub(super) fn module_index(mods: &[String], codebase: &str) -> usize {
    mods.iter()
        .position(|m| m == codebase)
        .unwrap_or(mods.len())
}

/// True when a session needs a person: an open ask or an error.
pub(super) fn alert(pane: &Pane) -> bool {
    pane.ask.is_some() || pane.state == SessionState::Error
}

/// Sort the live sessions of `module` (all when `None`) into the gates.
///
/// In a gate the codebase groups with an ask or an error come first, then the groups go in
/// module order, so that the ribbons cross as little as possible. In a group the sessions
/// with an ask come first, then the longest wait.
pub(super) fn split<'a>(war: &'a WarRoom, module: Option<&str>) -> Gates<'a> {
    let mods = modules(war);
    let mut lists: [Vec<&Pane>; 4] = Default::default();
    for s in &war.sessions {
        if !s.state.is_live() || module.is_some_and(|m| s.codebase != m) {
            continue;
        }
        lists[gate_of(s.state)].push(s);
    }
    for list in &mut lists {
        let hot: HashSet<String> = list
            .iter()
            .filter(|s| alert(s))
            .map(|s| s.codebase.clone())
            .collect();
        list.sort_by_key(|s| {
            (
                !hot.contains(&s.codebase),
                module_index(&mods, &s.codebase),
                s.codebase.clone(),
                s.ask.is_none(),
                s.since,
            )
        });
    }
    Gates { lists, mods }
}

/// The codebases with a live session, in map order, at most [`MAX_MODULES`].
pub(super) fn modules(war: &WarRoom) -> Vec<String> {
    war.codebases
        .iter()
        .filter(|c| {
            war.sessions
                .iter()
                .any(|s| s.codebase == c.id && s.state.is_live())
        })
        .take(MAX_MODULES)
        .map(|c| c.id.clone())
        .collect()
}

/// The sessions of a gate cut into runs of one codebase, in gate order.
pub(super) fn groups<'a>(list: &[&'a Pane]) -> Vec<(String, Vec<&'a Pane>)> {
    let mut out: Vec<(String, Vec<&Pane>)> = Vec::new();
    for s in list {
        match out.last_mut() {
            Some((cb, members)) if *cb == s.codebase => members.push(s),
            _ => out.push((s.codebase.clone(), vec![*s])),
        }
    }
    out
}

/// True when a codebase group of a gate is open. In the tier [`Tier::Closed`] only the
/// groups with an ask or an error start open; a click toggles a group in every tier.
pub(super) fn group_open(tier: Tier, alert: bool, toggled: bool) -> bool {
    let default = tier != Tier::Closed || alert;
    default != toggled
}

/// Left edge and width of the four gates in a row of `width` px. The four gates have the
/// same width (owner 2026-11-05), also an empty one.
pub(super) fn gate_spans(width: f32) -> [(f32, f32); 4] {
    let w = ((width - 3. * GAP) / 4.).max(0.);
    let mut out = [(0., 0.); 4];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = (i as f32 * (w + GAP), w);
    }
    out
}

/// True when a gate has no session (while another gate has one): it shows its header only.
pub(super) fn gate_empty(which: usize, counts: [usize; 4]) -> bool {
    counts[which] == 0 && counts.iter().any(|&c| c > 0)
}

/// Center x of module `i` of `n` in a row of `width` px.
pub(super) fn module_center(width: f32, n: usize, i: usize) -> f32 {
    let n = n.max(1) as f32;
    let w = (width - (n - 1.) * GAP) / n;
    i as f32 * (w + GAP) + w / 2.
}

/// The gate (0 to 3) of a state.
pub(super) fn gate_of(state: SessionState) -> usize {
    match state {
        SessionState::NeedsYou => NEEDS,
        SessionState::Working => WORKING,
        SessionState::Error => BLOCKED,
        _ => IDLE,
    }
}

/// The color of a state on the board: lines, dots, accents.
pub(super) fn state_tone(state: SessionState) -> u32 {
    match state {
        SessionState::NeedsYou => tk().q,
        SessionState::Working => tk().work,
        SessionState::Error => tk().e,
        _ => tk().quiet,
    }
}

/// The short name of a session in the run log: the first four characters of its id.
pub(super) fn short_id(id: &str) -> String {
    id.chars().take(4).collect()
}

/// The last line of a transcript that tells what the agent does: a tool call, else text.
pub(super) fn doing(tail: &[Line]) -> Option<String> {
    let line = tail
        .iter()
        .rev()
        .find(|l| matches!(l.kind, LineKind::Tool | LineKind::Error))
        .or_else(|| tail.iter().rev().find(|l| l.kind == LineKind::Agent))?;
    let text = line.text.lines().next().unwrap_or("").trim();
    Some(match &line.label {
        Some(l) => format!("{l} {text}"),
        None => text.to_string(),
    })
}

/// The last `n` transcript lines without the meta lines, oldest first.
pub(super) fn last_lines(tail: &[Line], n: usize) -> Vec<&Line> {
    let lines: Vec<&Line> = tail.iter().filter(|l| l.kind != LineKind::Meta).collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

/// The one state line of a card.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum StateLine {
    /// The ask: its question and options.
    Ask {
        question: String,
        options: Vec<String>,
    },
    /// The current tool or the last agent text.
    Tool(String),
    /// The error that stopped the session.
    Error(String),
    /// The time without a turn.
    Idle(String),
}

/// The state line of a session at `now`.
pub(super) fn state_line(pane: &Pane, now: aivory_protocol::UnixMs) -> StateLine {
    if let Some(ask) = &pane.ask
        && ask.kind != AskKind::Error
        && pane.state != SessionState::Error
    {
        let question = ask.text.lines().next().unwrap_or("").trim().to_string();
        return StateLine::Ask {
            question: if question.is_empty() {
                kind_label(pane).to_string()
            } else {
                question
            },
            options: ask.options.clone(),
        };
    }
    match pane.state {
        SessionState::Error => {
            let text = pane
                .tail
                .iter()
                .rev()
                .find(|l| l.kind == LineKind::Error)
                .map(|l| l.text.lines().next().unwrap_or("").trim().to_string())
                .or_else(|| pane.ask.as_ref().map(|a| a.text.clone()))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "The session stopped on an error".to_string());
            StateLine::Error(text)
        }
        SessionState::NeedsYou => StateLine::Ask {
            question: "Waits for you".to_string(),
            options: Vec::new(),
        },
        SessionState::Working => {
            StateLine::Tool(doing(&pane.tail).unwrap_or_else(|| "Thinking".to_string()))
        }
        _ => StateLine::Idle(format!(
            "Idle {}",
            wait_text(pane, now).replace("new", "0m")
        )),
    }
}

/// The word of an event tag: the tool name, else the first word.
fn tag_word(tail: &[Line]) -> Option<String> {
    let line = tail
        .iter()
        .rev()
        .find(|l| matches!(l.kind, LineKind::Tool | LineKind::Error | LineKind::Agent))?;
    let word = match &line.label {
        Some(l) => l.clone(),
        None => line.text.split_whitespace().next()?.to_string(),
    };
    Some(word.chars().take(10).collect())
}

/// Cut a text to `n` characters with an ellipsis.
pub(super) fn clip(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(n.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

/// The short kind of an ask.
pub(super) fn kind_label(pane: &Pane) -> &'static str {
    match pane.ask.as_ref().map(|a| a.kind) {
        Some(AskKind::Permission) => "Permission",
        Some(AskKind::Plan) => "Plan",
        Some(AskKind::Question) => "Question",
        Some(AskKind::Error) | None => "Error",
    }
}

/// The wait of a session as the board writes it.
pub(super) fn wait_text(pane: &Pane, now: aivory_protocol::UnixMs) -> String {
    match pane.minutes(now) {
        0 => "new".to_string(),
        m => format!("{m}m"),
    }
}

/// The collapse rule: only the Needs you gate shows full cards (the ask slider). The other
/// gates show one-line rows in every tier.
pub(super) fn shows_card(gate: usize) -> bool {
    gate == NEEDS
}

/// Move `ix` by `delta` in a ring of `n` items. An empty ring gives 0.
pub(super) fn wrap_index(ix: usize, delta: isize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    (ix as isize + delta).rem_euclid(n as isize) as usize
}

/// The providers with usage this week: tokens in a session, or a usage window above 0 %.
/// The order is the first appearance in the sessions, then in the usage windows.
pub(super) fn active_providers(war: &WarRoom) -> Vec<String> {
    let mut names: Vec<&str> = Vec::new();
    for name in war
        .sessions
        .iter()
        .map(|s| s.provider.as_str())
        .chain(war.usage.iter().map(|w| w.provider.as_str()))
    {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
        .into_iter()
        .filter(|name| {
            war.sessions
                .iter()
                .any(|s| s.provider == *name && s.tokens > 0)
                || war.usage.iter().any(|w| w.provider == *name && w.pct > 0)
        })
        .map(str::to_string)
        .collect()
}

/// Move the selection by `delta` along the screen order. Nothing selected starts at the
/// first (or last) session. The end of the list holds.
pub(super) fn step_selection(
    order: &[String],
    current: Option<&str>,
    delta: isize,
) -> Option<String> {
    if order.is_empty() {
        return None;
    }
    let at = current.and_then(|c| order.iter().position(|s| s == c));
    let next = match at {
        None if delta >= 0 => 0,
        None => order.len() - 1,
        Some(i) => (i as isize + delta).clamp(0, order.len() as isize - 1) as usize,
    };
    Some(order[next].clone())
}

/// A dispatch text split into its parts: `@codebase task text [with provider]`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Dispatch {
    pub codebase: Option<String>,
    pub provider: Option<String>,
    pub prompt: String,
}

/// Read a dispatch line. A word that starts with `@` names the codebase (a unique prefix of
/// an id is enough). A trailing `with <provider>` names the provider.
pub(super) fn parse_dispatch(text: &str, codebases: &[String], providers: &[String]) -> Dispatch {
    let mut words: Vec<&str> = text.split_whitespace().collect();
    let mut codebase = None;
    if let Some(i) = words.iter().position(|w| w.starts_with('@')) {
        let name = words[i][1..].to_lowercase();
        let hit = codebases
            .iter()
            .find(|c| c.to_lowercase() == name)
            .or_else(|| {
                let mut starts = codebases
                    .iter()
                    .filter(|c| c.to_lowercase().starts_with(&name));
                let first = starts.next();
                if starts.next().is_none() { first } else { None }
            });
        if let (Some(c), false) = (hit, name.is_empty()) {
            codebase = Some(c.clone());
            words.remove(i);
        }
    }
    let mut provider = None;
    if words.len() >= 3
        && words[words.len() - 2].eq_ignore_ascii_case("with")
        && let Some(p) = providers
            .iter()
            .find(|p| p.eq_ignore_ascii_case(words[words.len() - 1]))
    {
        provider = Some(p.clone());
        words.truncate(words.len() - 2);
    }
    Dispatch {
        codebase,
        provider,
        prompt: words.join(" "),
    }
}

impl WarRoomScreen {
    /// True while the Minimal skin shows the board (and not the pane grid).
    pub(in crate::warroom) fn board_on(&self) -> bool {
        self.skin() == aivory_protocol::Look::Minimal && self.board.view == BoardView::Board
    }

    /// Answer the ask of a session with one of its options.
    pub(super) fn answer_option(&mut self, pane: &Pane, option: &str, cx: &mut Context<Self>) {
        let Some(ask) = &pane.ask else { return };
        self.act(
            Action::Answer {
                id: pane.id.clone(),
                ask: ask.id.clone(),
                option: Some(option.to_string()),
                text: None,
            },
            cx,
        );
        self.answered(&pane.id, cx);
    }

    /// The ask that the slider of the Needs you gate shows.
    pub(super) fn slider_pane<'a>(&self, gates: &Gates<'a>) -> Option<&'a Pane> {
        let asks = &gates.lists[NEEDS];
        asks.get(wrap_index(self.board.ask_ix, 0, asks.len()))
            .copied()
    }

    /// The keys of the board: Left/Right or [ ] move the ask slider, 1 to 3 answer the ask in
    /// the slider, J and K select a session, Enter opens the Focus view.
    /// Returns true when the key was used.
    pub(in crate::warroom) fn board_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(war) = self.war.clone() else {
            return false;
        };
        let gates = split(&war, self.board.module.as_deref());
        let current = self.focus_session.clone();
        match key {
            "left" | "right" | "[" | "]" => {
                let delta = if matches!(key, "left" | "[") { -1 } else { 1 };
                let n = gates.lists[NEEDS].len();
                self.board.ask_ix = wrap_index(self.board.ask_ix, delta, n);
                n > 0
            }
            "j" | "k" => {
                let delta = if key == "j" { 1 } else { -1 };
                self.focus_session = step_selection(&gates.order(), current.as_deref(), delta);
                true
            }
            "enter" => match current.or_else(|| self.slider_pane(&gates).map(|p| p.id.clone())) {
                Some(id) => {
                    cx.emit(WarRoomEvent::Focus(id));
                    true
                }
                None => false,
            },
            "1" | "2" | "3" => {
                let n: usize = key.parse().unwrap_or(1);
                let pane = self.slider_pane(&gates).cloned();
                let option = pane
                    .as_ref()
                    .and_then(|p| p.ask.as_ref())
                    .and_then(|a| a.options.get(n - 1).cloned());
                match (pane, option) {
                    (Some(pane), Some(option)) => {
                        self.answer_option(&pane, &option, cx);
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Sample the activity bars and find the new transcript lines (tags and run log).
    fn update_feed(&mut self, war: &WarRoom) {
        let step = self.tick / (SAMPLE_SECS * 4);
        let sample = step != self.board.last_sample || self.board.bars.is_empty();
        self.board.last_sample = step;
        for cb in modules(war) {
            if sample {
                let (mut agents, mut waits) = (0., 0.);
                for s in war.sessions.iter().filter(|s| s.codebase == cb) {
                    match s.state {
                        SessionState::Working => agents += s.agents() as f32,
                        SessionState::NeedsYou | SessionState::Error => waits += 1.,
                        _ => {}
                    }
                }
                let value = (agents * 0.25 + waits * 0.1).min(1.);
                let bars = self.board.bars.entry(cb).or_default();
                bars.push_back(value);
                while bars.len() > BARS {
                    bars.pop_front();
                }
            }
        }
        let first = self.board.seen.is_empty();
        let now = Instant::now();
        let stamp = chrono::Local::now().format("%H:%M:%S").to_string();
        for s in war.sessions.iter().filter(|s| s.state.is_live()) {
            let before = self.board.seen.insert(s.id.clone(), s.lines_total);
            let Some(text) = doing(&s.tail) else { continue };
            if first {
                if self.board.log.len() < 8 {
                    self.board.log.push_back(LogEntry {
                        ts: stamp.clone(),
                        session: s.id.clone(),
                        text: clip(&text, 80),
                    });
                }
            } else if before.is_some_and(|b| b != s.lines_total) {
                self.board.events.push_back(now);
                self.board.log.push_back(LogEntry {
                    ts: stamp.clone(),
                    session: s.id.clone(),
                    text: clip(&text, 80),
                });
                if let Some(word) = tag_word(&s.tail) {
                    self.board.tags.push(Tag {
                        session: s.id.clone(),
                        text: word,
                        at: now,
                    });
                }
            }
        }
        while self.board.log.len() > LOG_MAX {
            self.board.log.pop_front();
        }
        while self
            .board
            .events
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
        {
            self.board.events.pop_front();
        }
        self.board
            .tags
            .retain(|t| now.duration_since(t.at) < TAG_LIFE);
        self.board.seen.retain(|id, _| war.session(id).is_some());
    }

    /// The own-answer field of the ask slider. Enter sends the text to the ask in the slider.
    pub(super) fn answer_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(input) = &self.board.answer {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Own answer"));
        let events = cx.subscribe_in(
            &input,
            window,
            |this, _input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.send_own_answer(window, cx);
                }
            },
        );
        self.board.answer = Some(input.clone());
        self.board.answer_events = Some(events);
        input
    }

    /// Send the own answer to the ask in the slider and clear the field.
    pub(super) fn send_own_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(input), Some((id, ask))) =
            (self.board.answer.clone(), self.board.answer_for.clone())
        else {
            return;
        };
        let text = input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.act(
            Action::Answer {
                id: id.clone(),
                ask,
                option: None,
                text: Some(text),
            },
            cx,
        );
        input.update(cx, |s, cx| s.clean(window, cx));
        self.answered(&id, cx);
    }

    /// The board: legend, modules, lines, gates, strip.
    pub(in crate::warroom) fn render_board(
        &mut self,
        war: &WarRoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.update_feed(war);
        self.answer_input(window, cx);
        let gates = split(war, self.board.module.as_deref());
        let animate =
            self.board.motion && (!self.board.tags.is_empty() || !gates.lists[WORKING].is_empty());
        if animate {
            window.request_animation_frame();
        }
        let t = self.board.started.elapsed().as_secs_f32();
        let hover = self
            .board
            .hover
            .as_deref()
            .map(|m| module_index(&gates.mods, m));
        let lit = self.board.hover_line.as_ref().map(|h| h.0.clone());
        let ribbons = stream::ribbons(&gates, lit.as_deref());
        let tags: Vec<stream::TagView> = self
            .board
            .tags
            .iter()
            .filter_map(|tag| {
                let r = ribbons.iter().find(|r| r.session == tag.session)?;
                Some(stream::TagView {
                    x: {
                        let (a, b) = r.xs(1000.);
                        (a + b) / 2000.
                    },
                    text: tag.text.clone(),
                    state: r.state,
                    age: tag.at.elapsed().as_secs_f32(),
                })
            })
            .collect();
        let pad = if width_class(f32::from(self.width)) == WidthClass::Compact {
            16.
        } else {
            24.
        };
        let mods = gates.mods.clone();
        let band = stream::render(
            ribbons,
            tags,
            hover,
            self.board.motion,
            t,
            self.board.band.clone(),
        );
        div()
            .size_full()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .px(px(pad))
            .pb(px(16.))
            .overflow_hidden()
            .child(self.render_legend(cx))
            .child(self.render_modules(war, &mods, cx))
            .child(self.render_band(war, band, cx))
            .child(self.render_gates(war, &gates, cx))
            .child(div().h(px(GAP)).flex_none())
            .child(self.render_strip(war, &gates, cx))
            .into_any_element()
    }
}

/// A small pill button of the board (`.ab`): graphite when primary, soft grey otherwise.
pub(super) fn ab(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    primary: bool,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(26.))
        .px(px(10.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded_full()
        .whitespace_nowrap()
        .text_size(ui::ts(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .when(primary, |d| {
            d.bg(rgb(tk().dark))
                .text_color(rgb(tk().card))
                .hover(|s| s.bg(rgb(tk().dark_hover)))
        })
        .when(!primary, |d| {
            d.bg(rgb(tk().track))
                .text_color(rgb(tk().ink))
                .hover(|s| s.bg(rgb(tk().track_2)))
        })
        .child(label.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::demo::Demo;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_live_sessions_reach_their_gate() {
        let war = Demo::new().war_room;
        let g = split(&war, None);
        assert_eq!(
            g.live(),
            war.sessions.iter().filter(|s| s.state.is_live()).count()
        );
        assert!(
            g.lists[NEEDS]
                .iter()
                .all(|s| s.state == SessionState::NeedsYou)
        );
        assert!(
            g.lists[BLOCKED]
                .iter()
                .all(|s| s.state == SessionState::Error)
        );
        assert!(
            g.lists[WORKING]
                .iter()
                .all(|s| s.state == SessionState::Working)
        );
    }

    #[test]
    fn a_gate_puts_the_alert_groups_first_then_the_module_order() {
        let war = Demo::new().war_room;
        let g = split(&war, None);
        for list in &g.lists {
            let hot: Vec<bool> = groups(list)
                .iter()
                .map(|(_, m)| m.iter().any(|s| alert(s)))
                .collect();
            // The alert groups come before the calm groups.
            assert!(hot.windows(2).all(|w| w[0] || !w[1]));
            // In each part the groups go in module order, and each codebase is one group.
            for want in [true, false] {
                let order: Vec<usize> = groups(list)
                    .iter()
                    .filter(|(_, m)| m.iter().any(|s| alert(s)) == want)
                    .map(|(cb, _)| module_index(&g.mods, cb))
                    .collect();
                assert!(order.windows(2).all(|w| w[0] <= w[1]), "{order:?}");
            }
            let names: Vec<String> = groups(list).into_iter().map(|(cb, _)| cb).collect();
            let mut unique = names.clone();
            unique.dedup();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), names.len());
        }
    }

    #[test]
    fn the_ribbons_of_a_module_leave_in_the_order_of_their_ends() {
        let war = Demo::new().war_room;
        let g = split(&war, None);
        let ribbons = stream::ribbons(&g, None);
        assert_eq!(ribbons.len(), g.live());
        for m in 0..=g.mods.len() {
            let mut own: Vec<_> = ribbons.iter().filter(|r| r.module() == m).collect();
            own.sort_by(|a, b| a.xs(1200.).0.total_cmp(&b.xs(1200.).0));
            let ends: Vec<f32> = own.iter().map(|r| r.xs(1200.).1).collect();
            assert!(ends.windows(2).all(|w| w[0] <= w[1]), "{ends:?}");
        }
    }

    #[test]
    fn the_density_tier_follows_the_live_count() {
        assert_eq!(tier(1), Tier::One);
        assert_eq!(tier(2), Tier::Large);
        assert_eq!(tier(4), Tier::Large);
        assert_eq!(tier(5), Tier::Compact);
        assert_eq!(tier(12), Tier::Compact);
        assert_eq!(tier(13), Tier::Rows);
        assert_eq!(tier(30), Tier::Rows);
        assert_eq!(tier(31), Tier::Closed);
    }

    #[test]
    fn many_sessions_close_the_calm_groups_only() {
        assert!(group_open(Tier::Rows, false, false));
        assert!(!group_open(Tier::Closed, false, false));
        assert!(group_open(Tier::Closed, true, false));
        assert!(group_open(Tier::Closed, false, true));
        assert!(!group_open(Tier::Closed, true, true));
    }

    #[test]
    fn the_width_class_follows_the_window() {
        assert_eq!(width_class(959.), WidthClass::Compact);
        assert_eq!(width_class(960.), WidthClass::Regular);
        assert_eq!(width_class(1439.), WidthClass::Regular);
        assert_eq!(width_class(1440.), WidthClass::Wide);
        assert_eq!(side_width(WidthClass::Wide), 360.);
        assert_eq!(side_width(WidthClass::Regular), 320.);
    }

    #[test]
    fn the_four_gates_have_the_same_width() {
        let w = 1000.;
        let spans = gate_spans(w);
        assert!(spans.iter().all(|s| (s.1 - spans[0].1).abs() < 0.01));
        assert!((spans[3].0 + spans[3].1 - w).abs() < 0.01);
        assert!((spans[1].0 - (spans[0].1 + GAP)).abs() < 0.01);
        assert!(gate_empty(0, [0, 5, 2, 0]));
        assert!(!gate_empty(1, [0, 5, 2, 0]));
        assert!(!gate_empty(0, [0; 4]));
    }

    #[test]
    fn the_state_line_says_what_the_session_does() {
        let war = Demo::new().war_room;
        for s in war.sessions.iter().filter(|s| s.state.is_live()) {
            match (s.state, state_line(s, war.now)) {
                (SessionState::Working, StateLine::Tool(t)) => assert!(!t.is_empty()),
                (SessionState::Error, StateLine::Error(t)) => assert!(!t.is_empty()),
                (SessionState::NeedsYou, StateLine::Ask { question, .. }) => {
                    assert!(!question.is_empty())
                }
                (SessionState::Idle, StateLine::Idle(t)) => assert!(t.starts_with("Idle")),
                (state, line) => panic!("{state:?} {line:?}"),
            }
        }
    }

    #[test]
    fn only_the_needs_you_gate_shows_cards() {
        assert!(shows_card(NEEDS));
        assert!(!shows_card(WORKING));
        assert!(!shows_card(BLOCKED));
        assert!(!shows_card(IDLE));
    }

    #[test]
    fn the_ask_slider_wraps_at_both_ends() {
        assert_eq!(wrap_index(0, -1, 3), 2);
        assert_eq!(wrap_index(2, 1, 3), 0);
        assert_eq!(wrap_index(1, 1, 3), 2);
        assert_eq!(wrap_index(7, 0, 3), 1);
        assert_eq!(wrap_index(4, 1, 0), 0);
    }

    #[test]
    fn only_providers_with_usage_this_week_show() {
        let mut war = Demo::new().war_room;
        let all = active_providers(&war);
        assert!(!all.is_empty());
        let gone = all[0].clone();
        for s in war.sessions.iter_mut().filter(|s| s.provider == gone) {
            s.tokens = 0;
        }
        for w in war.usage.iter_mut().filter(|w| w.provider == gone) {
            w.pct = 0;
        }
        assert!(!active_providers(&war).contains(&gone));
        if let Some(w) = war.usage.iter_mut().find(|w| w.provider == gone) {
            w.pct = 5;
            assert!(active_providers(&war).contains(&gone));
        }
    }

    #[test]
    fn modules_split_the_row_evenly() {
        let c0 = module_center(1000., 5, 0);
        let c4 = module_center(1000., 5, 4);
        assert!((c0 + c4 - 1000.).abs() < 0.01);
    }

    #[test]
    fn j_and_k_hold_at_both_ends() {
        let order = names(&["a", "b", "c"]);
        assert_eq!(step_selection(&order, None, 1).as_deref(), Some("a"));
        assert_eq!(step_selection(&order, None, -1).as_deref(), Some("c"));
        assert_eq!(step_selection(&order, Some("c"), 1).as_deref(), Some("c"));
        assert_eq!(step_selection(&order, Some("b"), -1).as_deref(), Some("a"));
        assert_eq!(step_selection(&[], Some("b"), 1), None);
    }

    #[test]
    fn a_dispatch_line_names_codebase_and_provider() {
        let cbs = names(&["scipio-erp", "aivory-code"]);
        let ps = names(&["claude", "codex"]);
        let d = parse_dispatch("@scipio add refund tests with codex", &cbs, &ps);
        assert_eq!(d.codebase.as_deref(), Some("scipio-erp"));
        assert_eq!(d.provider.as_deref(), Some("codex"));
        assert_eq!(d.prompt, "add refund tests");
        let d = parse_dispatch("fix the build with nobody", &cbs, &ps);
        assert_eq!(d.codebase, None);
        assert_eq!(d.provider, None);
        assert_eq!(d.prompt, "fix the build with nobody");
        let d = parse_dispatch("@ x", &cbs, &ps);
        assert_eq!(d.codebase, None);
    }

    #[test]
    fn long_text_gets_an_ellipsis() {
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("abc", 4), "abc");
    }
}
