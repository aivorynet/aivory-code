//! The Bridge Focus screen (mockup `BridgeFocus`, nav 01·1): one session with its code. Two
//! islands under the frame of the shell. The left island is what you act on: the chat, the
//! open ask and the field. The right island holds the details: the diff, the files, the tests.
//! The button column at the left switches the session; the column at the right switches the
//! detail (diff, files, tests, console). Every control goes through `act`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use aivory_protocol::Pane;
use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::session::{DiffFile, DiffLineKind, SessionDiff};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ScrollHandle, SharedString, Subscription, Window, div, rgb,
};
use gpui_base::input::{InputEvent, InputState};

use crate::actions::{Action, act};
use crate::bridge_kit::*;
use crate::bridge_screens::{big, row};
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
    /// A drag of the scroll bar thumb: the mouse y, the scroll offset and the offset per px.
    drag: Rc<Cell<Option<(f32, f32, f32)>>>,
    /// The session and item count that the chat was last scrolled to the end for.
    scrolled: String,
    snap: u8,
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
            drag: Rc::new(Cell::new(None)),
            scrolled: String::new(),
            snap: 0,
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

    /// The LCARS scroll bar of the chat: a rounded track and a colored thumb. It follows the
    /// wheel, a click on the track jumps, and the thumb drags.
    fn lcars_scrollbar(&self, cx: &mut Context<Self>) -> AnyElement {
        use gpui::{DispatchPhase, point};
        let handle = self.scroll.clone();
        let max = f32::from(handle.max_offset().y);
        let off = -f32::from(handle.offset().y);
        let view = f32::from(handle.bounds().size.height);
        let span = crate::ui::thumb_span(max, off, view, view);
        let drag = self.drag.clone();
        let (down_drag, down_handle) = (drag.clone(), handle.clone());
        let mut track = div()
            .id("lf-scroll")
            .relative()
            .flex_none()
            .w(px(22.))
            .h_full()
            .rounded_full()
            .bg(rgb(0x3A2E4A))
            .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, _| {
                let h = &down_handle;
                let max = f32::from(h.max_offset().y);
                let mut off = -f32::from(h.offset().y);
                let view = f32::from(h.bounds().size.height);
                let Some((top, th)) = crate::ui::thumb_span(max, off, view, view) else {
                    return;
                };
                let y = f32::from(e.position.y) - f32::from(h.bounds().top());
                let free = (view - th).max(1.);
                if y < top || y > top + th {
                    off = ((y - th / 2.) / free * max).clamp(0., max);
                    h.set_offset(point(h.offset().x, px(-off)));
                }
                down_drag.set(Some((f32::from(e.position.y), off, max / free)));
                window.refresh();
            });
        if let Some((top, th)) = span {
            track = track.child(
                div()
                    .absolute()
                    .left(px(3.))
                    .top(px(top))
                    .w(px(16.))
                    .h(px(th))
                    .rounded_full()
                    .bg(rgb(PE))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(lit(PE)))),
            );
        }
        let moved = handle.clone();
        let events = gpui::canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let (d1, h1) = (drag.clone(), moved.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, _| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let Some((y0, off0, ratio)) = d1.get() else {
                        return;
                    };
                    if e.pressed_button != Some(MouseButton::Left) {
                        d1.set(None);
                        return;
                    }
                    let max = f32::from(h1.max_offset().y);
                    let off = (off0 + (f32::from(e.position.y) - y0) * ratio).clamp(0., max);
                    h1.set_offset(point(h1.offset().x, px(-off)));
                    window.refresh();
                });
                let d2 = drag.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, _| d2.set(None));
            },
        )
        .absolute()
        .size_0();
        let _ = cx;
        track
            .child(events)
            .child(crate::ui::scroll_watch(&handle, max, off))
            .into_any_element()
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
                    let demo = this.model.read(cx).demo;
                    let diff = match result.ok() {
                        Some(d) if !d.files.is_empty() => Some(d),
                        _ if demo => Some(demo_diff(&id)),
                        other => other,
                    };
                    this.diff = Some((diff, Instant::now()));
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// Demo only (`--demo`): the daemon is not there, so the Changes panel shows a small sample.
fn demo_diff(id: &str) -> SessionDiff {
    let line =
        |kind, old: Option<u32>, new: Option<u32>, text: &str| aivory_protocol::session::DiffLine {
            kind,
            old,
            new,
            text: text.to_string(),
            blame: None,
        };
    use DiffLineKind::{Added, Context, Hunk, Removed};
    SessionDiff {
        session: id.to_string(),
        files: vec![
            DiffFile {
                path: "src/payment/retry.rs".into(),
                added: 5,
                removed: 2,
                lines: vec![
                    line(
                        Hunk,
                        None,
                        None,
                        "@@ -18,7 +18,10 @@ pub fn delay(attempt: u32) -> Duration",
                    ),
                    line(Context, Some(18), Some(18), "    let base = BASE_MS;"),
                    line(
                        Removed,
                        Some(19),
                        None,
                        "    thread::sleep(Duration::from_secs(1));",
                    ),
                    line(Removed, Some(20), None, "    Duration::from_secs(1)"),
                    line(
                        Added,
                        None,
                        Some(19),
                        "    let delay = BASE_MS * 2u64.pow(attempt);",
                    ),
                    line(Added, None, Some(20), "    let capped = delay.min(30_000);"),
                    line(Added, None, Some(21), "    Duration::from_millis(capped)"),
                    line(Context, Some(21), Some(22), "}"),
                    line(Hunk, None, None, "@@ -40,3 +43,5 @@ mod tests"),
                    line(Added, None, Some(44), "    #[test]"),
                    line(
                        Added,
                        None,
                        Some(45),
                        "    fn the_delay_doubles_and_caps() {",
                    ),
                    line(Context, Some(40), Some(46), "    }"),
                ],
            },
            DiffFile {
                path: "migrations/0042_webhook_attempts.sql".into(),
                added: 2,
                removed: 0,
                lines: vec![
                    line(Hunk, None, None, "@@ -0,0 +1,2 @@"),
                    line(Added, None, Some(1), "ALTER TABLE webhook_events"),
                    line(
                        Added,
                        None,
                        Some(2),
                        "  ADD COLUMN attempts integer NOT NULL DEFAULT 0;",
                    ),
                ],
            },
        ],
    }
}

