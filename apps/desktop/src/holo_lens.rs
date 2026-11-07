//! The Holo Lens screen: one codebase seen as a map. The left panel is what you pick: the
//! parts, the hot files, the search. The right panel is the deep dive of the picked file: its
//! functions and who calls them. A tab with a small triangle picks the codebase. The data
//! comes from `repos/get`, `code/file`, `code/references` and `code/symbols`; nothing here
//! reads a real session. It follows the structure of the Bridge Lens screen.

use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::code::{
    CodeFile, CodeFileParams, CodePosParams, PlacePage, SymbolPage, SymbolsParams,
};
use aivory_protocol::rpc::{CodebaseRef, method};
use aivory_protocol::{FileNode, RepoDetail};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Div, Entity, EventEmitter, PathBuilder, SharedString, Stateful,
    Subscription, Window, canvas, div, point, px, rgb,
};
use gpui_base::input::{InputEvent, InputState};

use crate::holo_kit::*;
use crate::lens::logic::{OutlineRow, bare_name, flatten_outline};
use crate::model::Model;

/// How old a read gets before the view asks again.
const TTL: Duration = Duration::from_secs(15);
/// How long the charts take to grow in.
const GROW_MS: f32 = 700.;
/// A part counts as recent when a file changed in this time.
const RECENT_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone)]
pub enum HoloLensEvent {
    /// Back to the screen that opened the lens.
    Back,
    /// Open the session view of a session.
    OpenSession(String),
}

