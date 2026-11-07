//! The Tactical Lens: one codebase seen as a map. The left panel is what you pick (the parts, the
//! hot files, a symbol search); the right panel is the picked file as code, its functions, or the
//! callers of one function. The data comes from `repos/get`, `code/file`, `code/references` and
//! `code/symbols`. Every control calls the daemon through the model.

use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::code::{
    CodeFile, CodeFileParams, CodePosParams, PlacePage, SymbolPage, SymbolsParams,
};
use aivory_protocol::rpc::{CodebaseRef, method};
use aivory_protocol::{FileNode, RepoDetail};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Div, Entity, EventEmitter, Stateful, Subscription, Window, div, px,
};
use gpui_base::input::{InputEvent, InputState};

use crate::lens::logic::{OutlineRow, bare_name, flatten_outline};
use crate::model::Model;
use crate::tactical_kit::{
    self as tac, Frame, c, code, dots, entry, frame, label, navb, section, trow,
};
use crate::tactical_screens::{console, panel};
use crate::ui;

/// How old a read gets before the view asks again.
const TTL: Duration = Duration::from_secs(15);
/// The code view shows this many lines at most.
const CODE_LINES: usize = 400;

#[derive(Debug, Clone)]
pub enum TacLensEvent {
    /// Back to the screen that opened the lens.
    Back,
    /// Open the dossier of a session.
    OpenSession(String),
}

impl EventEmitter<TacLensEvent> for TacticalLens {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Left {
    Map,
    Hot,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deep {
    Code,
    Functions,
    Callers,
}

/// A read: the key it answers, the answer (`None` when it failed), and when it came.
type Read<T> = Option<(String, Option<T>, Instant)>;

pub struct TacticalLens {
    model: Entity<Model>,
    codebase: Option<String>,
    from_session: Option<String>,
    left: Left,
    part: Option<String>,
    picker: bool,
    path: Option<String>,
    deep: Deep,
    func: Option<u32>,
    detail: Read<Rc<RepoDetail>>,
    file: Read<Rc<CodeFile>>,
    refs: Read<PlacePage>,
    found: Read<SymbolPage>,
    busy: [bool; 4],
    input: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl TacticalLens {
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
            deep: Deep::Code,
            func: None,
            detail: None,
            file: None,
            refs: None,
            found: None,
            busy: [false; 4],
            input,
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

    fn pick(&mut self, path: Option<&str>) {
        self.path = path.map(str::to_string);
        self.file = None;
        self.refs = None;
        self.func = None;
        self.deep = Deep::Code;
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
                    cx.notify();
                });
            })
            .detach();
        }
    }

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

/// The part of a glob up to its first wildcard.
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

fn scroll_box(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
}

