//! The Operator Lens screen: one codebase as an operator console. A menu (codebase, map, hot
//! files, search), a list (parts, files, search hits) and a detail screen (functions, callers,
//! source of the picked file). The data comes from `repos/get`, `code/file`,
//! `code/references` and `code/symbols`, the same calls as the Bridge Lens.

use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::code::{
    CodeFile, CodeFileParams, CodePosParams, PlacePage, SymbolPage, SymbolsParams,
};
use aivory_protocol::rpc::{CodebaseRef, method};
use aivory_protocol::{FileNode, RepoDetail};
use gpui::prelude::*;
use gpui::{AnyElement, App, Context, Entity, EventEmitter, Subscription, Window, div, px, rgb};
use gpui_base::input::{InputEvent, InputState};

use super::operator_screens::{
    btn, caption, head_line, list_row, menu_col, menu_item, mon, page, prose,
};
use crate::lens::logic::{OutlineRow, bare_name, flatten_outline};
use crate::model::Model;
use crate::ui;
use crate::warroom::skins::operator::kit::*;

/// How old a read gets before the view asks again.
const TTL: Duration = Duration::from_secs(15);
/// A part counts as recent when a file changed in this time.
const RECENT_MS: i64 = 24 * 60 * 60 * 1000;
/// The source view shows this many lines at most.
const SOURCE_LINES: usize = 240;

#[derive(Debug, Clone)]
pub enum OperatorLensEvent {
    /// Back to the screen that opened the lens.
    Back,
    /// Open the session view of a session.
    OpenSession(String),
}

impl EventEmitter<OperatorLensEvent> for OperatorLens {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Left {
    Map,
    Hot,
    Search,
    Codebase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deep {
    Functions,
    Callers,
    Source,
}

/// A read: the key it answers, the answer (`None` when it failed), and when it came.
type Read<T> = Option<(String, Option<T>, Instant)>;

pub struct OperatorLens {
    model: Entity<Model>,
    codebase: Option<String>,
    from_session: Option<String>,
    left: Left,
    part: Option<String>,
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

impl OperatorLens {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("find a function or type"));
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
            path: None,
            deep: Deep::Functions,
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
        if self.left == Left::Codebase {
            self.left = Left::Map;
        }
        self.pick(if path.is_empty() { None } else { Some(path) });
        cx.notify();
    }

    fn pick(&mut self, path: Option<&str>) {
        self.path = path.map(str::to_string);
        self.file = None;
        self.refs = None;
        self.func = None;
        self.deep = Deep::Functions;
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

impl Render for OperatorLens {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let war = self.model.read(cx).war.clone();
        let Some(codebase) = self.codebase_id(cx) else {
            return page(
                head_line("F4", "Lens", Vec::new()),
                vec![
                    mon(
                        "ml-none",
                        true,
                        "Lens",
                        "1",
                        "no codebase",
                        prose("No codebase yet."),
                    )
                    .flex_1()
                    .into_any_element(),
                ],
            );
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

        let mut head = head_line(
            "F4",
            "Lens",
            vec![
                ("codebase".into(), codebase.clone()),
                ("files".into(), nodes.len().to_string()),
                ("hot files".into(), hot.len().to_string()),
                (
                    "last change".into(),
                    last.map_or("none".to_string(), |t| ago(now, t)),
                ),
            ],
        )
        .gap(px(20.));
        head = head.child(
            btn("ml-back".into(), "‹ Back", false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(OperatorLensEvent::Back))),
        );

        // The menu.
        let mut menu = div().flex().flex_col().gap(px(2.));
        for (to, label, count) in [
            (Left::Codebase, "Codebase ▾", None),
            (Left::Map, "Map", detail.as_ref().map(|d| d.parts.len())),
            (Left::Hot, "Hot files", Some(hot.len())),
            (Left::Search, "Search", None),
        ] {
            menu = menu.child(
                menu_item(format!("ml-menu-{label}"), label, count, self.left == to).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.left = to;
                        cx.notify();
                    }),
                ),
            );
        }
        let menu = menu_col(
            "ml-menu",
            mon("ml-menu-s", true, "Menu", "1", "section", menu),
        );

