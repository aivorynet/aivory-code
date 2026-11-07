//! The Lens (`Lens.dc.html` frames 1 to 4): one file of a codebase with the turn blame, the file
//! heat of the live claims, the diagnostics, and a side panel (Outline, Problems, Timeline,
//! Radar). It opens from Repos -> Files and from the Session deep dive (a click on a path).
//!
//! The code view asks `code/file` for the highlights of the visible lines plus a margin, and
//! asks again when the user scrolls past them. A hover on a line shows the blame by prompt; a
//! hover on a symbol shows `code/hover`.

mod code;
pub mod logic;
mod overlays;
mod panel;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::code::{
    BlameParams, CodeFile, CodeFileParams, CodeSource, DiagnosticsParams, DiagnosticsResult,
    Finding, Hover, LineRange, Place, PlacePage, Symbol, TimelineParams,
};
use aivory_protocol::coord::{MailItem, MailListParams};
use aivory_protocol::rpc::{CodebaseRef, RpcError, code as rpc_code, method};
use aivory_protocol::{FileNode, RepoDetail, SessionState, UnixMs};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, FocusHandle, FontWeight, KeyDownEvent, Render,
    SharedString, Subscription, Task, UniformListScrollHandle, Window, div, px, rgb, rgba,
    uniform_list,
};
use gpui_base::input::{InputEvent, InputState};

use crate::model::Model;
use crate::radar::{self, Coord};
use crate::theme::*;
use crate::ui;
use logic::{
    BlameMap, DiffRow, MARGIN, OutlineRow, Point, add_range, bare_name, fetch_for, file_name,
    flatten_outline, indent, lang_label, part_of, split_lines, timeline_points, visible_rows,
};

/// The poll of the claims, the radar, and the problems.
const POLL: Duration = Duration::from_secs(10);
/// The run of the fast diagnostic stages.
const DIAG_EVERY: Duration = Duration::from_secs(60);
/// The delay before a card closes after the pointer leaves the line and the card.
const CARD_CLOSE: Duration = Duration::from_millis(160);
/// The first request of a file: lines 1 to 60 and the margin.
const FIRST_LINES: usize = 60;
const SYMBOL_ROWS: u32 = 30;

// Surfaces of the lens that no other screen shares.
const ROW_HOVER: u32 = 0xF8F9FB;
const SELECTED: u32 = 0xEEF3FE;
const LINE_NO: u32 = 0xA0A7B5;
const CLAIM_BG: u32 = 0xEEF2FF;
const CLAIM_PALE_BG: u32 = 0xF5F6FD;
const CLAIM_PALE: u32 = 0x6B77C8;
const KICK: u32 = 0x2C4FA8;
const BUTTON_BORDER: u32 = 0xC9CCD6;
const BUTTON_BORDER_HOVER: u32 = 0x8A93A6;
const TAB_TRACK: u32 = 0xECEEF2;
const ADDED: u32 = 0x15803D;
const HEAT: u32 = 0x4B5BDB;

/// What the lens tells the shell.
#[derive(Debug, Clone)]
pub enum LensEvent {
    /// Back to the screen that opened the lens.
    Back,
    /// "Open session X": the deep dive.
    OpenSession(String),
    /// "Open both" of a radar hit.
    OpenSessions(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelTab {
    Outline,
    Problems,
    Timeline,
    Radar,
}

/// The tabs of the side panel, in design order.
const TABS: [(PanelTab, &str); 4] = [
    (PanelTab::Outline, "Outline"),
    (PanelTab::Problems, "Problems"),
    (PanelTab::Timeline, "Timeline"),
    (PanelTab::Radar, "Radar"),
];

/// The text of the open file.
pub(crate) struct FileData {
    pub language: String,
    pub lines: Vec<SharedString>,
    pub total: u32,
    pub truncated: bool,
    /// The tokens of each line; empty for a line that is not highlighted yet.
    pub tokens: Vec<Vec<aivory_protocol::code::Highlight>>,
    pub loaded: Vec<LineRange>,
    pub outline: Vec<OutlineRow>,
    /// The findings of `code/file`, joined with the rows of `code/diagnostics` for this file.
    pub findings: Vec<Finding>,
}

pub(crate) enum FileState {
    Loading,
    Ready(FileData),
    Failed(String),
}

/// The pointer is on this line, and on this symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hovered {
    pub line: u32,
    pub sym: Option<(Range<usize>, String)>,
}

/// The answer of `code/hover` and the counts behind the doc card.
pub(crate) enum HoverEntry {
    Loading,
    /// The engine knows no name here.
    Empty,
    Ready {
        hover: Box<Hover>,
        usages: Option<u32>,
        sessions: Option<u32>,
    },
}

/// The line under a result of a card button: a revert error, a fork result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Note {
    pub text: SharedString,
    pub error: bool,
    /// A session that the note offers to open.
    pub open: Option<String>,
}

pub(crate) enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(String),
}

