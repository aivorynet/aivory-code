//! The Neon Lens screen: one codebase and one file of it. The menu picks what the left
//! list shows (files, hot files, search); the right panel shows the file as code or as its
//! functions. Data comes from `repos/get`, `code/file` and `code/symbols`.

use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::FileNode;
use aivory_protocol::RepoDetail;
use aivory_protocol::code::{CodeFile, CodeFileParams, SymbolPage, SymbolsParams};
use aivory_protocol::rpc::{CodebaseRef, method};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, SharedString, Subscription, Window, div, px,
    rgb,
};
use gpui_base::input::{InputEvent, InputState};

use super::neon_kit::*;
use crate::lens::logic::{bare_name, flatten_outline};
use crate::model::Model;
use crate::theme::tk;
use crate::ui;

const TTL: Duration = Duration::from_secs(15);
/// The code panel shows this many lines at most.
const CODE_LINES: usize = 400;

#[derive(Debug, Clone)]
pub(super) enum NightLensEvent {
    Back,
    OpenSession(String),
}

impl EventEmitter<NightLensEvent> for NightLens {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Left {
    Files,
    Hot,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deep {
    Code,
    Functions,
}

type Read<T> = Option<(String, Option<T>, Instant)>;

pub(super) struct NightLens {
    model: Entity<Model>,
    codebase: Option<String>,
    from_session: Option<String>,
    left: Left,
    deep: Deep,
    path: Option<String>,
    detail: Read<Rc<RepoDetail>>,
    file: Read<Rc<CodeFile>>,
    found: Read<SymbolPage>,
    busy: [bool; 3],
    input: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl NightLens {
    pub(super) fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            left: Left::Files,
            deep: Deep::Code,
            path: None,
            detail: None,
            file: None,
            found: None,
            busy: [false; 3],
            input,
            _subs: vec![observe, events],
        }
    }

    /// Show a codebase, and a file of it when `path` is not empty.
    pub(super) fn show(
        &mut self,
        codebase: &str,
        path: &str,
        session: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.codebase.as_deref() != Some(codebase) {
            self.detail = None;
            self.found = None;
        }
        self.codebase = Some(codebase.to_string());
        self.from_session = session;
        self.pick(if path.is_empty() { None } else { Some(path) });
        cx.notify();
    }

    fn pick(&mut self, path: Option<&str>) {
        self.path = path.map(str::to_string);
        self.file = None;
        self.deep = Deep::Code;
    }

    fn codebase_id(&self, cx: &gpui::App) -> Option<String> {
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

    /// Search the symbols of the codebase for the text in the field.
    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().trim().to_string();
        let Some(codebase) = self.codebase_id(cx) else {
            return;
        };
        if query.is_empty() || self.busy[2] {
            return;
        }
        self.busy[2] = true;
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
                this.busy[2] = false;
                this.found = Some((query, result.ok(), Instant::now()));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn scroll_box(id: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
}

impl Render for NightLens {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(codebase) = self.codebase_id(cx) else {
            return screen(
                "Lens",
                "No codebase yet".to_string(),
                YELLOW,
                (Vec::new(), Vec::new()),
                div().into_any_element(),
            );
        };
        self.ensure(&codebase, cx);
        let nodes: Vec<FileNode> = self
            .detail
            .as_ref()
            .and_then(|(_, d, _)| d.as_ref())
            .map(|d| d.files.iter().filter(|f| !f.dir).cloned().collect())
            .unwrap_or_default();
        let hot: Vec<FileNode> = nodes
            .iter()
            .filter(|f| f.claimed_by.is_some())
            .cloned()
            .collect();

        let mut menu: Vec<AnyElement> = Vec::new();
        for (l, name, n) in [
            (Left::Files, "Files", Some(nodes.len())),
            (Left::Hot, "Hot files", Some(hot.len())),
            (Left::Search, "Search", None),
        ] {
            menu.push(
                menu_tab(format!("nl-tab-{name}"), name, n, YELLOW, self.left == l)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.left = l;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let mut actions = vec![
            button("nl-back", "◂ Back", CYAN, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(NightLensEvent::Back)))
                .into_any_element(),
        ];
        if let Some(s) = self.from_session.clone() {
            actions.push(
                button("nl-session", "Session ›", CYAN, false)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(NightLensEvent::OpenSession(s.clone()));
                    }))
                    .into_any_element(),
            );
        }