/// The lit tab.
const LIT: u32 = 0xFFE0C8;
/// The dim panel behind code (R3): darker than the chat, so the two never look alike.
const DIM_PANEL: u32 = 0x0E0A15;

/// The width of a vertical band of a panel frame.
const BAND: f32 = 36.;

/// One LCARS panel: a left and a right band with round caps, a top bar (`top`, a row 36 px
/// high), a bottom bar and four concave inner corners. `body` fills the inside. `x`, `w` and
/// `y0`..`yb` give the frame; `yb` is the top of the bottom bar.
fn panel(
    (x, w): (f32, f32),
    (y0, yb): (f32, f32),
    c: u32,
    top: impl IntoElement,
    body: impl IntoElement,
) -> Div {
    let h = yb + 36. - y0;
    let inner = |ex: f32, ey: f32| rect(ex, ey, 30., 30., c);
    div()
        .absolute()
        .left_0()
        .top_0()
        .size_full()
        .child(rect(x, y0, BAND, h, c).rounded_l(px(BAND)))
        .child(rect(x + w - BAND, y0, BAND, h, c).rounded_r(px(BAND)))
        .child(
            area(x + BAND, y0, w - 2. * BAND, 36.)
                .flex()
                .items_center()
                .gap(px(4.))
                .child(top),
        )
        .child(rect(x + BAND, yb, w - 2. * BAND, 36., c))
        .child(inner(x + BAND, y0 + 36.))
        .child(rect(x + BAND, y0 + 36., 30., 30., BLACK).rounded_tl(px(26.)))
        .child(inner(x + BAND, yb - 30.))
        .child(rect(x + BAND, yb - 30., 30., 30., BLACK).rounded_bl(px(26.)))
        .child(inner(x + w - BAND - 30., y0 + 36.))
        .child(rect(x + w - BAND - 30., y0 + 36., 30., 30., BLACK).rounded_tr(px(26.)))
        .child(inner(x + w - BAND - 30., yb - 30.))
        .child(rect(x + w - BAND - 30., yb - 30., 30., 30., BLACK).rounded_br(px(26.)))
        .child(
            area(x + 56., y0 + 52., w - 112., yb - 10. - (y0 + 52.))
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(body),
        )
}

/// The title at the right end of a panel's top bar.
fn panel_title(text: String, color: u32) -> Div {
    div()
        .flex_none()
        .px(px(8.))
        .font_family(LABEL)
        .text_size(tsz(S_BIG))
        .line_height(tsz(36.))
        .text_color(rgb(color))
        .whitespace_nowrap()
        .child(text.to_uppercase())
}