/// The diff of a timeline point.
pub(crate) struct DiffState {
    pub point: Point,
    pub rows: Loadable<(Vec<DiffRow>, u32, u32)>,
    /// The daemon keeps no snapshot of the turn: the rows are the lines that the turn wrote
    /// and that the file still has (no removed lines).
    pub approx: bool,
}

/// The usages list under the header.
pub(crate) struct RefsState {
    pub name: String,
    pub page: Loadable<PlacePage>,
    pub selected: usize,
    pub scroll: gpui::ScrollHandle,
    /// Scroll the selected row into view once (after an arrow key).
    pub reveal: bool,
}

/// The "Go to symbol" palette.
pub(crate) struct Palette {
    pub input: Entity<InputState>,
    pub query: String,
    pub results: Vec<Symbol>,
    pub selected: usize,
    pub stamp: u64,
    pub _events: Subscription,
}

/// The callers and tests of the symbol at the cursor.
pub(crate) struct SideRefs {
    pub key: (String, u32),
    pub items: Loadable<Vec<Place>>,
}

pub(crate) struct Field {
    pub input: Entity<InputState>,
    pub _events: Subscription,
}

pub struct LensScreen {
    model: Entity<Model>,
    focus: FocusHandle,
    codebase: String,
    path: String,
    /// The session that opened the lens (`from session X`); its claims show no heat.
    from_session: Option<String>,
    back_label: SharedString,
    detail: Option<Rc<RepoDetail>>,
    coord: Option<Rc<Coord>>,
    problems: Option<Rc<DiagnosticsResult>>,
    mail: Vec<(String, Vec<MailItem>)>,
    file: FileState,
    blame: BlameMap,
    timeline: Vec<Point>,
    /// The engine of the last answer of a query.
    engine: Option<CodeSource>,
    tab: PanelTab,
    scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    tree_scroll: UniformListScrollHandle,
    tree_rows: Vec<usize>,
    collapsed: HashSet<String>,
    filter: Option<Field>,
    /// The line of the last click or jump.
    cursor: Option<u32>,
    pending_jump: Option<u32>,
    fetching: bool,
    /// Bumped at each file change: a late answer for the old file is dropped.
    generation: u64,
    hover: Option<Hovered>,
    hover_gen: u64,
    card_hover: bool,
    hovers: HashMap<(u32, usize), HoverEntry>,
    /// The claims of other sessions on the open file, with a range.
    heat: Vec<logic::Heat>,
    heat_hover: Option<u32>,
    note: Option<Note>,
    confirm_revert: bool,
    diff: Option<DiffState>,
    refs: Option<RefsState>,
    palette: Option<Palette>,
    side: Option<SideRefs>,
    /// When `code/diagnostics` last ran.
    diag_at: Option<Instant>,
    _poll: Task<()>,
}

impl EventEmitter<LensEvent> for LensScreen {}