        // ---- left: files, hot files or search ------------------------------------------------
        let file_row = |k: usize, f: &FileNode, cx: &mut Context<Self>| {
            let path = f.path.clone();
            let on = self.path.as_deref() == Some(f.path.as_str());
            let dir = f.path.rsplit_once('/').map_or("", |(d, _)| d).to_string();
            row(
                SharedString::from(format!("nl-f-{k}")),
                &format!("{:02}", k + 1),
                if f.claimed_by.is_some() { YELLOW } else { CYAN },
                (
                    &f.name,
                    &dir,
                    if f.claimed_by.is_some() { "HOT" } else { "" },
                ),
                on,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.pick(Some(&path));
                cx.notify();
            }))
            .into_any_element()
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        let (left_title, left_note);
        match self.left {
            Left::Files => {
                left_title = "Files";
                left_note = format!("{} FILES", nodes.len());
                for (k, f) in nodes.iter().take(300).enumerate() {
                    rows.push(file_row(k, f, cx));
                }
            }
            Left::Hot => {
                left_title = "Hot files";
                left_note = "A LIVE SESSION CLAIMS THEM".to_string();
                for (k, f) in hot.iter().enumerate() {
                    rows.push(file_row(k, f, cx));
                }
                if rows.is_empty() {
                    rows.push(
                        div()
                            .px(px(24.))
                            .py(px(16.))
                            .child(label("No file is hot now", MIST))
                            .into_any_element(),
                    );
                }
            }
            Left::Search => {
                left_title = "Search";
                left_note = String::new();
                rows.push(
                    div()
                        .px(px(24.))
                        .pb(px(16.))
                        .child(div().h(px(40.)).child(self.input.clone()))
                        .into_any_element(),
                );
                if let Some((q, Some(page), _)) = &self.found {
                    rows.push(
                        div()
                            .px(px(24.))
                            .pb(px(8.))
                            .child(label(format!("{} MATCHES FOR {q}", page.total), MIST))
                            .into_any_element(),
                    );
                    for (k, s) in page.items.iter().enumerate() {
                        let path = s.path.clone();
                        rows.push(
                            row(
                                SharedString::from(format!("nl-s-{k}")),
                                &format!("{:02}", k + 1),
                                CYAN,
                                (&s.name, &s.path, &format!("L{}", s.line)),
                                false,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pick(Some(&path));
                                cx.notify();
                            }))
                            .into_any_element(),
                        );
                    }
                }
            }
        }
        let left = frame(YELLOW, "nl-left", false)
            .w(px(480.))
            .flex_none()
            .h_full()
            .child(section(left_title, YELLOW, Some(left_note)))
            .child(scroll_box("nl-list").pb(px(16.)).children(rows));

        // ---- right: the file as code or as functions ----------------------------------------
        let mut right = frame(CYAN, "nl-right", self.path.is_some())
            .flex_1()
            .min_w(px(0.))
            .h_full();
        let file = self.file.as_ref().and_then(|(_, f, _)| f.as_ref()).cloned();
        match (&self.path, file) {
            (Some(path), Some(f)) => {
                right = right.child(section(
                    path.rsplit('/').next().unwrap_or(path),
                    CYAN,
                    Some(format!("{} · {} LINES", f.language, f.total_lines)),
                ));
                let mut tabs = div()
                    .flex_none()
                    .px(px(24.))
                    .flex()
                    .border_b_1()
                    .border_color(gpui::rgba(0x8C96BC30));
                for (d, name, id) in [
                    (Deep::Code, "Code", "nl-d-code"),
                    (Deep::Functions, "Functions", "nl-d-fn"),
                ] {
                    tabs = tabs.child(menu_tab(id, name, None, CYAN, self.deep == d).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.deep = d;
                            cx.notify();
                        }),
                    ));
                }
                right = right.child(tabs);
                let mut body = scroll_box("nl-body").px(px(24.)).py(px(16.));
                match self.deep {
                    Deep::Code => {
                        for (i, l) in f.text.lines().take(CODE_LINES).enumerate() {
                            body = body.child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .gap(px(16.))
                                    .font_family(tk().mono)
                                    .text_size(ui::ts(T_LABEL + 1.))
                                    .line_height(ui::ts(20.))
                                    .child(
                                        div()
                                            .w(px(40.))
                                            .flex_none()
                                            .text_color(rgb(DIM))
                                            .child((i + 1).to_string()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .text_color(rgb(PALE))
                                            .child(l.to_string()),
                                    ),
                            );
                        }
                    }
                    Deep::Functions => {
                        let outline = flatten_outline(&f.outline);
                        if outline.is_empty() {
                            body = body.child(label("No functions found in this file", MIST));
                        }
                        for (k, o) in outline.iter().enumerate() {
                            body = body.child(
                                div()
                                    .flex_none()
                                    .h(px(36.))
                                    .pl(px(f32::from(o.depth) * 20.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(16.))
                                    .child(entry(bare_name(&o.name).to_string()))
                                    .child(label(format!("L{}", o.line), MIST))
                                    .id(SharedString::from(format!("nl-o-{k}"))),
                            );
                        }
                    }
                }
                right = right.child(body);
            }
            (Some(_), None) => {
                right = right
                    .child(section("File", CYAN, None))
                    .child(div().px(px(24.)).child(label("Reading the file", MIST)));
            }
            _ => {
                right = right.child(section("File", CYAN, None)).child(
                    div()
                        .px(px(24.))
                        .child(entry("Pick a file on the left to read it.")),
                );
            }
        }
        screen(
            "Lens",
            format!(
                "{codebase} · {}",
                self.path.as_deref().unwrap_or("no file picked")
            ),
            YELLOW,
            (menu, actions),
            div()
                .size_full()
                .flex()
                .gap(px(32.))
                .child(left)
                .child(right)
                .into_any_element(),
        )
    }
}