/// A tab of the top edge of a panel: black text on color; lit when on.
fn edge_tab(id: &'static str, label: &str, on: bool) -> gpui::Stateful<Div> {
    let c = if on { LIT } else { OR };
    div()
        .id(id)
        .flex_none()
        .h(px(36.))
        .min_w(px(96.))
        .px(px(18.))
        .flex()
        .items_end()
        .justify_end()
        .pb(px(5.))
        .bg(rgb(c))
        .font_family(LABEL)
        .text_size(tsz(S_LEAD))
        .line_height(tsz(S_LEAD))
        .text_color(rgb(BLACK))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(lit(c))))
        .child(label.to_uppercase())
}

/// A key of the key row: a pill, black on color; lit when on.
fn key(id: impl Into<SharedString>, label: &str, c: u32, on: bool) -> gpui::Stateful<Div> {
    let c = if on { LIT } else { c };
    div()
        .id(id.into())
        .flex_none()
        .h(px(34.))
        .px(px(16.))
        .rounded_full()
        .bg(rgb(c))
        .flex()
        .items_center()
        .font_family(LABEL)
        .text_size(tsz(S_SMALL))
        .line_height(tsz(S_SMALL))
        .text_color(rgb(BLACK))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(lit(c))))
        .child(label.to_uppercase())
}

/// The geometry of the session screen: the top and the bottom bar of the panels.
fn rows(art: Art) -> (f32, f32) {
    (300., art.by(848.))
}

