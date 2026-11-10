//! The Holo Focus screen: one session with its code. A title with readouts on top, then two
//! glass panels. The left panel is what you act on: the session menu, the chat, the open ask
//! and the field. The right panel holds the details: the diff, the files, the tests. Every
//! control goes through `act`. It follows the structure of the Bridge Focus screen.

use std::time::Instant;

use crate::ui;
use aivory_protocol::Pane;
use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::session::{DiffFile, DiffLineKind, SessionDiff};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, ScrollHandle, SharedString, Subscription,
    Window, div, px, rgb,
};
use gpui_base::input::{InputEvent, InputState};

use crate::actions::{Action, act};
use crate::compose::{self, Tools};
use crate::compose_controls::{ControlStyle, Controls};
use crate::console::display;
use crate::holo_kit::*;
use crate::media::{self, MediaStyle};
use crate::model::Model;
use crate::session_console::{ConsoleEvent, ConsoleLook, SessionConsole};
use crate::theme::PaneTheme;
use crate::warroom::bridge_data::{provider_name, test_lines, tests_of, turn_of, wait_text};

/// How old a diff gets before the view asks again.
const TTL: std::time::Duration = std::time::Duration::from_secs(8);
/// How long the charts take to grow in.
const GROW_MS: f32 = 700.;
/// The chat shows this many items at most.
const CHAT_ITEMS: usize = 120;
/// The diff shows this many lines at most.
const DIFF_LINES: usize = 120;

#[derive(Debug, Clone)]
pub enum HoloFocusEvent {
    /// Show another session.
    Open(String),
    /// Open a file of the session in the code lens.
    OpenFile { session: String, path: String },
}

impl EventEmitter<HoloFocusEvent> for HoloFocus {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Diff,
    Files,
    Tests,
    Console,
}

pub struct HoloFocus {
    model: Entity<Model>,
    id: String,
    input: Entity<InputState>,
    controls: Controls,
    tools: Tools,
    console: Entity<SessionConsole>,
    diff: Option<(Option<SessionDiff>, Instant)>,
    busy: bool,
    side: Side,
    /// Narrow window: the chat tab is on. Else the details tabs show.
    chat_tab: bool,
    /// The path of the file that the diff shows. The first file when `None`.
    file: Option<String>,
    scroll: ScrollHandle,
    /// The session and item count that the chat was last scrolled to the end for.
    scrolled: String,
    opened: Instant,
    _subs: Vec<Subscription>,
}

