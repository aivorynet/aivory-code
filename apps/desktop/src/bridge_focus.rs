//! The Bridge Focus screen (mockup `BridgeFocus`, nav 01·1): one session with its code. Two
//! islands under the frame of the shell. The left island is what you act on: the chat, the
//! open ask and the field. The right island holds the details: the diff, the files, the tests.
//! The button column at the left switches the session; the column at the right switches the
//! detail (diff, files, tests, console). Every control goes through `act`.

use std::time::Instant;

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
use crate::bridge_kit::*;
use crate::bridge_screens::{Island, big, row};
use crate::compose::{self, Tools};
use crate::compose_controls::{ControlStyle, Controls};
use crate::media::{self, MediaStyle};
use crate::model::Model;
use crate::session_console::{ConsoleEvent, ConsoleLook, SessionConsole};
use crate::theme::PaneTheme;
use crate::warroom::bridge_data::{provider_name, test_lines, tests_of, turn_of, wait_text};

/// How old a diff gets before the view asks again.
const TTL: std::time::Duration = std::time::Duration::from_secs(8);
/// How long the charts take to grow in.
const GROW_MS: f32 = 700.;
/// The diff shows this many lines at most.
const DIFF_LINES: usize = 120;

#[derive(Debug, Clone)]
pub enum BridgeFocusEvent {
    /// Show another session.
    Open(String),
    /// Open a file of the session in the code lens.
    OpenFile { session: String, path: String },
}

impl EventEmitter<BridgeFocusEvent> for BridgeFocus {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Diff,
    Files,
    Tests,
    Console,
}

pub struct BridgeFocus {
    model: Entity<Model>,
    id: String,
    input: Entity<InputState>,
    controls: Controls,
    tools: Tools,
    console: Entity<SessionConsole>,
    diff: Option<(Option<SessionDiff>, Instant)>,
    busy: bool,
    side: Side,
    /// Narrow window: the chat shows (else the detail tab).
    show_chat: bool,
    /// The path of the file that the diff shows. The first file when `None`.
    file: Option<String>,
    scroll: ScrollHandle,
    /// The session and item count that the chat was last scrolled to the end for.
    scrolled: String,
    opened: Instant,
    _subs: Vec<Subscription>,
}