/// The panels of the session screen. Wide: the chat panel and the changes panel side by side.
/// Narrow: one panel (`chat` says which). `tabs` are the tabs of the changes edge.
#[allow(clippy::too_many_arguments)]
fn session_frame(
    art: Art,
    narrow: bool,
    stats: Vec<(String, String, u32)>,
    keys: Div,
    chat_color: u32,
    chat: AnyElement,
    changes_title: String,
    tabs: Vec<gpui::Stateful<Div>>,
    changes: AnyElement,
) -> Div {
    let w = 1440. + art.dx;
    let (y0, yb) = rows(art);
    let mut st = div()
        .absolute()
        .left(px(316.))
        .top(px(88.))
        .flex()
        .gap(crate::ui::sw(if narrow { 28. } else { 64. }));
    for (label, value, color) in stats {
        st = st.child(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .gap(px(8.))
                .child(th(label).whitespace_nowrap())
                .child(
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
    let filler = |c: u32| div().flex_1().h(px(36.)).bg(rgb(c));
    let mut out = div()
        .size_full()
        .relative()
        .overflow_hidden()
        .child(st)
        .child(keys);
    if narrow {
        let top = div()
            .flex()
            .w_full()
            .gap(px(4.))
            .children(tabs)
            .child(filler(PE));
        out = out.child(panel((16., w - 32.), (y0, yb), PE, top, changes));
        let _ = (chat, chat_color, changes_title);
    } else {
        let end = (16. + 0.544 * (w - 72.)).max(636.);
        let top_chat = div()
            .flex()
            .w_full()
            .items_center()
            .child(filler(chat_color))
            .child(panel_title("Chat".into(), PE));
        let top_changes = div()
            .flex()
            .w_full()
            .gap(px(4.))
            .children(tabs)
            .child(filler(PE))
            .child(panel_title(changes_title, PE));
        out = out
            .child(panel(
                (16., end - 16.),
                (y0, yb),
                chat_color,
                top_chat,
                chat,
            ))
            .child(panel(
                (end + 24., w - 16. - (end + 24.)),
                (y0, yb),
                PE,
                top_changes,
                changes,
            ));
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
        let narrow = f32::from(vp.width) / kf().max(1.) < 1290.;
        let k = kf();
        let Some(p) = self.pane(cx) else {
            return session_frame(
                art,
                narrow,
                Vec::new(),
                div(),
                PK,
                big("This session is gone").into_any_element(),
                "Changes".into(),
                Vec::new(),
                div().into_any_element(),
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

        // ---- the key row: the session tabs, then War room at the right end ------------------
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
        let (_, yb) = rows(art);
        let body_h = yb - 10. - 352.;
        let win_w = 1440. + art.dx;
        // Whole names, as many as fit; the open session always shows.
        let tab_w = |name: &str| name.chars().count() as f32 * 6.6 + 40.;
        let room = win_w - 16. - 282. - 150. - 20. - 120.;
        let mut shown_ids: Vec<usize> = Vec::new();
        let mut used = 0.;
        for (i, s) in sessions.iter().enumerate() {
            let need = tab_w(&s.1) + 6.;
            if used + need > room && !shown_ids.is_empty() {
                break;
            }
            used += need;
            shown_ids.push(i);
        }
        if let Some(at) = sessions.iter().position(|s| s.0 == self.id)
            && !shown_ids.contains(&at)
        {
            shown_ids.pop();
            shown_ids.push(at);
        }
        let more = sessions.len() - shown_ids.len();
        let first_hidden = (0..sessions.len())
            .find(|i| !shown_ids.contains(i))
            .map(|i| sessions[i].0.clone());
        let mut keys = area(282., 256., win_w - 16. - 282., 34.)
            .flex()
            .items_center()
            .gap(px(6.));
        let mut tabs_row = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .gap(px(6.))
            .overflow_hidden();
        for k in shown_ids {
            let (sid, name, c) = &sessions[k];
            let sid = sid.clone();
            tabs_row = tabs_row.child(
                key(format!("lf-s-{k}"), name, *c, *sid == self.id)
                    .tooltip(crate::ui::tip(name.clone()))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(BridgeFocusEvent::Open(sid.clone()));
                    })),
            );
        }
        if more > 0 {
            let hidden = first_hidden.clone().unwrap_or_default();
            tabs_row = tabs_row.child(
                key("lf-more", &format!("+{more} more"), PU, false)
                    .tooltip(crate::ui::tip("Open the next hidden session".to_string()))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(BridgeFocusEvent::Open(hidden.clone()));
                    })),
            );
        }
        // Back lives in the header only (one Back per screen).
        keys = keys.child(tabs_row);

        let lines = p.tail.clone();
        // The chat column is wider on a large window: the line length follows its width.
        let left_w = if narrow {
            win_w - 32. - 112. - 24.
        } else {
            (16. + 0.544 * (win_w - 72.)).max(636.) - 16. - 112. - 24.
        };
        let chars = (((left_w - 116. * k - 16.) / (7.6 * k)) as usize).clamp(24, 160);
        let compact = body_h < 420.;
        let shown = chat_display(&lines, (body_h * 3.).max(600.), chars);
        let skey = format!("{}:{}", p.id, shown.len());
        if self.scrolled != skey {
            self.scroll.scroll_to_bottom();
            self.scrolled = skey;
            self.snap = 4;
            window.request_animation_frame();
        } else if self.snap > 0 {
            self.snap -= 1;
            crate::ui::snap_rows(&self.scroll);
            window.request_animation_frame();
        }
        let agent = provider_name(&p.provider);
        let mut chat = div()
            .id("lf-chat")
            .flex_initial()
            .max_h_full()
            .min_h(px(0.))
            .min_w(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .pt(px(8.))
            .pb(px(16.))
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
                .border_l(px(10.))
                .border_color(rgb(RD))
                .pl(px(24.))
                .py(px(10.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .max_h(tsz(if compact { 120. } else { 168. }))
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
        // The input is an LCARS element: round caps at both ends, a color frame, Antonio.
        let field = div()
            .flex_none()
            .h(px(44.))
            .flex()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex()
                    .child(
                        div()
                            .w(px(34.))
                            .h_full()
                            .flex_none()
                            .bg(rgb(OR))
                            .rounded_l(px(22.)),
                    )
                    .child(div().w(px(4.)).h_full().flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .bg(rgb(0x120D16))
                            .border_y(px(3.))
                            .border_color(rgb(PK))
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .font_family(BODY)
                            .text_size(tsz(S_BODY))
                            .text_color(rgb(TXT))
                            .child(div().flex_1().min_w(px(0.)).child(self.input.clone())),
                    )
                    .child(div().w(px(4.)).h_full().flex_none())
                    .child(
                        div()
                            .w(px(22.))
                            .h_full()
                            .flex_none()
                            .bg(rgb(PK))
                            .rounded_r(px(22.)),
                    ),
            )
            .child(
                pill("lf-attach", PU, "Attach")
                    .min_h(px(44.))
                    .min_w(crate::ui::sw(104.))
                    .text_size(tsz(S_LEAD))
                    .line_height(tsz(S_LEAD))
                    .on_click(cx.listener(|_, _, window, cx| {
                        compose::attach(|this: &mut Self| Some(&mut this.tools), window, cx)
                    })),
            )
            .child(
                pill("lf-send", OR, "Send")
                    .min_h(px(44.))
                    .min_w(crate::ui::sw(104.))
                    .text_size(tsz(S_LEAD))
                    .line_height(tsz(S_LEAD))
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            );

        self.controls.sync(&p);
        let providers = self.model.read(cx).providers.clone();
        // Plan, Edit, Auto and the pickers: LCARS pills, black on color.
        let style = ControlStyle {
            pill: rgb(PK),
            pill_hover: rgb(lit(PK)),
            pill_text: rgb(BLACK),
            on: rgb(OR),
            on_text: rgb(BLACK),
            menu: rgb(0x120D16),
            menu_text: rgb(TXT),
            menu_hover: rgb(RULE),
            font: Some(BODY.into()),
            radius: None,
            text: Some(S_LEAD),
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
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .min_w(px(0.))
                            .flex()
                            .gap(px(10.))
                            // A short chat sits at the bottom, above the input.
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .justify_end()
                                    .child(chat),
                            )
                            .child(self.lcars_scrollbar(cx)),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right(px(32.))
                            .h(px(20.))
                            .bg(gpui::linear_gradient(
                                180.,
                                gpui::linear_color_stop(gpui::rgba(0x00000000), 0.),
                                gpui::linear_color_stop(gpui::rgba(0x000000FF), 1.),
                            )),
                    ),
            )
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
                    // The file name is a title in Antonio; the numbers are a small label.
                    Some(f) => div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex_none()
                                .truncate()
                                .font_family(LABEL)
                                .text_size(tsz(S_LEAD))
                                .line_height(tsz(S_LEAD + 8.))
                                .text_color(rgb(OR))
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
                    None => div()
                        .flex_none()
                        .min_h(tsz(S_SMALL + 8.))
                        .child(th("No change yet · last output")),
                });
                if let Some(f) = current {
                    let mut lines_box = scroll_box("lf-diff");
                    for l in f.lines.iter().take(DIFF_LINES) {
                        let (mark, c, bg) = match l.kind {
                            DiffLineKind::Added => ('+', PE, Some(0x1B1A3A)),
                            DiffLineKind::Removed => ('−', RS, Some(0x3A1B20)),
                            DiffLineKind::Hunk => ('@', PU, None),
                            DiffLineKind::Context => (' ', MUTED, None),
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
                                .px(px(8.))
                                .when_some(bg, |d, b| d.bg(rgb(b)))
                                .when(l.kind == DiffLineKind::Hunk, |d| d.pt(px(6.)))
                                .font_family(CODE)
                                .text_size(tsz(S_CODE))
                                .line_height(tsz(20.))
                                .text_color(rgb(c))
                                .child(text),
                        );
                    }
                    right = right.child(lines_box);
                } else {
                    // No change: the pane shows the last output of the session.
                    right = right.child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_hidden()
                            .child(self.console.clone()),
                    );
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

        // The views of the changes sit as tabs on the top edge of the CHANGES panel.
        let side_tab = |id: &'static str, label: &str, side: Side, cx: &mut Context<Self>| {
            edge_tab(id, label, self.side == side && !(narrow && self.show_chat)).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.side = side;
                    this.show_chat = false;
                    cx.notify();
                }),
            )
        };
        let mut tabs = Vec::new();
        if narrow {
            tabs.push(
                edge_tab("lf-chat-tab", "Chat", self.show_chat).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.show_chat = true;
                        cx.notify();
                    },
                )),
            );
        }
        tabs.extend([
            side_tab("lf-diff-tab", "Diff", Side::Diff, cx),
            side_tab("lf-files-tab", "Files", Side::Files, cx),
            side_tab("lf-console-tab", "Console", Side::Console, cx),
            side_tab("lf-tests-tab", "Tests", Side::Tests, cx),
        ]);
        // R3: code sits on a dim panel with its own frame line, apart from the chat.
        let right = div()
            .size_full()
            .min_h(px(0.))
            .rounded(px(12.))
            .bg(rgb(DIM_PANEL))
            .border_l(px(4.))
            .border_color(rgb(PU))
            .p(px(14.))
            .overflow_hidden()
            .child(right);
        let (left_out, right_out) = if narrow {
            if self.show_chat {
                (left.into_any_element(), div().into_any_element())
            } else {
                (div().into_any_element(), right.into_any_element())
            }
        } else {
            (left.into_any_element(), right.into_any_element())
        };
        let shown_body = if narrow && self.show_chat {
            left_out
        } else if narrow {
            right_out
        } else {
            // Wide: the changes panel takes `right_out`; the chat panel takes `left_out`.
            return session_frame(
                art,
                narrow,
                stats,
                keys,
                if waits { RD } else { PK },
                left_out,
                format!("Changes · {}", files.len()),
                tabs,
                right_out,
            );
        };
        session_frame(
            art,
            narrow,
            stats,
            keys,
            PE,
            div().into_any_element(),
            String::new(),
            tabs,
            shown_body,
        )
    }
}