impl LensScreen {
    pub fn new(
        model: Entity<Model>,
        codebase: String,
        path: String,
        from_session: Option<String>,
        back_label: &str,
        cx: &mut Context<Self>,
    ) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, |this, cx| this.refresh_side(cx)).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            model,
            focus: cx.focus_handle(),
            codebase,
            path,
            from_session,
            back_label: back_label.to_string().into(),
            detail: None,
            coord: None,
            problems: None,
            mail: Vec::new(),
            file: FileState::Loading,
            blame: BlameMap::default(),
            timeline: Vec::new(),
            engine: None,
            tab: PanelTab::Outline,
            scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            tree_scroll: UniformListScrollHandle::new(),
            tree_rows: Vec::new(),
            collapsed: HashSet::new(),
            filter: None,
            cursor: None,
            pending_jump: None,
            fetching: false,
            generation: 0,
            hover: None,
            hover_gen: 0,
            card_hover: false,
            hovers: HashMap::new(),
            heat: Vec::new(),
            heat_hover: None,
            note: None,
            confirm_revert: false,
            diff: None,
            refs: None,
            palette: None,
            side: None,
            diag_at: None,
            _poll: poll,
        };
        this.load_file(None, cx);
        this.refresh_side(cx);
        this
    }

    // ---- Loading ----

    fn session_arg(&self) -> Option<String> {
        self.from_session.clone()
    }

    /// Load the file, its blame, and its timeline.
    fn load_file(&mut self, jump: Option<u32>, cx: &mut Context<Self>) {
        self.generation += 1;
        let stamp = self.generation;
        self.file = FileState::Loading;
        self.blame = BlameMap::default();
        self.timeline.clear();
        self.diff = None;
        self.refs = None;
        self.hover = None;
        self.hovers.clear();
        self.side = None;
        self.note = None;
        self.confirm_revert = false;
        self.fetching = false;
        self.pending_jump = jump;
        self.cursor = jump;
        let (codebase, path, session) =
            (self.codebase.clone(), self.path.clone(), self.session_arg());
        let first = logic::request_range(0, FIRST_LINES, u32::MAX, MARGIN);
        let params = CodeFileParams {
            codebase: codebase.clone(),
            path: path.clone(),
            session: session.clone(),
            rev: None,
            lines: Some(first),
        };
        let call = self
            .model
            .read(cx)
            .call::<method::CodeFileMethod>(params.clone(), cx);
        cx.spawn(async move |this, cx| {
            let mut result = call.await;
            if result.is_err() && params.session.is_some() {
                // The session knows no worktree: read the main checkout.
                let retry = this
                    .update(cx, |this, cx| {
                        let mut p = params.clone();
                        p.session = None;
                        this.model.read(cx).call::<method::CodeFileMethod>(p, cx)
                    })
                    .ok();
                if let Some(retry) = retry {
                    result = retry.await;
                }
            }
            let _ = this.update(cx, |this, cx| this.file_loaded(stamp, result, cx));
        })
        .detach();

        let blame = self.model.read(cx).call::<method::LedgerBlame>(
            BlameParams {
                codebase: codebase.clone(),
                path: path.clone(),
                session: session.clone(),
                lines: None,
            },
            cx,
        );
        let timeline = self.model.read(cx).call::<method::LedgerTimeline>(
            TimelineParams {
                codebase,
                path,
                session,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let blame = blame.await;
            let timeline = timeline.await;
            let _ = this.update(cx, |this, cx| {
                if stamp != this.generation {
                    return;
                }
                match blame {
                    Ok(b) => {
                        let total = b.hunks.iter().map(|h| h.line + h.count).max().unwrap_or(0);
                        this.blame = BlameMap::new(b.hunks, total);
                    }
                    Err(e) => eprintln!("aivory: ledger/blame failed: {e:#}"),
                }
                match timeline {
                    Ok(t) => this.timeline = timeline_points(&t.events),
                    Err(e) => eprintln!("aivory: ledger/timeline failed: {e:#}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn file_loaded(
        &mut self,
        stamp: u64,
        result: anyhow::Result<CodeFile>,
        cx: &mut Context<Self>,
    ) {
        if stamp != self.generation {
            return;
        }
        self.file = match result {
            Ok(f) => {
                let lines: Vec<SharedString> =
                    split_lines(&f.text).into_iter().map(Into::into).collect();
                let mut tokens = vec![Vec::new(); lines.len()];
                for h in f.highlights {
                    if let Some(slot) = tokens.get_mut(h.line.saturating_sub(1) as usize) {
                        slot.push(h);
                    }
                }
                let mut loaded = Vec::new();
                add_range(&mut loaded, f.highlighted);
                FileState::Ready(FileData {
                    language: f.language,
                    total: f.total_lines.max(lines.len() as u32),
                    truncated: f.truncated,
                    lines,
                    tokens,
                    loaded,
                    outline: flatten_outline(&f.outline),
                    findings: f.diagnostics,
                })
            }
            Err(e) => FileState::Failed(error_text(&e)),
        };
        self.rebuild_findings();
        cx.notify();
    }

    /// Join the findings of `code/file` with the rows of `code/diagnostics` for this file.
    fn rebuild_findings(&mut self) {
        let Some(problems) = self.problems.clone() else {
            return;
        };
        if let FileState::Ready(f) = &mut self.file {
            for row in problems.rows.iter().filter(|r| r.path == self.path) {
                let known = f.findings.iter().any(|x| {
                    (!row.fingerprint.is_empty() && x.fingerprint == row.fingerprint)
                        || (x.line == row.line && x.col == row.col && x.rule == row.rule)
                });
                if !known {
                    f.findings.push(row.clone());
                }
            }
        }
    }

    /// Ask for the highlights of the lines that scroll into view.
    pub(crate) fn ensure_lines(&mut self, visible: Range<usize>, cx: &mut Context<Self>) {
        let FileState::Ready(f) = &self.file else {
            return;
        };
        if self.fetching {
            return;
        }
        let Some(want) = fetch_for(&f.loaded, visible.start, visible.end, f.total, MARGIN) else {
            return;
        };
        self.fetching = true;
        let stamp = self.generation;
        let call = self.model.read(cx).call::<method::CodeFileMethod>(
            CodeFileParams {
                codebase: self.codebase.clone(),
                path: self.path.clone(),
                session: self.session_arg(),
                rev: None,
                lines: Some(want),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                if stamp != this.generation {
                    return;
                }
                this.fetching = false;
                match result {
                    Ok(resp) => {
                        if let FileState::Ready(f) = &mut this.file {
                            for h in resp.highlights {
                                if let Some(slot) =
                                    f.tokens.get_mut(h.line.saturating_sub(1) as usize)
                                {
                                    slot.push(h);
                                }
                            }
                            // Lines that two answers cover hold their tokens twice.
                            for ix in resp.highlighted.from..=resp.highlighted.to {
                                if let Some(slot) = f.tokens.get_mut(ix.saturating_sub(1) as usize)
                                {
                                    slot.sort_by_key(|t| (t.start, t.end));
                                    slot.dedup_by_key(|t| (t.start, t.end));
                                }
                            }
                            add_range(&mut f.loaded, resp.highlighted);
                            // An answer that is shorter than the request must not repeat it.
                            add_range(&mut f.loaded, want);
                        }
                    }
                    Err(e) => {
                        // Do not ask again for the same lines.
                        if let FileState::Ready(f) = &mut this.file {
                            add_range(&mut f.loaded, want);
                        }
                        eprintln!("aivory: code/file failed: {e:#}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Load the repo detail, the claims, the radar, the problems, and the mail of the sessions
    /// that a hit on this file names.
    fn refresh_side(&mut self, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let detail = model.read(cx).call::<method::ReposGet>(
            CodebaseRef {
                id: self.codebase.clone(),
            },
            cx,
        );
        let coord = radar::fetch(&model, &self.codebase, cx);
        // The run of the fast stages is the costly call: once each minute.
        let with_diag = self.diag_at.is_none_or(|t| t.elapsed() >= DIAG_EVERY);
        if with_diag {
            self.diag_at = Some(Instant::now());
        }
        let diag = with_diag.then(|| {
            model.read(cx).call::<method::CodeDiagnostics>(
                DiagnosticsParams {
                    codebase: self.codebase.clone(),
                    session: self.session_arg(),
                    turn: None,
                    path: None,
                },
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            let detail = detail.await;
            let coord = coord.await;
            let diag = match diag {
                Some(call) => Some(call.await),
                None => None,
            };
            let targets = this
                .update(cx, |this, _| this.mail_targets(&coord))
                .unwrap_or_default();
            let mut mail = Vec::new();
            for session in targets {
                let call = this.update(cx, |this, cx| {
                    this.model.read(cx).call::<method::MailListMethod>(
                        MailListParams {
                            session: session.clone(),
                        },
                        cx,
                    )
                });
                if let Ok(call) = call
                    && let Ok(list) = call.await
                {
                    mail.push((session, list.items));
                }
            }
            let _ = this.update(cx, |this, cx| {
                match detail {
                    Ok(d) => {
                        this.detail = Some(Rc::new(d));
                    }
                    Err(e) => eprintln!("aivory: repos/get failed: {e:#}"),
                }
                this.coord = Some(Rc::new(coord));
                match diag {
                    Some(Ok(d)) => this.problems = Some(Rc::new(d)),
                    Some(Err(e)) => eprintln!("aivory: code/diagnostics failed: {e:#}"),
                    None => {}
                }
                this.mail = mail;
                this.rebuild_findings();
                cx.notify();
            });
        })
        .detach();
    }

    /// The sessions whose mailbox the Radar tab reads: the sessions of the hits on this file,
    /// and the opening session.
    fn mail_targets(&self, coord: &Coord) -> Vec<String> {
        let mut ids: Vec<String> = self.from_session.iter().cloned().collect();
        for hit in logic::hits_for_file(&coord.hits, &coord.claims, &self.path) {
            for id in [&hit.a, &hit.b] {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
        ids.truncate(4);
        ids
    }

    // ---- Navigation ----

    /// Show a file of this codebase, at a line when given.
    pub(crate) fn open_path(&mut self, path: &str, line: Option<u32>, cx: &mut Context<Self>) {
        if path == self.path && !matches!(self.file, FileState::Failed(_)) {
            if let Some(line) = line {
                self.jump_to(line, cx);
            }
            self.diff = None;
            self.refs = None;
            cx.notify();
            return;
        }
        self.path = path.to_string();
        self.load_file(line, cx);
    }

    /// Scroll the code view to a line and put the cursor on it.
    pub(crate) fn jump_to(&mut self, line: u32, cx: &mut Context<Self>) {
        self.cursor = Some(line);
        self.pending_jump = Some(line);
        self.diff = None;
        cx.notify();
    }

    fn close_overlays(&mut self, cx: &mut Context<Self>) {
        self.hover = None;
        self.card_hover = false;
        self.note = None;
        self.confirm_revert = false;
        cx.notify();
    }

    // ---- Findings and claims of the open file ----

    pub(crate) fn findings(&self) -> &[Finding] {
        match &self.file {
            FileState::Ready(f) => &f.findings,
            _ => &[],
        }
    }

    pub(crate) fn claims(&self) -> &[aivory_protocol::coord::Claim] {
        self.coord.as_ref().map_or(&[], |c| c.claims.as_slice())
    }

    fn files(&self) -> &[FileNode] {
        self.detail.as_ref().map_or(&[], |d| d.files.as_slice())
    }

    /// The problems per path: the rows of `code/diagnostics`.
    fn problem_count(&self, path: &str) -> u32 {
        let mut n = self.problems.as_ref().map_or(0, |p| {
            p.rows.iter().filter(|r| r.path == path).count() as u32
        });
        if n == 0 && path == self.path {
            n = self.findings().len() as u32;
        }
        n
    }

    /// The state of a session in the war room.
    fn session_state(&self, id: &str, cx: &gpui::App) -> Option<SessionState> {
        self.model
            .read(cx)
            .war
            .as_ref()?
            .session(id)
            .map(|s| s.state)
    }

    /// The session that claims the open file: the strongest live claim.
    fn claimant(&self) -> Option<String> {
        radar::claim_pill(self.claims(), &self.path)
            .map(|(s, _)| s)
            .or_else(|| {
                self.files()
                    .iter()
                    .find(|f| f.path == self.path)
                    .and_then(|f| f.claimed_by.clone())
            })
    }

    pub(crate) fn clock(&self, at: UnixMs, cx: &gpui::App) -> String {
        self.model.read(cx).clock(at)
    }

    fn now(&self, cx: &gpui::App) -> UnixMs {
        self.model
            .read(cx)
            .war
            .as_ref()
            .map_or_else(|| chrono::Utc::now().timestamp_millis(), |w| w.now)
    }

    // ---- Keys ----

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        let secondary = ev.keystroke.modifiers.secondary();
        if secondary && key == "o" {
            cx.stop_propagation();
            self.open_palette(window, cx);
            return;
        }
        if secondary && key == "p" {
            cx.stop_propagation();
            let input = self.filter_input(window, cx);
            input.update(cx, |s, cx| s.focus(window, cx));
            return;
        }
        if self.palette.is_some() {
            match key {
                "escape" => self.palette = None,
                "up" => self.move_palette(-1),
                "down" => self.move_palette(1),
                _ => return,
            }
            cx.stop_propagation();
            self.focus.focus(window, cx);
            cx.notify();
            return;
        }
        if self.refs.is_some() {
            match key {
                "escape" => self.refs = None,
                "up" => self.move_refs(-1),
                "down" => self.move_refs(1),
                "enter" => self.open_selected_ref(cx),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "escape" {
            if self.hover.is_some() || self.note.is_some() {
                self.close_overlays(cx);
            } else if self.diff.is_some() {
                self.diff = None;
                cx.notify();
            }
        }
    }

    // ---- The text filter of the tree ----

    fn filter_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(f) = &self.filter {
            return f.input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Go to file"));
        let events = cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        self.filter = Some(Field {
            input: input.clone(),
            _events: events,
        });
        input
    }

    // ---- Header ----

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let back = div()
            .id("lens-back")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .py(px(6.))
            .pl(px(6.))
            .pr(px(10.))
            .rounded(tk().r_button)
            .text_size(crate::ui::ts(crate::theme::T_BODY))
            .text_color(rgb(tk().ink_2))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(tk().track)).text_color(rgb(tk().ink)))
            .when(ui::irix(), |d| {
                d.rounded(px(0.))
                    .bg(rgb(0xC8C8C8))
                    .text_color(rgb(0x111111))
            })
            .when(ui::irix(), |d| ui::bevel_edges(true, d))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(LensEvent::Back)))
            .child(ui::icon("icons/back.svg", 14., rgb(tk().ink_2)))
            .child(self.back_label.clone());
        let branch = self
            .detail
            .as_ref()
            .map(|d| d.codebase.branch.clone())
            .unwrap_or_default();
        let part = part_of(self.files(), &self.path).map(str::to_string);
        let crumb = match (part, branch.is_empty()) {
            (Some(p), false) => format!("{} / {p} · {branch}", self.codebase),
            (Some(p), true) => format!("{} / {p}", self.codebase),
            (None, false) => format!("{} · {branch}", self.codebase),
            (None, true) => self.codebase.clone(),
        };
        let language = match &self.file {
            FileState::Ready(f) => Some(lang_label(&f.language)),
            _ => None,
        };
        let claimant = self.claimant();
        let open_target = claimant.clone().or_else(|| self.from_session.clone());
        let mut header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(32.))
            .py(px(12.))
            .h(HEADER_H)
            .bg(rgb(tk().card))
            .border_b_1()
            .border_color(rgba(tk().line))
            .overflow_hidden()
            .child(back)
            .child(
                div()
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .whitespace_nowrap()
                    .child(crumb),
            )
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_TITLE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .child(file_name(&self.path).to_string()),
            );
        if let Some(l) = language {
            header = header.child(header_pill(l, false));
        }
        if let Some(c) = claimant {
            header = header.child(header_pill(format!("claimed by {c}"), true));
        }
        if let Some(s) = &self.from_session {
            header = header.child(header_pill(format!("from session {s}"), false));
        }
        header = header.child(div().flex_1()).child(
            header_button("lens-symbol", "Go to symbol")
                .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx)))
                .child(
                    div()
                        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                        .text_color(rgb(tk().muted))
                        .child(SYMBOL_KEY),
                ),
        );
        if let Some(id) = open_target {
            let label = format!("Open session {id}");
            header = header.child(header_button("lens-open", &label).on_click(
                cx.listener(move |_, _, _, cx| cx.emit(LensEvent::OpenSession(id.clone()))),
            ));
        }
        header
    }

    // ---- Tree ----

    fn tree(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let input = self.filter_input(window, cx);
        let query = input.read(cx).value().to_string();
        let rows = visible_rows(self.files(), &self.collapsed, &query);
        self.tree_rows = rows;
        let parts = self.detail.as_ref().map_or(0, |d| d.codebase.parts.len());
        let branch = self
            .detail
            .as_ref()
            .map(|d| d.codebase.branch.clone())
            .unwrap_or_default();
        let meta = if branch.is_empty() {
            format!("{} · {}", self.codebase, plural(parts, "part"))
        } else {
            format!("{} · {branch} · {}", self.codebase, plural(parts, "part"))
        };
        let count = self.tree_rows.len();
        let list: AnyElement = if self.detail.is_none() {
            note_line("Loads the files from aivoryd…")
        } else if count == 0 {
            note_line(if query.is_empty() {
                "No files mapped yet."
            } else {
                "No file matches."
            })
        } else {
            uniform_list(
                "lens-tree",
                count,
                cx.processor(|this, range: Range<usize>, _, cx| {
                    range.map(|i| this.tree_row(i, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.tree_scroll)
            .flex_1()
            .min_h(px(0.))
            .into_any_element()
        };
        div()
            .w(px(300.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(tk().card))
            .border_r_1()
            .border_color(rgba(tk().line))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .pt(px(12.))
                    .px(px(14.))
                    .pb(px(8.))
                    .child(
                        div()
                            .h(px(32.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .rounded(tk().r_button)
                            .border_1()
                            .border_color(rgba(tk().line_2))
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .text_color(rgb(tk().muted))
                            .child(ui::icon("icons/search.svg", 13., rgb(tk().muted)))
                            .child(div().flex_1().min_w(px(0.)).child(input))
                            .child(
                                div()
                                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                                    .child(FILE_KEY),
                            ),
                    )
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .whitespace_nowrap()
                            .child(meta),
                    ),
            )
            .child(list)
            .into_any_element()
    }

    fn tree_row(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(node) = self.tree_rows.get(i).and_then(|&n| self.files().get(n)) else {
            return div().into_any_element();
        };
        let on = !node.dir && node.path == self.path;
        let collapsed = self.collapsed.contains(&node.path);
        let icon = if node.dir {
            ui::icon(
                if collapsed {
                    "icons/twist-right.svg"
                } else {
                    "icons/twist-down.svg"
                },
                10.,
                rgb(tk().muted),
            )
            .into_any_element()
        } else {
            div().child("·").into_any_element()
        };
        let claim = if node.dir {
            None
        } else {
            radar::claim_pill(self.claims(), &node.path)
                .map(|(s, _)| s)
                .or_else(|| node.claimed_by.clone())
                .map(|s| {
                    let needs = self.session_state(&s, cx) == Some(SessionState::NeedsYou)
                        || (node.claimed_by.as_deref() == Some(s.as_str())
                            && node.claim_state == Some(SessionState::NeedsYou));
                    (s, needs)
                })
        };
        let problems = if node.dir {
            0
        } else {
            self.problem_count(&node.path)
        };
        let path = node.path.clone();
        let dir = node.dir;
        div()
            .id(("lens-file", i))
            .w_full()
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(indent(node.depth)))
            .pr(px(12.))
            .font_family(tk().mono)
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .cursor_pointer()
            .when(on, |d| d.bg(rgb(SELECTED)))
            .when(!on, |d| d.hover(|s| s.bg(rgb(ROW_HOVER))))
            .on_click(cx.listener(move |this, _, _, cx| {
                if dir {
                    if !this.collapsed.remove(&path) {
                        this.collapsed.insert(path.clone());
                    }
                    cx.notify();
                } else {
                    this.open_path(&path, None, cx);
                }
            }))
            .child(
                div()
                    .w(px(14.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .text_color(rgb(tk().muted))
                    .child(icon),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(node.name.clone()),
            )
            .when_some(claim, |d, (s, needs)| {
                d.child(claim_pill_el(
                    &format!("claimed by {s}"),
                    if needs {
                        ClaimTone::Needs
                    } else {
                        ClaimTone::Plain
                    },
                ))
            })
            .when_some(node.part.clone(), |d, p| d.child(part_tag(p)))
            .when(problems > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                        .text_color(rgb(tk().e_text))
                        .child(div().size(px(6.)).rounded_full().bg(rgb(tk().e)))
                        .child(problems.to_string()),
                )
            })
            .into_any_element()
    }
}

impl Render for LensScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Keys reach the lens while nothing else has the focus.
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        let width = window.viewport_size().width;
        let show_panel = width >= px(1100.);
        let show_tree = width >= NARROW;
        let header = self.header(cx);
        let tree = show_tree.then(|| self.tree(window, cx));
        let center = self.center(cx);
        let irix = ui::irix();
        let mut body = div().flex_1().min_h(px(0.)).flex().relative();
        if irix {
            // Sunken wells on the grey desktop: the tree, the code, the panel.
            let well = |el: AnyElement, grow: bool| {
                let w = div().h_full().min_w_0().flex().overflow_hidden();
                let w = if grow { w.flex_1() } else { w.flex_none() };
                ui::bevel_edges(false, w.bg(rgb(0xFFFFFF)).p(px(2.)).child(el))
            };
            body = body.p(px(16.)).gap(px(16.));
            if let Some(t) = tree {
                body = body.child(well(t, false));
            }
            body = body.child(well(center.into_any_element(), true));
            if show_panel {
                let panel = self.panel(cx).into_any_element();
                body = body.child(well(panel, false));
            }
        } else {
            body = body.when_some(tree, |d, t| d.child(t)).child(center);
            if show_panel {
                body = body.child(self.panel(cx));
            }
        }
        if self.refs.is_some() {
            body = body.child(self.refs_overlay(show_tree, show_panel, cx));
        }
        if self.palette.is_some() {
            body = body.child(self.palette_overlay(cx));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .line_height(gpui::relative(1.3))
            .track_focus(&self.focus)
            .capture_key_down(
                cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)),
            )
            .child(header)
            .child(body)
    }
}

// ---- Widgets ----

#[cfg(target_os = "macos")]
const SYMBOL_KEY: &str = "⌘O";
#[cfg(not(target_os = "macos"))]
const SYMBOL_KEY: &str = "Ctrl O";
#[cfg(target_os = "macos")]
const FILE_KEY: &str = "⌘P";
#[cfg(not(target_os = "macos"))]
const FILE_KEY: &str = "Ctrl P";

/// The tone of a claim pill: plain (blue), needs you (orange), or pale (an intent claim).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimTone {
    Plain,
    Needs,
    Pale,
}

pub(crate) fn claim_pill_el(text: &str, tone: ClaimTone) -> Div {
    let (bg, fg) = match tone {
        ClaimTone::Plain => (CLAIM_BG, tk().link),
        ClaimTone::Needs => (tk().q_bg, tk().q_text),
        ClaimTone::Pale => (CLAIM_PALE_BG, CLAIM_PALE),
    };
    div()
        .flex_none()
        .px(px(8.))
        .py(px(1.))
        .rounded_full()
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .whitespace_nowrap()
        .child(text.to_string())
}

/// The part tag (`.ptag`).
pub(crate) fn part_tag(text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(px(8.))
        .py(px(1.))
        .rounded_full()
        .bg(rgb(tk().track))
        .text_color(rgb(tk().ink_2))
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .whitespace_nowrap()
        .child(text.into())
}

/// The header pill: grey (`.pill.gr`), or blue for a claim (`.pill.cl`).
fn header_pill(text: impl Into<SharedString>, blue: bool) -> Div {
    div()
        .h(px(24.))
        .px(px(10.))
        .flex()
        .flex_none()
        .items_center()
        .rounded_full()
        .bg(rgb(if blue { CLAIM_BG } else { tk().track }))
        .text_color(rgb(if blue { tk().link } else { tk().ink_2 }))
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .whitespace_nowrap()
        .when(ui::irix(), |d| d.rounded(px(0.)).bg(rgb(0xFFFFFF)))
        .when(ui::irix(), |d| ui::bevel_edges(false, d))
        .child(text.into())
}

/// The header button with a thin border (`.hb`).
fn header_button(id: &'static str, label: &str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(32.))
        .px(px(12.))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .rounded(tk().r_button)
        .border_1()
        .border_color(rgba(tk().line_2))
        .text_size(crate::ui::ts(crate::theme::T_BODY))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(tk().ink)))
        .when(ui::irix(), |d| {
            d.rounded(px(0.))
                .border_0()
                .bg(rgb(0xC8C8C8))
                .text_color(rgb(0x111111))
        })
        .when(ui::irix(), |d| ui::bevel_edges(true, d))
        .child(label.to_string())
}

/// A button of the cards and bars (`.gb`, `.gb.sm`).
pub(crate) fn gb_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    small: bool,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(if small { 26. } else { 30. }))
        .px(px(if small { 10. } else { 12. }))
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(BUTTON_BORDER))
        .bg(rgb(tk().card))
        .text_size(crate::ui::ts(if small {
            crate::theme::T_CAPTION
        } else {
            crate::theme::T_SMALL
        }))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(BUTTON_BORDER_HOVER)))
        .when(ui::irix(), |d| {
            d.rounded(px(0.))
                .border_0()
                .bg(rgb(0xC8C8C8))
                .text_color(rgb(0x111111))
        })
        .when(ui::irix(), |d| ui::bevel_edges(true, d))
        .child(label.into())
}