impl EventEmitter<HoloLensEvent> for HoloLens {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Left {
    Map,
    Hot,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deep {
    Functions,
    Callers,
}

/// A read: the key it answers, the answer (`None` when it failed), and when it came.
type Read<T> = Option<(String, Option<T>, Instant)>;

pub struct HoloLens {
    model: Entity<Model>,
    /// The codebase on view; the first one when `None`.
    codebase: Option<String>,
    /// The session that opened the lens: its claims show in the file reads.
    from_session: Option<String>,
    left: Left,
    /// The part whose files the map lists.
    part: Option<String>,
    picker: bool,
    path: Option<String>,
    deep: Deep,
    /// The line of the function whose callers show.
    func: Option<u32>,
    detail: Read<Rc<RepoDetail>>,
    file: Read<Rc<CodeFile>>,
    refs: Read<PlacePage>,
    found: Read<SymbolPage>,
    busy: [bool; 4],
    input: Entity<InputState>,
    grow_at: Instant,
    _subs: Vec<Subscription>,
}

impl HoloLens {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find a function or type"));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.search(cx);
            }
        });
        Self {
            model,
            codebase: None,
            from_session: None,
            left: Left::Map,
            part: None,
            picker: false,
            path: None,
            deep: Deep::Functions,
            func: None,
            detail: None,
            file: None,
            refs: None,
            found: None,
            busy: [false; 4],
            input,
            grow_at: Instant::now(),
            _subs: vec![observe, events],
        }
    }

    /// Show a codebase, and a file of it when `path` is not empty.
    pub fn show(
        &mut self,
        codebase: &str,
        path: &str,
        session: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.codebase.as_deref() != Some(codebase) {
            self.part = None;
            self.detail = None;
            self.found = None;
        }
        self.codebase = Some(codebase.to_string());
        self.from_session = session;
        self.picker = false;
        self.pick(if path.is_empty() { None } else { Some(path) });
        cx.notify();
    }

    /// Pick a file for the deep dive, or none.
    fn pick(&mut self, path: Option<&str>) {
        self.path = path.map(str::to_string);
        self.file = None;
        self.refs = None;
        self.func = None;
        self.deep = Deep::Functions;
        self.grow_at = Instant::now();
    }

    fn codebase_id(&self, cx: &App) -> Option<String> {
        self.codebase.clone().or_else(|| {
            self.model
                .read(cx)
                .war
                .as_ref()
                .and_then(|w| w.codebases.first().map(|c| c.id.clone()))
        })
    }

    fn stale<T>(read: &Read<T>, key: &str) -> bool {
        !read
            .as_ref()
            .is_some_and(|(k, _, t)| k == key && t.elapsed() <= TTL)
    }

    /// Ask for what the screen shows and does not hold yet.
    fn ensure(&mut self, codebase: &str, cx: &mut Context<Self>) {
        if !self.busy[0] && Self::stale(&self.detail, codebase) {
            self.busy[0] = true;
            let task = self.model.read(cx).call::<method::ReposGet>(
                CodebaseRef {
                    id: codebase.to_string(),
                },
                cx,
            );
            let key = codebase.to_string();
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.busy[0] = false;
                    this.detail = Some((key, result.ok().map(Rc::new), Instant::now()));
                    cx.notify();
                });
            })
            .detach();
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        let key = format!("{codebase}\n{path}");
        if !self.busy[1] && self.file.as_ref().is_none_or(|(k, _, _)| *k != key) {
            self.busy[1] = true;
            let task = self.model.read(cx).call::<method::CodeFileMethod>(
                CodeFileParams {
                    codebase: codebase.to_string(),
                    path,
                    session: self.from_session.clone(),
                    rev: None,
                    lines: None,
                },
                cx,
            );
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.busy[1] = false;
                    this.file = Some((key, result.ok().map(Rc::new), Instant::now()));
                    this.grow_at = Instant::now();
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// Ask for the callers of the function at `line` of the open file.
    fn ensure_refs(&mut self, codebase: &str, line: u32, name: &str, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let key = format!("{codebase}\n{path}\n{line}");
        if self.busy[2] || self.refs.as_ref().is_some_and(|(k, _, _)| *k == key) {
            return;
        }
        let col = self
            .file
            .as_ref()
            .and_then(|(_, f, _)| f.as_ref())
            .and_then(|f| f.text.lines().nth(line.saturating_sub(1) as usize))
            .and_then(|l| l.find(name))
            .unwrap_or(0) as u32;
        self.busy[2] = true;
        let task = self.model.read(cx).call::<method::CodeReferences>(
            CodePosParams {
                codebase: codebase.to_string(),
                path,
                line,
                col,
                session: self.from_session.clone(),
                page: None,
                per_page: Some(20),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy[2] = false;
                this.refs = Some((key, result.ok(), Instant::now()));
                cx.notify();
            });
        })
        .detach();
    }

    /// Search the symbols of the codebase for the text in the field.
    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().trim().to_string();
        let Some(codebase) = self.codebase_id(cx) else {
            return;
        };
        if query.is_empty() || self.busy[3] {
            return;
        }
        self.busy[3] = true;
        let task = self.model.read(cx).call::<method::CodeSymbols>(
            SymbolsParams {
                codebase,
                query: query.clone(),
                kind: None,
                session: None,
                page: None,
                per_page: Some(20),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy[3] = false;
                this.found = Some((query, result.ok(), Instant::now()));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// The part of a glob up to its first wildcard: `src/payment/**` gives `src/payment/`.
fn prefix(glob: &str) -> &str {
    let end = glob.find(['*', '?', '[']).unwrap_or(glob.len());
    &glob[..end]
}

fn in_part(path: &str, globs: &[String]) -> bool {
    globs.iter().any(|g| path.starts_with(prefix(g)))
}

/// `14 min ago`, `3 h ago`, `2 d ago`.
fn ago(now: i64, t: i64) -> String {
    let m = ((now - t) / 60_000).max(0);
    match m {
        0 => "now".to_string(),
        1..=59 => format!("{m} min ago"),
        60..=1439 => format!("{} h ago", m / 60),
        _ => format!("{} d ago", m / 1440),
    }
}

/// A scroll box that fills the rest of its parent.
fn scroll_box(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
}

/// A small filled triangle that points down.
fn caret(c: u32) -> Div {
    div().flex_none().ml(px(8.)).w(px(12.)).h(px(8.)).child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let x = f32::from(bounds.origin.x);
                let y = f32::from(bounds.origin.y);
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let mut path = PathBuilder::fill();
                path.move_to(point(px(x), px(y)));
                path.line_to(point(px(x + w), px(y)));
                path.line_to(point(px(x + w / 2.), px(y + h)));
                path.line_to(point(px(x), px(y)));
                if let Ok(path) = path.build() {
                    window.paint_path(path, rgb(c));
                }
            },
        )
        .size_full(),
    )
}