impl HoloFocus {
    pub fn new(
        model: Entity<Model>,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Tell the agent what to do"));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.send(window, cx);
            }
        });
        crate::complete::attach(
            &input,
            model.clone(),
            {
                let weak = cx.entity().downgrade();
                move |cx| weak.upgrade().map(|v| v.read(cx).id.clone())
            },
            |_| (None, None),
            cx,
        );
        let console = cx.new(|cx| SessionConsole::new(model.clone(), id.clone(), cx));
        console.update(cx, |c, _| c.set_look(console_look()));
        let opened = cx.subscribe(&console, |_, _, e: &ConsoleEvent, cx| {
            let ConsoleEvent::OpenFile { session, path } = e;
            cx.emit(HoloFocusEvent::OpenFile {
                session: session.clone(),
                path: path.clone(),
            });
        });
        let _ = crate::session_console::take_want_code(&id);
        Self {
            model,
            id,
            input,
            console,
            controls: Controls::default(),
            tools: Tools::default(),
            diff: None,
            busy: false,
            side: Side::Diff,
            chat_tab: true,
            file: None,
            scroll: ScrollHandle::new(),
            scrolled: String::new(),
            opened: Instant::now(),
            _subs: vec![observe, events, opened],
        }
    }

    #[allow(dead_code)]
    pub fn session(&self) -> &str {
        &self.id
    }

    /// Show another session: the data resets and the charts grow in again.
    pub fn show(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.id != id {
            self.id = id.to_string();
            self.console.update(cx, |c, cx| c.show(id, cx));
            if crate::session_console::take_want_code(id) {
                self.side = Side::Diff;
            }
            self.diff = None;
            self.file = None;
            self.controls = Controls::default();
            self.tools = Tools::default();
            self.scrolled.clear();
            self.opened = Instant::now();
            cx.notify();
        }
    }

    fn pane(&self, cx: &gpui::App) -> Option<Pane> {
        self.model
            .read(cx)
            .war
            .as_ref()
            .and_then(|w| w.session(&self.id))
            .cloned()
    }

    /// Send the field: an answer to the open ask, else a message to the agent.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.input.read(cx).value().trim().to_string();
        let Some(pane) = self.pane(cx) else {
            return;
        };
        let images = self.tools.images();
        let text = self.tools.message(&typed, None);
        if text.trim().is_empty() && images.is_empty() {
            return;
        }
        let action = match &pane.ask {
            Some(ask) => Action::Answer {
                id: pane.id.clone(),
                ask: ask.id.clone(),
                option: None,
                text: Some(text),
            },
            _ => {
                let prefs = self.controls.current(&pane);
                crate::compose_controls::submit(&self.model, &pane, text, images, prefs, cx);
                self.tools.sent();
                self.input.update(cx, |s, cx| s.clean(window, cx));
                return;
            }
        };
        act(&self.model, action, cx);
        self.tools.sent();
        self.input.update(cx, |s, cx| s.clean(window, cx));
    }

    /// Read the diff again when the last read is old.
    fn ensure(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.diff.as_ref().is_some_and(|(_, t)| t.elapsed() <= TTL) {
            return;
        }
        self.busy = true;
        let id = self.id.clone();
        let task = self
            .model
            .read(cx)
            .call::<method::SessionDiffMethod>(SessionRef { id: id.clone() }, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if this.id == id {
                    this.diff = Some((result.ok(), Instant::now()));
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// The console in the colors of the glass: no fill, light cyan text.
fn console_look() -> ConsoleLook {
    let theme = PaneTheme {
        bg: 0x00000000,
        top: 0x00000000,
        text: (TXT << 8) | 0xFF,
        dim: (MUTED << 8) | 0xFF,
        accent: (PK << 8) | 0xFF,
        ..PaneTheme::NAVY
    };
    ConsoleLook {
        theme: Some(theme),
        size: 13.,
        pad_x: 4.,
        max_w: None,
    }
}

/// A scroll box that fills the rest of its parent.
fn scroll_box(id: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
}

impl Render for HoloFocus {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let art = Art::of(window);
        let vw = f32::from(window.viewport_size().width);
        let vh = f32::from(window.viewport_size().height);
        let narrow = vw < 1100.;
        let Some(p) = self.pane(cx) else {
            return island(
                art,
                Island {
                    stats: Vec::new(),
                    left_title: "Session",
                    left_color: PK,
                    left_tabs: Vec::new(),
                    left: big("This session is gone").into_any_element(),
                    right_title: "Changes".into(),
                    right_tabs: Vec::new(),
                    right: div().into_any_element(),
                },
            );
        };
        self.ensure(cx);
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let mins = p.minutes(now);
        let waits = p.ask.is_some();
        let grow = ease(self.opened.elapsed(), GROW_MS);
        if grow < 1. {
            window.request_animation_frame();
        }

        // ---- stats -------------------------------------------------------------------------
        let state_word = match p.state {
            aivory_protocol::SessionState::Error => "Error",
            aivory_protocol::SessionState::Working => "Working",
            aivory_protocol::SessionState::Idle => "Idle",
            aivory_protocol::SessionState::NeedsYou => "Waits",
            aivory_protocol::SessionState::Parked => "Parked",
            aivory_protocol::SessionState::Done => "Done",
        };
        let stats = vec![
            ("State".to_string(), state_word.to_string(), state_color(&p)),
            ("Agent".to_string(), provider_name(&p.provider), TXT),
            ("Context".to_string(), format!("{}%", p.context_pct), TXT),
            (
                if waits { "Waits" } else { "In state" }.to_string(),
                wait_text(mins),
                if waits { OR } else { TXT },
            ),
        ];

        // ---- left: the session column and the chat ------------------------------------------
        let mut left_tabs: Vec<AnyElement> = Vec::new();
        let sessions: Vec<(String, String, u32, String)> = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map(|w| {
                order(w)
                    .iter()
                    .map(|s| {
                        let name = if s.title.is_empty() { &s.id } else { &s.title };
                        // A tab shows at most 24 characters; a long title (a whole prompt)
                        // would run out of the panel. The full title shows on hover.
                        let short = if name.chars().count() > 24 {
                            let cut: String = name.chars().take(23).collect();
                            format!("{}…", cut.trim_end())
                        } else {
                            name.clone()
                        };
                        (s.id.clone(), short, state_color(s), name.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Full names only: as many tabs as the menu holds, the open session always among
        // them, and a "+N" tab for the rest.
        let menu_w = 0.46 * (vw - 2. * MARGIN - GAP) - 2. * PAD;
        let width_of = |name: &str| name.chars().count() as f32 * 8.5 + 28.;
        let mut shown: Vec<usize> = Vec::new();
        let mut used = 0.;
        for (k, (_, name, _, _)) in sessions.iter().enumerate() {
            let w = width_of(name);
            if used + w + width_of("+99") > menu_w && !shown.is_empty() {
                break;
            }
            used += w;
            shown.push(k);
        }
        let open_at = sessions.iter().position(|(sid, _, _, _)| *sid == self.id);
        if let Some(at) = open_at
            && !shown.contains(&at)
            && let Some(last) = shown.last_mut()
        {
            *last = at;
        }
        for &k in &shown {
            let (sid, name, c, full) = &sessions[k];
            let sid = sid.clone();
            left_tabs.push(
                tab(format!("lf-s-{k}"), name, *c, *sid == self.id)
                    .tooltip(ui::tip(full.clone()))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(HoloFocusEvent::Open(sid.clone()));
                    }))
                    .into_any_element(),
            );
        }
        if let Some(next) = (0..sessions.len()).find(|k| !shown.contains(k)) {
            let more = sessions.len() - shown.len();
            let sid = sessions[next].0.clone();
            left_tabs.push(
                tab("lf-s-more", &format!("+{more}"), PK, false)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(HoloFocusEvent::Open(sid.clone()));
                    }))
                    .into_any_element(),
            );
        }

        let lines = p.tail.clone();
        let shown = display::last(&lines, CHAT_ITEMS);
        let key = format!("{}:{}", p.id, shown.len());
        if self.scrolled != key {
            self.scroll.scroll_to_bottom();
            if self.opened.elapsed() > std::time::Duration::from_millis(1200) {
                self.scrolled = key;
            }
            window.request_animation_frame();
        }
        let mut chat = scroll_box("lf-chat")
            .min_h(px(96.))
            .track_scroll(&self.scroll)
            .gap(px(2.))
            .pr(px(16.))
            .pb(px(18.));
        if shown.is_empty() {
            chat = chat.child(th("No rows yet"));
        }
        let media_style = MediaStyle {
            card: ink(PK, 0.08).into(),
            text: gpui::rgb(TXT),
            muted: gpui::rgb(MUTED),
            radius: 8.,
        };
        for (n, d) in shown.iter().enumerate() {
            if d.kind == display::DKind::Meta {
                let found = lines
                    .iter()
                    .rev()
                    .find(|l| l.media.is_some() && l.text.trim_end() == d.text);
                if let Some(m) = found.and_then(|l| l.media.as_ref()) {
                    chat = chat.child(
                        div()
                            .flex_none()
                            .pl(px(84.))
                            .py(px(4.))
                            .child(media::preview(m, ("lf-media", n), media_style)),
                    );
                }
                continue;
            }
            chat = chat.children(display_rows(std::slice::from_ref(d)));
        }

        let mut meta = provider_name(&p.provider);
        if !p.model.is_empty() {
            meta.push_str(&format!(" · {}", p.model));
        }
        meta.push_str(&format!(" · {}", p.codebase));
        if let Some(t) = turn_of(&p) {
            meta.push_str(&format!(" · turn {t:02}"));
        }

        let ask_card = p.ask.as_ref().map(|ask| {
            let mut buttons = div().flex().flex_wrap().gap(px(8.));
            if p.controlled {
                let n = ask.options.len();
                for (k, option) in ask.options.iter().enumerate() {
                    let (sid, aid, opt) = (p.id.clone(), ask.id.clone(), option.clone());
                    buttons = buttons.child(
                        pill(
                            format!("lf-answer-{k}"),
                            option_color(k, n),
                            &option_text(k, option),
                        )
                        .px(px(18.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            act(
                                &this.model,
                                Action::Answer {
                                    id: sid.clone(),
                                    ask: aid.clone(),
                                    option: Some(opt.clone()),
                                    text: None,
                                },
                                cx,
                            );
                        })),
                    );
                }
            } else {
                buttons = buttons.child(th("Answer it in its terminal"));
            }
            div()
                .id("lf-ask")
                .flex_none()
                .max_h(px((vh * 0.22).clamp(96., 300.)))
                .overflow_y_scroll()
                .border_1()
                .border_color(ink(OR, 0.45))
                .bg(ink(OR, 0.08))
                .p(px(20.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .font_family(CODE)
                        .text_size(ui::ts(S_BODY))
                        .line_height(ui::ts(24.))
                        .text_color(rgb(WARM))
                        .child(ask.text.clone()),
                )
                .child(buttons)
        });

        let palette = compose::Palette {
            text: TXT << 8 | 0xFF,
            dim: MUTED << 8 | 0xFF,
            line: PK << 8 | 0x66,
            chip_bg: PK << 8 | 0x14,
            hover: PK << 8 | 0x33,
            fail: RD << 8 | 0xFF,
            ok: PK << 8 | 0xFF,
        };
        let chips = compose::chips(&self.tools, "lf", palette, |i| {
            Box::new(cx.listener(move |this, _, _, cx| {
                this.tools.remove(i);
                cx.notify();
            }))
        });
        let field = div()
            .flex_none()
            .min_h(px(40.))
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .child(
                div().flex_1().min_w(px(160.)).flex().child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .border_1()
                        .border_color(ink(PK, 0.4))
                        .bg(ink(PK, 0.04))
                        .px(px(16.))
                        .py(px(6.))
                        .flex()
                        .items_center()
                        .font_family(BODY)
                        .text_size(ui::ts(S_BODY))
                        .text_color(rgb(TXT))
                        .child(div().flex_1().min_w(px(0.)).child(self.input.clone())),
                ),
            )
            .child(
                pill("lf-attach", PU, "Attach")
                    .min_h(px(40.))
                    .min_w(ui::sw(88.))
                    .on_click(cx.listener(|_, _, window, cx| {
                        compose::attach(|this: &mut Self| Some(&mut this.tools), window, cx)
                    })),
            )
            .child(
                pill("lf-send", PK, "Send")
                    .min_h(px(40.))
                    .min_w(ui::sw(88.))
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            );

        self.controls.sync(&p);
        let providers = self.model.read(cx).providers.clone();
        // See-through glass: faint cyan fills, light text, a dark glass menu.
        let style = ControlStyle {
            pill: gpui::rgba(0x4FD8FF14),
            pill_hover: gpui::rgba(0x4FD8FF2E),
            pill_text: rgb(TXT),
            on: gpui::rgba(0x4FD8FF4D),
            on_text: rgb(PA),
            menu: gpui::rgba(0x03172AE6),
            menu_text: rgb(TXT),
            menu_hover: gpui::rgba(0x4FD8FF33),
            font: Some(BODY.into()),
            radius: None,
            text: None,
        };
        let composer = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(self.controls.row(
                "lf-ctl",
                &p,
                &providers,
                &style,
                |this: &mut Self| &mut this.controls,
                cx,
            ))
            .children(chips)
            .child(field);

        let left = div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .truncate()
                            .font_family(BODY)
                            .text_size(ui::ts(S_BIG))
                            .line_height(ui::ts(36.))
                            .text_color(rgb(TXT))
                            .child(p.title.clone()),
                    )
                    .child(th(meta)),
            )
            .child(ui::with_thumb(
                chat,
                &self.scroll,
                gpui::rgba(0x4FD8FF8C),
                4.,
            ))
            .children(ask_card)
            .child(composer);
        let left = compose::media_target(left, |this: &mut Self| Some(&mut this.tools), cx);

        // ---- right: the details -------------------------------------------------------------
        let files: Vec<DiffFile> = self
            .diff
            .as_ref()
            .and_then(|(d, _)| d.as_ref())
            .map(|d| d.files.clone())
            .unwrap_or_default();
        let (add, del) = files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.added, d + f.removed));
        let current: Option<&DiffFile> = self
            .file
            .as_ref()
            .and_then(|path| files.iter().find(|f| &f.path == path))
            .or(files.first());

        let mut right = div().size_full().flex().flex_col().gap(px(16.));
        match self.side {
            Side::Diff => {
                right = right.child(match current {
                    Some(f) => div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .truncate()
                                .font_family(CODE)
                                .text_size(ui::ts(S_CODE))
                                .text_color(rgb(TXT))
                                .child(f.path.clone()),
                        )
                        .child(th(format!(
                            "+{} −{} · file {} of {} · the Files tab picks another",
                            f.added,
                            f.removed,
                            files
                                .iter()
                                .position(|x| x.path == f.path)
                                .map_or(1, |i| i + 1),
                            files.len()
                        ))),
                    None => div().child(th("No change yet")),
                });
                if let Some(f) = current {
                    let mut lines_box = scroll_box("lf-diff").pr(px(8.));
                    for l in f.lines.iter().take(DIFF_LINES) {
                        let (mark, c) = match l.kind {
                            DiffLineKind::Added => ('+', PK),
                            DiffLineKind::Removed => ('−', PE),
                            DiffLineKind::Hunk => ('@', MUTED),
                            DiffLineKind::Context => (' ', MUTED),
                        };
                        let text = if l.kind == DiffLineKind::Hunk {
                            l.text.clone()
                        } else {
                            format!("{mark} {}", l.text.trim_end())
                        };
                        lines_box = lines_box.child(
                            div()
                                .flex_none()
                                .truncate()
                                .font_family(CODE)
                                .text_size(ui::ts(S_CODE))
                                .line_height(ui::ts(24.))
                                .text_color(rgb(c))
                                .child(text),
                        );
                    }
                    right = right.child(lines_box);
                }
            }
            Side::Files => {
                right = right.child(sec(format!(
                    "Files changed · {} · +{add} −{del}",
                    files.len()
                )));
                let widest = files
                    .iter()
                    .map(|f| f.added.max(f.removed))
                    .max()
                    .unwrap_or(1)
                    .max(1) as f32;
                let mut list = scroll_box("lf-files");
                for (k, f) in files.iter().enumerate() {
                    let path = f.path.clone();
                    let (name, dir) = match f.path.rsplit_once('/') {
                        Some((d, n)) => (n.to_string(), d.to_string()),
                        None => (f.path.clone(), String::new()),
                    };
                    let on = current.is_some_and(|c| c.path == f.path);
                    let w_add = f.added as f32 / widest * 160. * grow;
                    let w_del = f.removed as f32 / widest * 160. * grow;
                    list = list.child(
                        div()
                            .flex_none()
                            .child(
                                row(
                                    format!("lf-f-{k}"),
                                    &format!("{:02}", k + 1),
                                    PK,
                                    &name,
                                    &dir,
                                    &format!("+{} −{}", f.added, f.removed),
                                    MUTED,
                                    on,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.file = Some(path.clone());
                                        this.side = Side::Diff;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(4.))
                                    .pb(px(6.))
                                    .child(div().h(px(6.)).w(px(w_add)).bg(rgb(PK)))
                                    .child(div().h(px(6.)).w(px(w_del)).bg(rgb(SA))),
                            ),
                    );
                }
                if files.is_empty() {
                    list = list.child(th("No change yet"));
                }
                right = right.child(list);
            }
            Side::Console => {
                right = right.child(
                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .w_full()
                        .child(self.console.clone()),
                );
            }
            Side::Tests => {
                let counts = tests_of(&lines);
                let label = match counts {
                    Some((ok, bad)) => format!("{ok} of {} pass", ok + bad),
                    None => "No run yet".to_string(),
                };
                right = right.child(sec(label));
                if let Some((ok, bad)) = counts {
                    let total = (ok + bad).max(1) as usize;
                    let on = ((ok as usize * 12 / total) as f32 * grow).round() as usize;
                    let mut blocks = div().flex_none().flex().gap(px(4.)).h(px(18.));
                    for k in 0..12 {
                        blocks = blocks.child(div().flex_1().h_full().bg(rgb(if k < on {
                            PE
                        } else if bad > 0 && grow >= 1. {
                            RD
                        } else {
                            RULE
                        })));
                    }
                    right = right.child(blocks);
                    for l in test_lines(&lines, 6) {
                        right = right.child(
                            div()
                                .flex_none()
                                .truncate()
                                .font_family(CODE)
                                .text_size(ui::ts(S_CODE))
                                .line_height(ui::ts(24.))
                                .text_color(rgb(MUTED))
                                .child(l),
                        );
                    }
                }
            }
        }

        let side_tab = |id: &'static str, label: &str, side: Side, cx: &mut Context<Self>| {
            tab(id, label, PE, self.side == side)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.side = side;
                    cx.notify();
                }))
                .into_any_element()
        };
        let sid = p.id.clone();
        let open_path = current.map(|f| f.path.clone());
        let right_tabs = vec![
            side_tab("lf-diff-tab", "Diff", Side::Diff, cx),
            side_tab("lf-files-tab", "Files", Side::Files, cx),
            side_tab("lf-tests-tab", "Tests", Side::Tests, cx),
            side_tab("lf-console-tab", "Console", Side::Console, cx),
            tab(SharedString::from("lf-lens"), "Open in Lens ›", PK, false)
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(path) = &open_path {
                        cx.emit(HoloFocusEvent::OpenFile {
                            session: sid.clone(),
                            path: path.clone(),
                        });
                    }
                }))
                .into_any_element(),
        ];

        let right_title = format!("Changes · {}", files.len());
        let color = if waits { RD } else { PK };
        if narrow {
            // One panel: the chat and the details are tabs of the same column.
            let on_chat = self.chat_tab;
            let mut tabs: Vec<AnyElement> = vec![
                tab("lf-chat-tab", "Chat", PE, on_chat)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.chat_tab = true;
                        cx.notify();
                    }))
                    .into_any_element(),
            ];
            for t in right_tabs {
                tabs.push(t);
            }
            // A click on a details tab also leaves the chat.
            let tabs = tabs
                .into_iter()
                .enumerate()
                .map(|(k, t)| {
                    if k == 0 {
                        t
                    } else {
                        div()
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _, _, _| this.chat_tab = false),
                            )
                            .child(t)
                            .into_any_element()
                    }
                })
                .collect();
            return island_with(
                art,
                Island {
                    stats,
                    left_title: "Session",
                    left_color: color,
                    left_tabs: tabs,
                    left: if on_chat {
                        left.into_any_element()
                    } else {
                        right.into_any_element()
                    },
                    right_title,
                    right_tabs: Vec::new(),
                    right: div().into_any_element(),
                },
                true,
            );
        }
        island(
            art,
            Island {
                stats,
                left_title: "Session",
                left_color: color,
                left_tabs,
                left: left.into_any_element(),
                right_title,
                right_tabs,
                right: right.into_any_element(),
            },
        )
    }
}