/// The blue `.gb.pri`.
pub(crate) fn gb_primary(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
) -> gpui::Stateful<Div> {
    gb_button(id, label, false)
        .border_color(rgb(tk().primary))
        .bg(rgb(tk().primary))
        .text_color(rgb(tk().card))
        .font_weight(FontWeight::MEDIUM)
        .hover(|s| {
            s.bg(rgb(tk().primary_hover))
                .border_color(rgb(tk().primary_hover))
        })
}

fn note_line(text: &'static str) -> AnyElement {
    div()
        .flex_none()
        .px(px(16.))
        .py(px(14.))
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .text_color(rgb(tk().muted))
        .child(text)
        .into_any_element()
}

/// The section title of the side panel (`.sec`).
pub(crate) fn section(title: &str, meta: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .flex()
        .gap(px(8.))
        .pt(px(14.))
        .px(px(16.))
        .pb(px(6.))
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(tk().ink_3))
        .child(title.to_uppercase())
        .child(div().font_weight(FontWeight::MEDIUM).child(meta.into()))
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// The text of a daemon error, without the wrapper of `anyhow`.
pub(crate) fn error_text(e: &anyhow::Error) -> String {
    match e.downcast_ref::<RpcError>() {
        Some(r) => r.message.clone(),
        None => format!("{e:#}"),
    }
}