impl BridgeFocus {
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
        console.update(cx, |c, _| {
            c.set_look(ConsoleLook {
                theme: Some(PaneTheme::BRIDGE),
                size: S_CODE,
                pad_x: 8.,
                max_w: None,
            })
        });
        let open_file = cx.subscribe(&console, |_, _, e: &ConsoleEvent, cx| {
            let ConsoleEvent::OpenFile { session, path } = e;
            cx.emit(BridgeFocusEvent::OpenFile {
                session: session.clone(),
                path: path.clone(),
            });
        });
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
            show_chat: true,
            file: None,
            scroll: ScrollHandle::new(),
            scrolled: String::new(),
            opened: Instant::now(),
            _subs: vec![observe, events, open_file],
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
            self.diff = None;
            self.file = None;
            self.console.update(cx, |c, cx| c.show(id, cx));
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

/// The lit tab.
const LIT: u32 = 0xFFE0C8;

/// A button of a column of the frame: lit when on, `h` px high.
fn stab(
    id: impl Into<SharedString>,
    label: &str,
    color: u32,
    on: bool,
    h: f32,
) -> gpui::Stateful<Div> {
    ibtn_flow(id, if on { LIT } else { color }, label)
        .w(px(170.))
        .h(px(h))
        .flex_none()
        .pb(px(if h < 30. { 4. } else { 8. }))
}

/// The layer of the session screen: the same frame pieces as the other Bridge islands, but the
/// islands size from the window. Wide: the chat island and the detail island side by side.
/// Narrow (`narrow`): one island; the detail tabs sit in the button column at the right.
fn session_island(art: Art, narrow: bool, i: Island) -> Div {
    let c = i.left_color;
    let w = 1440. + art.dx;
    let up = (-art.dy).clamp(0., 44.);
    let (y0, y1, y2) = (300. - up, 384. - up, 424. - up);
    let col_h = art.by(808.) - y1;
    let body_h = art.by(796.) - y2;
    // The right end of the left island, and the start of the right island.
    let end = if narrow {
        art.rx(1124.) + 36.
    } else {
        (16. + 0.544 * (w - 72.)).max(636.)
    };
    let xs = end + 40.;
    let mut stats = div()
        .absolute()
        .left(px(316.))
        .top(px(88.))
        .flex()
        .gap(crate::ui::sw(if narrow { 28. } else { 64. }));
    for (label, value, color) in i.stats {
        stats = stats.child(
            div().flex().flex_col().gap(px(8.)).child(th(label)).child(
                div()
                    .font_family(LABEL)
                    .text_size(tsz(S_BIG))
                    .line_height(tsz(S_BIG + 4.))
                    .text_color(rgb(color))
                    .whitespace_nowrap()
                    .child(value),
            ),
        );
    }
    let title = |text: String, color: u32| {
        div()
            .flex_none()
            .font_family(LABEL)
            .text_size(tsz(S_BIG))
            .line_height(tsz(36.))
            .text_color(rgb(color))
            .whitespace_nowrap()
            .child(text.to_uppercase())
    };
    let mut out = div()
        .size_full()
        .relative()
        .overflow_hidden()
        .child(stats)
        .child(rect(16., y0, 300., 76., c).rounded_tl(px(66.)))
        .child(rect(186., y0 + 36., 133., 40., BLACK).rounded_tl(px(28.)))
        .child(
            area(
                320.,
                y0,
                (if narrow { art.rx(1124.) } else { end }) - 320.,
                36.,
            )
            .flex()
            .items_center()
            .gap(px(12.))
            .child(div().flex_1().h(px(36.)).bg(rgb(c)))
            .child(title(i.left_title.to_string(), PE)),
        )
        .child(
            area(16., y1, 170., col_h)
                .flex()
                .flex_col()
                .gap(px(3.))
                .overflow_hidden()
                .children(i.left_tabs)
                .child(div().flex_1().bg(rgb(c))),
        )
        .child(rect(16., art.by(812.), 300., 72., SK).rounded_bl(px(66.)))
        .child(rect(186., art.by(812.), 133., 36., BLACK).rounded_bl(px(28.)))
        .child(rect(
            320.,
            art.by(848.),
            if narrow {
                art.rx(1124.) - 320.
            } else {
                end - 360.
            },
            36.,
            SK,
        ))
        .child(
            area(
                222.,
                y2,
                if narrow { art.rx(1208.) } else { end - 34. } - 222.,
                body_h,
            )
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(i.left),
        )
        // The right column and its corners.
        .child(rect(art.rx(1124.), y0, 300., 76., PE).rounded_tr(px(66.)))
        .child(rect(art.rx(1124.) - 3., y0 + 36., 133., 40., BLACK).rounded_tr(px(28.)))
        .child(
            area(art.rx(1254.), y1, 170., col_h)
                .flex()
                .flex_col()
                .gap(px(4.))
                .overflow_hidden()
                .children(i.right_tabs)
                .child(div().flex_1().bg(rgb(SK))),
        )
        .child(rect(art.rx(1124.), art.by(812.), 300., 72., SK).rounded_br(px(66.)))
        .child(rect(art.rx(1124.) - 3., art.by(812.), 133., 36., BLACK).rounded_br(px(28.)));
    if !narrow {
        out = out
            .child(
                area(xs, y0, art.rx(1120.) - xs, 36.)
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex_none()
                            .size(px(36.))
                            .bg(rgb(PE))
                            .rounded_l(px(18.)),
                    )
                    .child(div().px(px(8.)).child(title(i.right_title, PE)))
                    .child(div().flex_1().h(px(36.)).bg(rgb(PE))),
            )
            .child(rect(
                xs + 40.,
                art.by(848.),
                art.rx(1120.) - xs - 40.,
                36.,
                SK,
            ))
            .child(rect(xs, art.by(848.), 36., 36., SK).rounded_l(px(18.)))
            .child(
                area(xs + 56., y2, art.rx(1208.) - xs - 56., body_h)
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(i.right),
            );
    }
    out
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

impl Render for BridgeFocus {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let art = Art::of(window);
        let vp = window.viewport_size();
        let narrow = f32::from(vp.width) < 1290.;
        let k = kf();
        let Some(p) = self.pane(cx) else {
            return session_island(
                art,
                narrow,
                Island {
                    stats: Vec::new(),
                    left_title: "Chat",
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
        if crate::session_console::take_want_code(&self.id) {
            self.side = Side::Diff;
        }
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
                if waits { RD } else { TXT },
            ),
        ];

        // ---- left: the session column and the chat ------------------------------------------
        let mut left_tabs: Vec<AnyElement> = Vec::new();
        let sessions: Vec<(String, String, u32)> = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map(|w| {
                order(w)
                    .iter()
                    .map(|s| {
                        let name = if s.title.is_empty() { &s.id } else { &s.title };
                        (s.id.clone(), name.clone(), state_color(s))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let col_h = art.by(808.) - (384. - (-art.dy).clamp(0., 44.));
        let body_h = art.by(796.) - (424. - (-art.dy).clamp(0., 44.));
        let tab_h = if col_h < 300. { 26. } else { 34. };
        let cap = (((col_h - 20.) / (tab_h + 3.)) as usize).clamp(1, 7);
        let mut shown_ids: Vec<usize> = (0..sessions.len().min(cap)).collect();
        if sessions.len() > cap {
            shown_ids.pop();
            let at = sessions.iter().position(|s| s.0 == self.id);
            if let Some(at) = at.filter(|a| !shown_ids.contains(a)) {
                shown_ids.pop();
                shown_ids.push(at);
            }
        }
        let more = sessions.len() - shown_ids.len();
        for k in shown_ids {
            let (sid, name, c) = &sessions[k];
            let sid = sid.clone();
            left_tabs.push(
                stab(format!("lf-s-{k}"), name, *c, *sid == self.id, tab_h)
                    .tooltip(crate::ui::tip(name.clone()))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(BridgeFocusEvent::Open(sid.clone()));
                    }))
                    .into_any_element(),
            );
        }

        if more > 0 {
            left_tabs.push(
                stab("lf-more", &format!("+{more} more"), MUTED, false, tab_h).into_any_element(),
            );
        }

        let lines = p.tail.clone();
        // The chat column is wider on a large window: the line length follows its width.
        let left_w = if narrow {
            art.rx(1208.) - 222.
        } else {
            (16. + 0.544 * (1440. + art.dx - 72.)).max(636.) - 256.
        };
        let chars = (((left_w - 116. * k - 16.) / (6.4 * k)) as usize).clamp(24, 160);
        let compact = body_h < 420.;
        let shown = chat_display(&lines, (body_h * 3.).max(600.), chars);
        let key = format!("{}:{}", p.id, shown.len());
        if self.scrolled != key {
            self.scroll.scroll_to_bottom();
            self.scrolled = key;
        }
        let agent = provider_name(&p.provider);
        let mut chat = div()
            .id("lf-chat")
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .pr(px(16.));
        let media_style = MediaStyle {
            card: gpui::rgba(0x120D16FF),
            text: gpui::rgba(TXT << 8 | 0xFF),
            muted: gpui::rgba(MUTED << 8 | 0xFF),
            radius: 8.,
        };
        for (n, d) in shown.iter().enumerate() {
            if d.kind == crate::console::display::DKind::Meta {
                let found = lines
                    .iter()
                    .rev()
                    .find(|l| l.media.is_some() && l.text.trim_end() == d.text);
                if let Some(m) = found.and_then(|l| l.media.as_ref()) {
                    chat = chat.child(
                        div()
                            .flex_none()
                            .pl(px(100. * k))
                            .py(px(4.))
                            .child(media::preview(m, ("lf-media", n), media_style)),
                    );
                    continue;
                }
            }
            chat = chat.child(display_row(d, &agent, 84. * k, false, chars));
        }
        if shown.is_empty() {
            chat = chat.child(th("No rows yet"));
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
                .max_h(px(if compact { 88. } else { 190. }))
                .overflow_y_scroll()
                .border_l(px(10.))
                .border_color(rgb(RD))
                .pl(px(24.))
                .py(px(10.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .max_h(tsz(if compact { 48. } else { 72. }))
                        .overflow_hidden()
                        .font_family(CODE)
                        .text_size(tsz(S_BODY))
                        .line_height(tsz(24.))
                        .text_color(rgb(WHITE))
                        .child(ask.text.clone()),
                )
                .child(buttons)
        });

        let palette = compose::Palette {
            text: TXT << 8 | 0xFF,
            dim: MUTED << 8 | 0xFF,
            line: PU << 8 | 0x88,
            chip_bg: 0x120D16FF,
            hover: RULE << 8 | 0xFF,
            fail: RD << 8 | 0xFF,
            ok: OR << 8 | 0xFF,
        };
        let chips = compose::chips(&self.tools, "lf", palette, |i| {
            Box::new(cx.listener(move |this, _, _, cx| {
                this.tools.remove(i);
                cx.notify();
            }))
        });
        let field = div()
            .flex_none()
            .min_h(tsz(40.))
            .flex()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .child(div().w(px(10.)).h_full().bg(rgb(PU)).rounded_l(px(5.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .bg(rgb(0x120D16))
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .font_family(BODY)
                            .text_size(tsz(S_BODY))
                            .text_color(rgb(TXT))
                            .child(div().flex_1().min_w(px(0.)).child(self.input.clone())),
                    ),
            )
            .child(
                pill("lf-attach", PU, "Attach")
                    .min_h(tsz(40.))
                    .min_w(crate::ui::sw(88.))
                    .on_click(cx.listener(|_, _, window, cx| {
                        compose::attach(|this: &mut Self| Some(&mut this.tools), window, cx)
                    })),
            )
            .child(
                pill("lf-send", OR, "Send")
                    .min_h(tsz(40.))
                    .min_w(crate::ui::sw(88.))
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            );

        self.controls.sync(&p);
        let providers = self.model.read(cx).providers.clone();
        let style = ControlStyle::console(0x120D16, TXT, OR, Some(BODY.into()));
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

        let mut left = div()
            .size_full()
            .min_h(px(0.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(if compact { 10. } else { 20. }));
        if body_h >= 380. {
            left = left.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .truncate()
                            .font_family(BODY)
                            .text_size(tsz(S_BIG))
                            .line_height(tsz(36.))
                            .text_color(rgb(TXT))
                            .child(p.title.clone()),
                    )
                    .child(th(meta)),
            );
        } else if body_h >= 320. {
            left = left.child(div().flex_none().child(th(meta)));
        }
        let left = left
            .child(crate::ui::with_thumb(
                chat,
                &self.scroll,
                gpui::rgba(0x9C9CFFB3),
                6.,
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

        let mut right = div()
            .size_full()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(px(16.));
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
                                .text_size(tsz(S_CODE))
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
                            DiffLineKind::Added => ('+', PE),
                            DiffLineKind::Removed => ('−', RS),
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
                                .text_size(tsz(S_CODE))
                                .line_height(tsz(24.))
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
                                    .child(div().h(px(6.)).w(px(w_del)).bg(rgb(RD))),
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
                        .overflow_hidden()
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
                                .text_size(tsz(S_CODE))
                                .line_height(tsz(24.))
                                .text_color(rgb(MUTED))
                                .child(l),
                        );
                    }
                }
            }
        }