/// A bar that grows to `share` (0 to 1) of `width`.
fn meter(share: f32, width: f32, c: u32) -> Div {
    div()
        .h(px(6.))
        .w(px(width))
        .bg(rgb(RULE))
        .child(div().h_full().w(px(width * share.clamp(0., 1.))).bg(rgb(c)))
}

impl Render for HoloLens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let art = Art::of(window);
        let war = self.model.read(cx).war.clone();
        let Some(codebase) = self.codebase_id(cx) else {
            return island(
                art,
                Island {
                    stats: Vec::new(),
                    left_title: "Map",
                    left_color: PK,
                    left_tabs: Vec::new(),
                    left: big("No codebase yet").into_any_element(),
                    right_title: "Deep dive".into(),
                    right_tabs: Vec::new(),
                    right: div().into_any_element(),
                },
            );
        };
        self.ensure(&codebase, cx);
        let now = war.as_ref().map_or(0, |w| w.now);
        let grow = ease(self.grow_at.elapsed(), GROW_MS);
        if grow < 1. {
            window.request_animation_frame();
        }
        let detail: Option<Rc<RepoDetail>> = self
            .detail
            .as_ref()
            .filter(|(k, _, _)| *k == codebase)
            .and_then(|(_, d, _)| d.clone());
        let file: Option<Rc<CodeFile>> = self
            .file
            .as_ref()
            .filter(|(k, _, _)| k.starts_with(&format!("{codebase}\n")))
            .and_then(|(_, f, _)| f.clone());

        // ---- stats -------------------------------------------------------------------------
        let nodes: Vec<&FileNode> = detail
            .as_ref()
            .map(|d| d.files.iter().filter(|f| !f.dir).collect())
            .unwrap_or_default();
        let hot: Vec<&FileNode> = nodes
            .iter()
            .copied()
            .filter(|f| f.claimed_by.is_some())
            .collect();
        let last = detail
            .as_ref()
            .and_then(|d| d.parts.iter().filter_map(|p| p.last_change).max());
        let stats = vec![
            ("Codebase".to_string(), codebase.clone(), TXT),
            ("Files".to_string(), nodes.len().to_string(), TXT),
            (
                "Hot files".to_string(),
                hot.len().to_string(),
                if hot.is_empty() { TXT } else { PE },
            ),
            (
                "Last change".to_string(),
                last.map_or("none".to_string(), |t| ago(now, t)),
                TXT,
            ),
        ];

        // ---- left: the picker, the parts, the hot files, the search -------------------------
        let picker_open = self.picker;
        let left_tab = |id: &'static str, label: &str, to: Left, cx: &mut Context<Self>| {
            tab(id, label, PK, self.left == to && !picker_open)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.left = to;
                    this.picker = false;
                    cx.notify();
                }))
                .into_any_element()
        };
        let left_tabs: Vec<AnyElement> = vec![
            tab("ll-codebase", "Codebase", PE, picker_open)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picker = !this.picker;
                    cx.notify();
                }))
                .child(caret(PK))
                .into_any_element(),
            left_tab("ll-map", "Map", Left::Map, cx),
            left_tab("ll-hot", "Hot files", Left::Hot, cx),
            left_tab("ll-search", "Search", Left::Search, cx),
        ];

        let file_row = |k: usize, f: &FileNode, on: bool, cx: &mut Context<Self>| {
            let (name, dir) = match f.path.rsplit_once('/') {
                Some((d, n)) => (n.to_string(), d.to_string()),
                None => (f.path.clone(), String::new()),
            };
            let path = f.path.clone();
            let session = f.claimed_by.clone();
            row(
                format!("ll-file-{k}"),
                &format!("{:02}", k + 1),
                if session.is_some() { PE } else { PK },
                &name,
                &dir,
                session.as_deref().unwrap_or(""),
                MUTED,
                on,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.pick(Some(path.as_str()));
                cx.notify();
            }))
        };

        let mut left = scroll_box("ll-left").pr(px(8.));
        let left_title;
        if picker_open {
            left_title = "Codebase";
            left = left.child(sec("Pick a codebase").mb(px(12.)));
            for (k, c) in war.iter().flat_map(|w| w.codebases.iter()).enumerate() {
                let id = c.id.clone();
                left = left.child(
                    row(
                        format!("ll-cb-{k}"),
                        &format!("{:02}", k + 1),
                        PK,
                        &c.id,
                        &format!("{} · {}", c.branch, c.path),
                        "",
                        MUTED,
                        c.id == codebase,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.show(&id, "", None, cx);
                    })),
                );
            }
        } else {
            match self.left {
                Left::Map => match &self.part {
                    None => {
                        left_title = "Map";
                        let parts = detail.as_ref().map(|d| d.parts.clone()).unwrap_or_default();
                        let widest = parts.iter().map(|p| p.lines).max().unwrap_or(1).max(1) as f32;
                        left = left.child(
                            sec(format!(
                                "{codebase} · {} parts · click one to open",
                                parts.len()
                            ))
                            .mb(px(12.)),
                        );
                        for (k, p) in parts.iter().enumerate() {
                            let name = p.name.clone();
                            let (word, c) = if p.claims > 0 {
                                ("Hot", PE)
                            } else if p.last_change.is_some_and(|t| now - t < RECENT_MS) {
                                ("Recent", PK)
                            } else {
                                ("Stable", MUTED)
                            };
                            left = left.child(
                                div()
                                    .flex_none()
                                    .child(
                                        row(
                                            format!("ll-part-{k}"),
                                            &format!("{:02}", k + 1),
                                            if p.claims > 0 { PE } else { PK },
                                            &p.name,
                                            &format!(
                                                "{} lines · {} live",
                                                p.lines,
                                                p.sessions.len()
                                            ),
                                            word,
                                            c,
                                            false,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.part = Some(name.clone());
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(div().pb(px(6.)).child(meter(
                                        p.lines as f32 / widest * grow,
                                        440.,
                                        PK,
                                    ))),
                            );
                        }
                        if parts.is_empty() {
                            left = left.child(th("No parts in this codebase"));
                        }
                    }
                    Some(name) => {
                        left_title = "Map";
                        let globs = detail
                            .as_ref()
                            .and_then(|d| d.parts.iter().find(|p| &p.name == name))
                            .map(|p| p.paths.clone())
                            .unwrap_or_default();
                        let mut list: Vec<&FileNode> = nodes
                            .iter()
                            .copied()
                            .filter(|f| in_part(&f.path, &globs))
                            .collect();
                        list.sort_by_key(|f| f.claimed_by.is_none());
                        left = left.child(
                            div()
                                .id("ll-all-parts")
                                .flex_none()
                                .pb(px(12.))
                                .cursor_pointer()
                                .child(sec(format!("‹ All parts · {name} · {} files", list.len())))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.part = None;
                                    cx.notify();
                                })),
                        );
                        for (k, f) in list.iter().take(80).enumerate() {
                            let on = self.path.as_deref() == Some(f.path.as_str());
                            left = left.child(file_row(k, f, on, cx));
                        }
                    }
                },
                Left::Hot => {
                    left_title = "Hot files";
                    left = left.child(
                        sec(format!("{} files that a live session claims", hot.len())).mb(px(12.)),
                    );
                    for (k, f) in hot.iter().take(80).enumerate() {
                        let on = self.path.as_deref() == Some(f.path.as_str());
                        left = left.child(file_row(k, f, on, cx));
                    }
                    if hot.is_empty() {
                        left = left.child(prose_note("No file is hot now"));
                    }
                }
                Left::Search => {
                    left_title = "Search";
                    left = left.child(
                        div().flex_none().h(px(36.)).mb(px(10.)).flex().child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .border_1()
                                .border_color(ink(PK, 0.4))
                                .bg(ink(PK, 0.04))
                                .px(px(16.))
                                .flex()
                                .items_center()
                                .font_family(BODY)
                                .text_size(px(S_BODY))
                                .text_color(rgb(TXT))
                                .child(div().flex_1().min_w(px(0.)).child(self.input.clone())),
                        ),
                    );
                    match self.found.as_ref().and_then(|(_, p, _)| p.as_ref()) {
                        Some(page) => {
                            left = left.child(
                                sec(format!("{} found · Enter searches again", page.total))
                                    .mb(px(12.)),
                            );
                            for (k, s) in page.items.iter().enumerate() {
                                let path = s.path.clone();
                                left = left.child(
                                    row(
                                        format!("ll-sym-{k}"),
                                        &format!("{:02}", k + 1),
                                        PK,
                                        &s.name,
                                        &format!("{}:{}", s.path, s.line),
                                        &format!("{:?}", s.kind),
                                        MUTED,
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.pick(Some(path.as_str()));
                                            cx.notify();
                                        },
                                    )),
                                );
                            }
                        }
                        None => {
                            left = left.child(th("Type a name and press Enter"));
                        }
                    }
                }
            }
        }

        // ---- right: the deep dive -----------------------------------------------------------
        let outline: Vec<OutlineRow> = file
            .as_ref()
            .map(|f| flatten_outline(&f.outline))
            .unwrap_or_default();
        let funcs: Vec<&OutlineRow> = outline.iter().filter(|o| o.depth <= 1).collect();
        let chosen: Option<&OutlineRow> = self
            .func
            .and_then(|l| funcs.iter().copied().find(|o| o.line == l))
            .or(funcs.first().copied());
        if self.deep == Deep::Callers
            && let Some(o) = chosen
        {
            let (line, name) = (o.line, bare_name(&o.name).to_string());
            self.ensure_refs(&codebase, line, &name, cx);
        }

        let mut right = div().size_full().flex().flex_col().gap(px(16.));
        match (&self.path, &file) {
            (None, _) => {
                right = right
                    .child(big("Pick a file"))
                    .child(prose_note(
                        "Open a part on the map, a hot file, or a search hit. The deep dive shows its functions and callers here.",
                    ));
            }
            (Some(path), None) => {
                right = right
                    .child(
                        div()
                            .truncate()
                            .font_family(CODE)
                            .text_size(px(S_CODE))
                            .text_color(rgb(TXT))
                            .child(path.clone()),
                    )
                    .child(th(
                        if self.file.as_ref().is_some_and(|(_, f, _)| f.is_none()) {
                            "This file does not open"
                        } else {
                            "Reading the file"
                        },
                    ));
            }
            (Some(path), Some(f)) => {
                right = right.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .truncate()
                                .font_family(CODE)
                                .text_size(px(S_CODE))
                                .text_color(rgb(TXT))
                                .child(path.clone()),
                        )
                        .child(th(format!(
                            "{} lines · {}",
                            f.total_lines,
                            if f.language.is_empty() {
                                "text"
                            } else {
                                f.language.as_str()
                            }
                        ))),
                );
                match self.deep {
                    Deep::Functions => {
                        right = right.child(sec(format!(
                            "Functions · {} of {}",
                            funcs.len().min(12),
                            outline.len()
                        )));
                        let widest = funcs
                            .iter()
                            .map(|o| o.end.saturating_sub(o.start) + 1)
                            .max()
                            .unwrap_or(1)
                            .max(1) as f32;
                        let mut list = scroll_box("ll-funcs").pr(px(8.));
                        for (k, o) in funcs.iter().take(40).enumerate() {
                            let n = o.end.saturating_sub(o.start) + 1;
                            let line = o.line;
                            list = list.child(
                                div()
                                    .id(SharedString::from(format!("ll-fn-{k}")))
                                    .flex_none()
                                    .py(px(10.))
                                    .border_t_1()
                                    .border_color(ink(PK, 0.12))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(ink(PK, 0.07)))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .flex()
                                            .justify_between()
                                            .gap(px(12.))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.))
                                                    .truncate()
                                                    .font_family(CODE)
                                                    .text_size(px(S_CODE))
                                                    .text_color(rgb(TXT))
                                                    .child(o.name.clone()),
                                            )
                                            .child(th(format!("{n} lines"))),
                                    )
                                    .child(meter(n as f32 / widest * grow, 300., PE))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.func = Some(line);
                                        this.deep = Deep::Callers;
                                        this.refs = None;
                                        cx.notify();
                                    })),
                            );
                        }
                        if funcs.is_empty() {
                            list = list.child(th("No functions found"));
                        }
                        right = right.child(list);
                    }
                    Deep::Callers => {
                        let name = chosen.map_or(String::new(), |o| o.name.clone());
                        let page = self.refs.as_ref().and_then(|(_, p, _)| p.as_ref());
                        right = right.child(sec(match (&self.refs, page) {
                            (_, Some(p)) => format!("{name} · used in {} places", p.total),
                            (Some(_), None) => format!("{name} · no answer"),
                            (None, None) => format!("{name} · looking"),
                        }));
                        let mut list = scroll_box("ll-refs").pr(px(8.));
                        for p in page.map(|p| p.items.as_slice()).unwrap_or(&[]) {
                            list = list.child(
                                div()
                                    .flex_none()
                                    .py(px(10.))
                                    .border_t_1()
                                    .border_color(ink(PK, 0.12))
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .truncate()
                                            .font_family(CODE)
                                            .text_size(px(S_CODE))
                                            .text_color(rgb(TXT))
                                            .child(format!("{}:{}", p.path, p.line)),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .font_family(CODE)
                                            .text_size(px(S_CODE))
                                            .text_color(rgb(MUTED))
                                            .child(p.preview.clone()),
                                    ),
                            );
                        }
                        right = right.child(list);
                    }
                }
            }
        }

        let deep_tab = |id: &'static str, label: &str, to: Deep, cx: &mut Context<Self>| {
            tab(id, label, PE, self.deep == to)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.deep = to;
                    cx.notify();
                }))
                .into_any_element()
        };
        let session = self.from_session.clone().or_else(|| {
            let p = self.path.as_ref()?;
            nodes
                .iter()
                .find(|f| &f.path == p)
                .and_then(|f| f.claimed_by.clone())
        });
        let right_tabs = vec![
            deep_tab("ll-functions", "Functions", Deep::Functions, cx),
            deep_tab("ll-callers", "Callers", Deep::Callers, cx),
            tab("ll-session", "Open session ›", PE, false)
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(id) = &session {
                        cx.emit(HoloLensEvent::OpenSession(id.clone()));
                    }
                }))
                .into_any_element(),
            tab("ll-back", "‹ Back", PK, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(HoloLensEvent::Back)))
                .into_any_element(),
        ];

        island(
            art,
            Island {
                stats,
                left_title,
                left_color: PK,
                left_tabs,
                left: left.into_any_element(),
                right_title: "Deep dive".into(),
                right_tabs,
                right: right.into_any_element(),
            },
        )
    }
}

/// A calm line of body text in the muted tone.
fn prose_note(text: &str) -> Div {
    div()
        .font_family(BODY)
        .text_size(px(S_BODY))
        .line_height(px(LH_BODY))
        .text_color(rgb(MUTED))
        .child(text.to_string())
}