/// True for the error that the daemon sends for a session in the main checkout.
pub(crate) fn is_invalid_state(e: &anyhow::Error) -> bool {
    e.downcast_ref::<RpcError>()
        .is_some_and(|r| r.code == rpc_code::INVALID_STATE)
}

/// `Invoice::total` from a hover.
pub(crate) fn hover_title(hover: &Hover) -> String {
    let name = if hover.name.is_empty() {
        bare_name(hover.signature.as_deref().unwrap_or(""))
    } else {
        hover.name.as_str()
    };
    match hover
        .definitions
        .first()
        .and_then(|d| d.container.as_deref())
    {
        Some(c) if !c.is_empty() => format!("{c}::{name}"),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::logic::{BLAME_COLORS, diff_lines, revs_of, split_lines, timeline_points};
    use super::*;
    use crate::daemon;
    use aivory_protocol::code::{CodeFileParams, TimelineParams};
    use aivory_protocol::rpc::method;

    const PATH: &str = "crates/orch/src/budget.rs";

    fn file_params(rev: Option<&str>, lines: Option<LineRange>) -> CodeFileParams {
        CodeFileParams {
            codebase: "aivory-code".into(),
            path: PATH.into(),
            session: Some("a01c".into()),
            rev: rev.map(str::to_string),
            lines,
        }
    }

    #[test]
    fn the_demo_daemon_serves_the_data_of_the_lens() {
        let endpoint = daemon::start_demo().expect("demo daemon");
        let first = logic::request_range(0, FIRST_LINES, u32::MAX, MARGIN);
        let file =
            daemon::call::<method::CodeFileMethod>(&endpoint, &file_params(None, Some(first)))
                .expect("code/file");
        assert_eq!(lang_label(&file.language), "Rust");
        assert_eq!(file.total_lines as usize, split_lines(&file.text).len());
        assert!(!file.highlights.is_empty() && !file.outline.is_empty());
        assert!(file.highlighted.from == 1 && file.highlighted.to >= 1);

        // The blame scale of the three demo turns: newest turn is rank 0, the commit is last.
        let blame = daemon::call::<method::LedgerBlame>(
            &endpoint,
            &BlameParams {
                codebase: "aivory-code".into(),
                path: PATH.into(),
                session: Some("a01c".into()),
                lines: None,
            },
        )
        .expect("ledger/blame");
        let map = BlameMap::new(blame.hunks, file.total_lines);
        let text: Vec<String> = split_lines(&file.text);
        // No snapshots in the demo: the lines of turn 3 stand in for its diff.
        let stand_in = logic::turn_lines(&text, &map, "a01c", 3, 3);
        assert_eq!(stand_in.added, 5);
        assert_eq!(map.color_at(1), BLAME_COLORS[4], "line 1 is a commit");
        assert_eq!(map.color_at(30), BLAME_COLORS[0], "line 30 is turn 3");
        assert_eq!(map.color_at(20), BLAME_COLORS[1], "line 20 is turn 2");
        assert_eq!(map.color_at(8), BLAME_COLORS[2], "line 8 is turn 1");

        let timeline = daemon::call::<method::LedgerTimeline>(
            &endpoint,
            &TimelineParams {
                codebase: "aivory-code".into(),
                path: PATH.into(),
                session: Some("a01c".into()),
            },
        )
        .expect("ledger/timeline");
        let points = timeline_points(&timeline.events);
        assert_eq!(points.len(), 4);
        assert_eq!(points[0].who, "a01c · turn 3");
        assert_eq!(points[0].color, Some(BLAME_COLORS[0]));
        assert_eq!(points[3].color, None, "the commit has no blame color");
        let (old, new) = revs_of(&points[0].kind);
        assert_eq!((old.as_str(), new.as_str()), ("turn:2", "turn:3"));

        // The diff of a turn needs the file at two revisions; the demo may not keep them.
        let one = LineRange { from: 1, to: 1 };
        let old = daemon::call::<method::CodeFileMethod>(
            &endpoint,
            &file_params(Some("turn:2"), Some(one)),
        );
        let new = daemon::call::<method::CodeFileMethod>(
            &endpoint,
            &file_params(Some("turn:3"), Some(one)),
        );
        if let (Ok(old), Ok(new)) = (old, new) {
            let d = diff_lines(&split_lines(&old.text), &split_lines(&new.text), 3);
            assert_eq!(d.rows.is_empty(), old.text == new.text);
        }
    }
}