        // The list.
        let file_row = |k: usize, f: &FileNode, on: bool, cx: &mut Context<Self>| {
            let (name, dir) = match f.path.rsplit_once('/') {
                Some((d, n)) => (n.to_string(), d.to_string()),
                None => (f.path.clone(), String::new()),
            };
            let path = f.path.clone();
            list_row(
                format!("ml-file-{k}"),
                k + 1,
                &name,
                &dir,
                if f.claimed_by.is_some() { "hot" } else { "" },
                if f.claimed_by.is_some() {
                    BLUE_HI
                } else {
                    BLUE_SOFT
                },
                on,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.pick(Some(path.as_str()));
                cx.notify();
            }))
        };
        let mut list = div().flex().flex_col();
        let list_title: String;
        match self.left {
            Left::Codebase => {
                list_title = "Pick a codebase".into();
                for (k, c) in war.iter().flat_map(|w| w.codebases.iter()).enumerate() {
                    let id = c.id.clone();
                    list = list.child(
                        list_row(
                            format!("ml-cb-{k}"),
                            k + 1,
                            &c.id,
                            &format!("{} · {}", c.branch, c.path),
                            "",
                            BLUE_SOFT,
                            c.id == codebase,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show(&id, "", None, cx);
                        })),
                    );
                }
            }
            Left::Map => match &self.part {
                None => {
                    let parts = detail.as_ref().map(|d| d.parts.clone()).unwrap_or_default();
                    list_title = format!("Map · {} parts", parts.len());
                    for (k, p) in parts.iter().enumerate() {
                        let name = p.name.clone();
                        let (word, c) = if p.claims > 0 {
                            ("hot", BLUE_HI)
                        } else if p.last_change.is_some_and(|t| now - t < RECENT_MS) {
                            ("recent", BLUE)
                        } else {
                            ("stable", BLUE_SOFT)
                        };
                        list = list.child(
                            list_row(
                                format!("ml-part-{k}"),
                                k + 1,
                                &p.name,
                                &format!("{} lines · {} live", p.lines, p.sessions.len()),
                                word,
                                c,
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
                        list = list.child(prose("No parts in this codebase."));
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
                    list_title = format!("{name} · {} files", files.len());
                    list = list.child(
                        btn("ml-all-parts".into(), "‹ All parts", false)
                            .mb(px(8.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.part = None;
                                cx.notify();
                            })),
                    );
                    for (k, f) in files.iter().take(80).enumerate() {
                        let on = self.path.as_deref() == Some(f.path.as_str());
                        list = list.child(file_row(k, f, on, cx));
                    }
                }
            },
            Left::Hot => {
                list_title = format!("Hot files · {}", hot.len());
                for (k, f) in hot.iter().take(80).enumerate() {
                    let on = self.path.as_deref() == Some(f.path.as_str());
                    list = list.child(file_row(k, f, on, cx));
                }
                if hot.is_empty() {
                    list = list.child(prose("No file is hot now."));
                }
            }
            Left::Search => {
                list_title = "Search".into();
                list = list.child(
                    div()
                        .flex_none()
                        .h(px(40.))
                        .mb(px(10.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .bg(rgb(0x021024))
                        .text_color(rgb(BLUE_HI))
                        .child("find>")
                        .child(div().flex_1().min_w(px(0.)).child(self.input.clone())),
                );
                match self.found.as_ref().and_then(|(_, p, _)| p.as_ref()) {
                    Some(page) => {
                        list = list.child(prose(&format!(
                            "{} found · Enter searches again",
                            page.total
                        )));
                        for (k, s) in page.items.iter().enumerate() {
                            let path = s.path.clone();
                            list = list.child(
                                list_row(
                                    format!("ml-sym-{k}"),
                                    k + 1,
                                    &s.name,
                                    &format!("{}:{}", s.path, s.line),
                                    &format!("{:?}", s.kind).to_lowercase(),
                                    BLUE_SOFT,
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
                        list = list.child(prose("Type a name and press Enter."));
                    }
                }
            }
        }
        let list = mon("ml-list", true, &list_title, "2", "click to open", list)
            .w(px(450.))
            .flex_none()
            .into_any_element();

        // The detail: the deep dive of the picked file.
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
        let mut body = div().flex().flex_col().gap(px(10.));
        match (&self.path, &file) {
            (None, _) => {
                body = body.child(prose(
                    "Open a part on the map, a hot file or a search hit. The functions, the callers and the source of the file show here.",
                ));
            }
            (Some(path), None) => {
                body = body.child(prose(path)).child(prose(
                    if self.file.as_ref().is_some_and(|(_, f, _)| f.is_none()) {
                        "This file does not open."
                    } else {
                        "Reading the file…"
                    },
                ));
            }
            (Some(path), Some(f)) => {
                body = body
                    .child(div().text_color(rgb(BLUE_HI)).child(path.clone()))
                    .child(
                        div()
                            .text_size(ui::ts(T_LABEL))
                            .text_color(rgb(BLUE_SOFT))
                            .child(format!(
                                "{} lines · {}",
                                f.total_lines,
                                if f.language.is_empty() {
                                    "text"
                                } else {
                                    f.language.as_str()
                                }
                            )),
                    );
                let mut tabs = div().flex().gap(px(6.));
                for (to, label) in [
                    (Deep::Functions, "Functions"),
                    (Deep::Callers, "Callers"),
                    (Deep::Source, "Source"),
                ] {
                    tabs = tabs.child(
                        btn(format!("ml-deep-{label}"), label, self.deep == to).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.deep = to;
                                cx.notify();
                            }),
                        ),
                    );
                }
                body = body.child(tabs);
                match self.deep {
                    Deep::Functions => {
                        body = body.child(caption(&format!(
                            "Functions · {} · click one for its callers",
                            funcs.len()
                        )));
                        for (k, o) in funcs.iter().take(60).enumerate() {
                            let n = o.end.saturating_sub(o.start) + 1;
                            let line = o.line;
                            body = body.child(
                                list_row(
                                    format!("ml-fn-{k}"),
                                    k + 1,
                                    &o.name,
                                    &format!("line {}", o.line),
                                    &format!("{n} lines"),
                                    BLUE_SOFT,
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
                            body = body.child(prose("No functions found."));
                        }
                    }
                    Deep::Callers => {
                        let name = chosen.map_or(String::new(), |o| o.name.clone());
                        let page = self.refs.as_ref().and_then(|(_, p, _)| p.as_ref());
                        body = body.child(caption(&match (&self.refs, page) {
                            (_, Some(p)) => format!("{name} · used in {} places", p.total),
                            (Some(_), None) => format!("{name} · no answer"),
                            (None, None) => format!("{name} · looking"),
                        }));
                        for p in page.map(|p| p.items.as_slice()).unwrap_or(&[]) {
                            body = body
                                .child(
                                    div()
                                        .flex_none()
                                        .mt(px(6.))
                                        .text_color(rgb(BLUE_HI))
                                        .child(format!("{}:{}", p.path, p.line)),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(ui::ts(T_LABEL))
                                        .text_color(rgb(BLUE_SOFT))
                                        .child(p.preview.clone()),
                                );
                        }
                    }
                    Deep::Source => {
                        body = body.child(caption("Source"));
                        let mut src = div().flex().flex_col().text_size(ui::ts(T_LABEL));
                        for (i, l) in f.text.lines().take(SOURCE_LINES).enumerate() {
                            src = src.child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .gap(px(12.))
                                    .child(
                                        div()
                                            .flex_none()
                                            .w(px(36.))
                                            .flex()
                                            .justify_end()
                                            .text_color(rgb(BLUE_RING))
                                            .child((i + 1).to_string()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .text_color(rgb(BLUE_HI))
                                            .child(if l.is_empty() {
                                                " ".to_string()
                                            } else {
                                                l.to_string()
                                            }),
                                    ),
                            );
                        }
                        body = body.child(src);
                    }
                }
            }
        }
        let session = self.from_session.clone().or_else(|| {
            let p = self.path.as_ref()?;
            nodes
                .iter()
                .find(|f| &f.path == p)
                .and_then(|f| f.claimed_by.clone())
        });
        if let Some(id) = session {
            body = body.child(div().mt(px(8.)).child(
                btn("ml-session".into(), "Open session ›", false).on_click(cx.listener(
                    move |_, _, _, cx| cx.emit(OperatorLensEvent::OpenSession(id.clone())),
                )),
            ));
        }
        let detail_mon: AnyElement = mon("ml-detail", true, "Deep dive", "3", "file", body)
            .flex_1()
            .min_w(px(0.))
            .into_any_element();
        page(head, vec![menu, list, detail_mon])
    }
}