        let n_tabs = if narrow { 6. } else { 5. };
        let rt_h = ((col_h - (n_tabs - 1.) * 4. - 16.) / n_tabs).clamp(22., 34.);
        let side_tab = |id: &'static str, label: &str, side: Side, cx: &mut Context<Self>| {
            stab(
                id,
                label,
                PE,
                self.side == side && !(narrow && self.show_chat),
                rt_h,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.side = side;
                this.show_chat = false;
                cx.notify();
            }))
            .into_any_element()
        };
        let sid = p.id.clone();
        let open_path = current.map(|f| f.path.clone());
        let mut right_tabs = Vec::new();
        if narrow {
            right_tabs.push(
                stab(
                    "lf-chat-tab",
                    "Chat",
                    if waits { RD } else { PK },
                    self.show_chat,
                    rt_h,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_chat = true;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        right_tabs.extend([
            side_tab("lf-diff-tab", "Diff", Side::Diff, cx),
            side_tab("lf-files-tab", "Files", Side::Files, cx),
            side_tab("lf-console-tab", "Console", Side::Console, cx),
            side_tab("lf-tests-tab", "Tests", Side::Tests, cx),
            stab(
                SharedString::from("lf-lens"),
                "Open in Lens ›",
                OR,
                false,
                rt_h,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(path) = &open_path {
                    cx.emit(BridgeFocusEvent::OpenFile {
                        session: sid.clone(),
                        path: path.clone(),
                    });
                }
            }))
            .into_any_element(),
        ]);
        let (left_title, left_out, right_out) = if narrow {
            if self.show_chat {
                ("Chat", left.into_any_element(), div().into_any_element())
            } else {
                (
                    "Changes",
                    right.into_any_element(),
                    div().into_any_element(),
                )
            }
        } else {
            ("Chat", left.into_any_element(), right.into_any_element())
        };
        session_island(
            art,
            narrow,
            Island {
                stats,
                left_title,
                left_color: if waits { RD } else { PK },
                left_tabs,
                left: left_out,
                right_title: format!("Changes · {}", files.len()),
                right_tabs,
                right: right_out,
            },
        )
    }
}