impl Render for TacticalLens {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let war = self.model.read(cx).war.clone();
        let back = navb("tl-back", "< BACK", false)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(TacLensEvent::Back)))
            .into_any_element();
        let Some(codebase) = self.codebase_id(cx) else {
            return frame(Frame {
                title: "LENS",
                kicker: "NO CODEBASE YET".into(),
                stats: Vec::new(),
                menu: vec![back],
                body: entry("No codebase yet.").into_any_element(),
            });
        };
        self.ensure(&codebase, cx);
        let now = war.as_ref().map_or(0, |w| w.now);
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
            (nodes.len().to_string(), "FILES".to_string(), tac::INK),
            (
                hot.len().to_string(),
                "HOT FILES".to_string(),
                if hot.is_empty() { tac::INK } else { tac::WHITE },
            ),
            (
                last.map_or("NONE".to_string(), |t| ago(now, t).to_uppercase()),
                "LAST CHANGE".to_string(),
                tac::INK,
            ),
        ];

        // ---- the menu ----------------------------------------------------------------------
        let picker_open = self.picker;
        let mut menu: Vec<AnyElement> = vec![
            navb(
                "tl-codebase",
                &format!("{} v", dots(&codebase.to_uppercase(), 24)),
                picker_open,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.picker = !this.picker;
                cx.notify();
            }))
            .into_any_element(),
        ];
        for (id, name, to) in [
            ("tl-map", "MAP", Left::Map),
            ("tl-hot", "HOT FILES", Left::Hot),
            ("tl-search", "SEARCH", Left::Search),
        ] {
            menu.push(
                navb(id, name, self.left == to && !picker_open)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.left = to;
                        this.picker = false;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        menu.push(div().flex_1().into_any_element());
        for (id, name, to) in [
            ("tl-code", "CODE", Deep::Code),
            ("tl-funcs", "FUNCTIONS", Deep::Functions),
            ("tl-callers", "CALLERS", Deep::Callers),
        ] {
            menu.push(
                navb(id, name, self.deep == to)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.deep = to;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let session = self.from_session.clone().or_else(|| {
            let p = self.path.as_ref()?;
            nodes
                .iter()
                .find(|f| &f.path == p)
                .and_then(|f| f.claimed_by.clone())
        });
        menu.push(
            navb("tl-session", "OPEN DOSSIER >", false)
                .border_1()
                .border_color(c(tac::INK, 1.))
                .text_color(c(tac::INK, 1.))
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(id) = &session {
                        cx.emit(TacLensEvent::OpenSession(id.clone()));
                    }
                }))
                .into_any_element(),
        );
        menu.push(back);

        // ---- left: the picker, the parts, the hot files, the search -------------------------
        let file_row = |k: usize, f: &FileNode, on: bool, cx: &mut Context<Self>| {
            let (name, dir) = match f.path.rsplit_once('/') {
                Some((d, n)) => (n.to_string(), d.to_string()),
                None => (f.path.clone(), String::new()),
            };
            let path = f.path.clone();
            trow(
                format!("tl-file-{k}"),
                if f.claimed_by.is_some() {
                    tac::WHITE
                } else {
                    tac::INK
                },
                &name,
                &dir,
                if f.claimed_by.is_some() {
                    "CLAIMED"
                } else {
                    ""
                },
                tac::WHITE,
                on,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.pick(Some(path.as_str()));
                cx.notify();
            }))
        };
        let mut list = scroll_box("tl-left");
        let caption;
        if picker_open {
            caption = "PICK A CODEBASE".to_string();
            for (k, cb) in war.iter().flat_map(|w| w.codebases.iter()).enumerate() {
                let id = cb.id.clone();
                list = list.child(
                    trow(
                        format!("tl-cb-{k}"),
                        tac::INK,
                        &cb.id,
                        &format!("{} · {}", cb.branch, dots(&cb.path, 40)),
                        "",
                        tac::LABEL,
                        cb.id == codebase,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.show(&id, "", None, cx))),
                );
            }
        } else {
            match self.left {
                Left::Map => match &self.part {
                    None => {
                        let parts = detail.as_ref().map(|d| d.parts.clone()).unwrap_or_default();
                        caption = format!("MAP · {} PARTS", parts.len());
                        for (k, p) in parts.iter().enumerate() {
                            let name = p.name.clone();
                            let hot_part = p.claims > 0;
                            list = list.child(
                                trow(
                                    format!("tl-part-{k}"),
                                    if hot_part { tac::WHITE } else { tac::INK },
                                    &p.name,
                                    &format!("{} lines · {} live", p.lines, p.sessions.len()),
                                    if hot_part {
                                        "HOT"
                                    } else if p.last_change.is_some_and(|t| now - t < 86_400_000) {
                                        "RECENT"
                                    } else {
                                        "STABLE"
                                    },
                                    if hot_part { tac::WHITE } else { tac::LABEL },
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.part = Some(name.clone());
                                        cx.notify();
                                    },
                                )),
                            );
                        }
                        if parts.is_empty() {
                            list = list.child(entry("No parts in this codebase."));
                        }
                    }
                    Some(name) => {
                        let globs = detail
                            .as_ref()
                            .and_then(|d| d.parts.iter().find(|p| &p.name == name))
                            .map(|p| p.paths.clone())
                            .unwrap_or_default();
                        let mut files: Vec<&FileNode> = nodes
                            .iter()
                            .copied()
                            .filter(|f| in_part(&f.path, &globs))
                            .collect();
                        files.sort_by_key(|f| f.claimed_by.is_none());
                        caption = format!("{} · {} FILES", name.to_uppercase(), files.len());
                        list = list.child(
                            navb("tl-all-parts", "< ALL PARTS", false)
                                .self_start()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.part = None;
                                    cx.notify();
                                })),
                        );
                        for (k, f) in files.iter().take(100).enumerate() {
                            let on = self.path.as_deref() == Some(f.path.as_str());
                            list = list.child(file_row(k, f, on, cx));
                        }
                    }
                },
                Left::Hot => {
                    caption = format!("HOT FILES · {}", hot.len());
                    for (k, f) in hot.iter().take(100).enumerate() {
                        let on = self.path.as_deref() == Some(f.path.as_str());
                        list = list.child(file_row(k, f, on, cx));
                    }
                    if hot.is_empty() {
                        list = list.child(entry("No file is claimed by a live session now."));
                    }
                }
                Left::Search => {
                    caption = "SEARCH".to_string();
                    list = list.child(
                        div()
                            .flex_none()
                            .h(px(40.))
                            .mb(px(10.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .border_1()
                            .border_color(c(tac::DIM, 1.))
                            .child(div().flex_1().min_w(px(0.)).child(self.input.clone())),
                    );
                    match self.found.as_ref().and_then(|(_, p, _)| p.as_ref()) {
                        Some(page) => {
                            list = list.child(label(format!(
                                "{} FOUND · ENTER SEARCHES AGAIN",
                                page.total
                            )));
                            for (k, s) in page.items.iter().enumerate() {
                                let path = s.path.clone();
                                list = list.child(
                                    trow(
                                        format!("tl-sym-{k}"),
                                        tac::INK,
                                        &s.name,
                                        &format!("{}:{}", s.path, s.line),
                                        &format!("{:?}", s.kind).to_uppercase(),
                                        tac::LABEL,
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
                        None => list = list.child(entry("Type a name and press Enter.")),
                    }
                }
            }
        }
        let left = panel(caption).child(list);

        // ---- right: the file ----------------------------------------------------------------
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

        let mut right = panel("FILE");
        match (&self.path, &file) {
            (None, _) => {
                right = right.child(section("PICK A FILE")).child(entry(
                    "Open a part on the map, a hot file, or a search hit. The file shows here.",
                ));
            }
            (Some(path), None) => {
                right = right.child(code(path.clone())).child(label(
                    if self.file.as_ref().is_some_and(|(_, f, _)| f.is_none()) {
                        "THIS FILE DOES NOT OPEN"
                    } else {
                        "READING THE FILE"
                    },
                ));
            }
            (Some(path), Some(f)) => {
                right = right.child(code(path.clone())).child(label(format!(
                    "{} LINES · {}",
                    f.total_lines,
                    if f.language.is_empty() {
                        "TEXT".to_string()
                    } else {
                        f.language.to_uppercase()
                    }
                )));
                match self.deep {
                    Deep::Code => {
                        let mut body = scroll_box("tl-code-body");
                        for (k, line) in f.text.lines().take(CODE_LINES).enumerate() {
                            body = body.child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .gap(px(12.))
                                    .min_h(px(20.))
                                    .text_size(ui::ts(tac::S_LABEL + 1.))
                                    .line_height(ui::ts(tac::S_LABEL + 1.) * 1.5)
                                    .child(
                                        div()
                                            .flex_none()
                                            .w(px(40.))
                                            .text_color(c(tac::DIM, 1.))
                                            .child(format!("{:>4}", k + 1)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .text_color(c(tac::TEXT, 1.))
                                            .child(line.to_string()),
                                    ),
                            );
                        }
                        right = right.child(body);
                    }
                    Deep::Functions => {
                        right = right.child(label(format!(
                            "FUNCTIONS · {} OF {}",
                            funcs.len().min(60),
                            outline.len()
                        )));
                        let mut body = scroll_box("tl-funcs-body");
                        for (k, o) in funcs.iter().take(60).enumerate() {
                            let n = o.end.saturating_sub(o.start) + 1;
                            let line = o.line;
                            body = body.child(
                                trow(
                                    format!("tl-fn-{k}"),
                                    tac::INK,
                                    &o.name,
                                    &format!("LINE {line}"),
                                    &format!("{n} LINES"),
                                    tac::LABEL,
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.func = Some(line);
                                        this.deep = Deep::Callers;
                                        this.refs = None;
                                        cx.notify();
                                    },
                                )),
                            );
                        }
                        if funcs.is_empty() {
                            body = body.child(entry("No functions found."));
                        }
                        right = right.child(body);
                    }
                    Deep::Callers => {
                        let name = chosen.map_or(String::new(), |o| o.name.clone());
                        let page = self.refs.as_ref().and_then(|(_, p, _)| p.as_ref());
                        right = right.child(label(match (&self.refs, page) {
                            (_, Some(p)) => {
                                format!("{} · USED IN {} PLACES", name.to_uppercase(), p.total)
                            }
                            (Some(_), None) => format!("{} · NO ANSWER", name.to_uppercase()),
                            (None, None) => format!("{} · LOOKING", name.to_uppercase()),
                        }));
                        let mut body = scroll_box("tl-refs-body").gap(px(10.));
                        for p in page.map(|p| p.items.as_slice()).unwrap_or(&[]) {
                            body = body.child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .flex_col()
                                    .child(code(format!("{}:{}", p.path, p.line)))
                                    .child(label(p.preview.clone())),
                            );
                        }
                        right = right.child(body);
                    }
                }
            }
        }

        frame(Frame {
            title: "LENS",
            kicker: format!("{} · A CODEBASE AS A MAP", codebase.to_uppercase()),
            stats,
            menu,
            body: console(left, right),
        })
    }
}
