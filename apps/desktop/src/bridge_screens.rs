//! The Bridge screens Inbox, Tasks, Repos and Settings (mockups `BridgeInbox`, `BridgeTasks`,
//! `BridgeRepos`, `BridgeSettings`). Each is two islands on black under the frame of the shell:
//! the left island lists, the right island shows the one selected thing, and both have a
//! button column. `island` draws the frame pieces; a view only fills the content.

use std::rc::Rc;
use std::time::Duration;

use aivory_protocol::rpc::{
    InboxListParams, TaskListParams, TaskStartParams, TaskUpdateParams, method,
};
use aivory_protocol::{
    InboxFilter, InboxItem, InboxKind, InboxList, Pane, StepState, Task, TaskColumn, TaskList,
    rpc::AutopilotPatch,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, SharedString, Stateful, Task as GpuiTask,
    Window, div, rgb,
};

use crate::actions::{Action, act};
use crate::bridge_kit::*;
use crate::model::Model;

/// What a screen asks the shell to do.
#[derive(Debug, Clone)]
pub enum BridgeEvent {
    /// Open the pairing card of the full settings screen.
    PairPhone,
    OpenSession(String),
    /// The full Settings screen.
    AllSettings,
}

/// The lit tab.
const LIT: u32 = 0xFFE0C8;

/// A button of a column: lit when on.
pub(crate) fn tab(id: impl Into<SharedString>, label: &str, color: u32, on: bool) -> Stateful<Div> {
    ibtn_flow(id, if on { LIT } else { color }, label)
        .w(px(170.))
        .h(px(34.))
        .flex_none()
}

/// Everything the island frame needs.
pub struct Island {
    pub stats: Vec<(String, String, u32)>,
    pub left_title: &'static str,
    pub left_color: u32,
    pub left_tabs: Vec<AnyElement>,
    pub left: AnyElement,
    pub right_title: String,
    pub right_tabs: Vec<AnyElement>,
    pub right: AnyElement,
}

/// The layer of an island screen: the stats row, two islands, and their button columns.
pub fn island(art: Art, i: Island) -> Div {
    let c = i.left_color;
    let mut stats = div()
        .absolute()
        .left(px(316.))
        .top(px(88.))
        .flex()
        .gap(px(if art.dx < 200. { 28. } else { 64. }));
    for (label, value, color) in i.stats {
        stats = stats.child(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .gap(px(8.))
                .child(th(label).whitespace_nowrap())
                .child(
                    div()
                        .font_family(LABEL)
                        .text_size(px(S_BIG))
                        .line_height(px(34.))
                        .text_color(rgb(color))
                        .child(value),
                ),
        );
    }
    let title = |text: String, color: u32| {
        div()
            .flex_none()
            .font_family(LABEL)
            .text_size(px(S_BIG))
            .line_height(px(36.))
            .text_color(rgb(color))
            .child(text.to_uppercase())
    };
    let col_h = art.by(832.) - 320.;
    let bh = body_h(art);
    div()
        .size_full()
        .relative()
        .overflow_hidden()
        .child(stats)
        // The left island.
        .child(rect(16., ISL_T, 300., 52., c).rounded_tl(px(46.)))
        .child(rect(186., ISL_T + 30., 133., 22., BLACK).rounded_tl(px(20.)))
        .child(
            area(320., ISL_T, 440., 30.)
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().flex_1().h(px(30.)).bg(rgb(c)))
                .child(title(i.left_title.to_string(), PE)),
        )
        .child(
            area(16., ISL_T + 58., 170., col_h)
                .flex()
                .flex_col()
                .gap(px(3.))
                .children(i.left_tabs)
                .child(div().flex_1().bg(rgb(c))),
        )
        .child(rect(16., art.by(832.), 300., 52., SK).rounded_bl(px(46.)))
        .child(rect(186., art.by(832.), 133., 22., BLACK).rounded_bl(px(20.)))
        .child(rect(320., art.by(862.), 400., 22., SK))
        .child(rect(724., art.by(862.), 22., 22., SK).rounded_r(px(11.)))
        .child(
            area(222., BODY_T, 504., bh)
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(i.left),
        )
        // The right island.
        .child(
            area(800., ISL_T, 320. + art.dx, 30.)
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    div()
                        .flex_none()
                        .size(px(30.))
                        .bg(rgb(PE))
                        .rounded_l(px(15.)),
                )
                .child(div().px(px(8.)).child(title(i.right_title, PE)))
                .child(div().flex_1().h(px(30.)).bg(rgb(PE))),
        )
        .child(rect(art.rx(1124.), ISL_T, 300., 52., PE).rounded_tr(px(46.)))
        .child(rect(art.rx(1124.) - 3., ISL_T + 30., 133., 22., BLACK).rounded_tr(px(20.)))
        .child(
            area(art.rx(1254.), ISL_T + 58., 170., col_h)
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(i.right_tabs)
                .child(div().flex_1().bg(rgb(SK))),
        )
        .child(rect(art.rx(1124.), art.by(832.), 300., 52., SK).rounded_br(px(46.)))
        .child(rect(art.rx(1124.) - 3., art.by(832.), 133., 22., BLACK).rounded_br(px(20.)))
        .child(rect(840., art.by(862.), 280. + art.dx, 22., SK))
        .child(rect(800., art.by(862.), 22., 22., SK).rounded_l(px(11.)))
        .child(
            area(856., BODY_T, 352. + art.dx, bh)
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(i.right),
        )
}

/// The top of an island frame, in art px.
const ISL_T: f32 = 264.;
/// The top of the text in an island.
const BODY_T: f32 = 310.;

/// The height of the text area in an island, in art px.
pub fn body_h(art: Art) -> f32 {
    art.by(848.) - BODY_T
}

/// A row of a list: a number chip, a title with a line below, and a note at the right.
#[allow(clippy::too_many_arguments)]
pub(crate) fn row(
    id: impl Into<SharedString>,
    no: &str,
    chip: u32,
    title: &str,
    sub: &str,
    note: &str,
    note_color: u32,
    on: bool,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .min_h(px(64.))
        .py(px(8.))
        .flex()
        .items_center()
        .gap(px(14.))
        .border_t(px(3.))
        .border_color(rgb(RULE))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0x120D16)))
        .when(on, |d| d.bg(rgb(0x1A1220)))
        .child(
            div()
                .flex_none()
                .w(px(44.))
                .h(px(24.))
                .rounded_l(px(12.))
                .bg(rgb(chip))
                .flex()
                .items_center()
                .justify_center()
                .font_family(LABEL)
                .text_size(px(S_SMALL))
                .text_color(rgb(BLACK))
                .child(no.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .font_family(BODY)
                        .text_size(px(S_BODY))
                        .line_height(px(LH_BODY))
                        .text_color(rgb(TXT))
                        .child(title.to_string()),
                )
                .when(!sub.is_empty(), |d| {
                    d.child(
                        div()
                            .truncate()
                            .font_family(BODY)
                            .text_size(px(S_SMALL))
                            .text_color(rgb(MUTED))
                            .child(sub.to_string()),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .font_family(LABEL)
                .text_size(px(S_SMALL))
                .text_color(rgb(note_color))
                .child(note.to_uppercase()),
        )
}

/// A quiet row under a list that selects the next hidden entry: "+3 more".
pub(crate) fn more_row(id: &'static str, text: String) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(32.))
        .flex()
        .items_center()
        .cursor_pointer()
        .child(th(text).text_color(rgb(OR)))
}

/// A block of prose in the right island.
pub(crate) fn prose(text: &str) -> Div {
    div()
        .font_family(BODY)
        .text_size(px(S_BODY))
        .line_height(px(LH_BODY))
        .text_color(rgb(TXT))
        .child(text.to_string())
}

/// A block of code or a path.
pub(crate) fn code(text: &str, color: u32) -> Div {
    div()
        .font_family(CODE)
        .text_size(px(S_CODE))
        .line_height(px(24.))
        .text_color(rgb(color))
        .child(text.to_string())
}

/// The big line of a decision or a title.
pub(crate) fn big(text: &str) -> Div {
    div()
        .font_family(BODY)
        .text_size(px(S_LEAD))
        .line_height(px(28.))
        .text_color(rgb(WHITE))
        .child(text.to_string())
}

/// One line of a transcript or a file list: it never wraps.
fn one_line(text: &str) -> Div {
    prose(text).truncate().h(px(LH_BODY)).flex_none()
}

/// A small frame of a look: an elbow, two bars and a title, drawn in its five colors.
fn look_preview(name: &str, c: [u32; 5], bg: u32) -> Div {
    let bar = |x: f32, y: f32, w: f32, h: f32, col: u32| {
        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .bg(rgb(col))
    };
    div()
        .relative()
        .flex_none()
        .w(px(300.))
        .h(px(110.))
        .bg(rgb(bg))
        .child(bar(0., 0., 70., 56., c[0]).rounded_tl(px(26.)))
        .child(bar(34., 24., 36., 32., bg).rounded_tl(px(12.)))
        .child(bar(74., 0., 150., 14., c[1]))
        .child(bar(228., 0., 72., 14., c[2]))
        .child(bar(0., 60., 34., 18., c[3]))
        .child(bar(0., 82., 34., 28., c[4]))
        .child(bar(74., 96., 226., 14., c[1]).rounded_r(px(7.)))
        .child(
            div()
                .absolute()
                .left(px(84.))
                .top(px(26.))
                .font_family(LABEL)
                .text_size(px(S_TITLE))
                .line_height(px(S_TITLE))
                .text_color(rgb(c[3]))
                .child(name.to_uppercase()),
        )
        .child(
            div()
                .absolute()
                .left(px(84.))
                .top(px(58.))
                .font_family(BODY)
                .text_size(px(S_SMALL))
                .text_color(rgb(c[2]))
                .child("Sessions · live"),
        )
}

/// What a look is: a line about it, its type scale, and five swatch colors.
fn look_card(look: aivory_protocol::Look) -> (&'static str, &'static str, [u32; 5]) {
    use aivory_protocol::Look;
    match look {
        Look::Minimal => (
            "The calm orchestration board: modules, streams, gates, a log and a queue.",
            "Inter 12 · 14 · 18 · 28",
            [0x2F6FEB, 0x4C9A6A, 0xE0A030, 0xD0584C, 0x8A8F9C],
        ),
        Look::Bridge => (
            "A starship console: colored frame islands on black, numbered segments, one decision at a time.",
            "Antonio 64 · 28 · 18 · 14 · Plex Mono 14",
            [OR, PK, PE, GD, RD],
        ),
        Look::Operator => (
            "An operator wall: code rain per agent on green screens, blue screens for decisions.",
            "Mono 14 · 16 · 22",
            [0x00FF66, 0x0A8F3C, 0x2A6CFF, 0x0B1F3A, 0xFFFFFF],
        ),
        Look::Neon => (
            "A neon city: one district per codebase, one tower per session, HUD panels on top.",
            "Rajdhani 14 · 16 · 22 · 36",
            [0xFF2A6D, 0x05D9E8, 0xD1F7FF, 0x01012B, 0xF9C80E],
        ),
        Look::Navigator => (
            "A 90s workstation: toolchest windows over a 3D file landscape.",
            "System 12 · 14 · 18",
            [0xC0C0C0, 0x000080, 0x008080, 0xFFFFFF, 0x808080],
        ),
        Look::Holo => (
            "Calm see-through glass: few panels, slow rings, one decision at a time.",
            "Sans 12 · 14 · 16 · 28",
            [0x7FE3FF, 0xA48BFF, 0xFFA45C, 0x8B97A8, 0x0B1220],
        ),
        Look::Tactical => (
            "A radar: sectors per codebase, blips per session, phosphor green, white for orders.",
            "B612 12 · 14 · 18 · 32",
            [0x39FF6A, 0x0F5A2A, 0xFFFFFF, 0x081A0E, 0x7A9A84],
        ),
        Look::Command => (
            "A command console: units per codebase on a map, a command sidebar, amber for a wait.",
            "Saira Condensed 12 · 16 · 22 · 40",
            [0x9BCB5B, 0xFFB800, 0xF06A55, 0x555D42, 0x101309],
        ),
    }
}

/// The last prose rows of a transcript as `You: text` lines.
fn chat_text(tail: &[aivory_protocol::Line], n: usize) -> Vec<String> {
    use aivory_protocol::LineKind;
    let mut rows: Vec<String> = tail
        .iter()
        .filter_map(|l| {
            let t = l.text.lines().next().unwrap_or("").trim();
            match l.kind {
                LineKind::User if !t.is_empty() => Some(format!("You: {t}")),
                LineKind::Agent if t.chars().next().is_some_and(char::is_alphabetic) => {
                    Some(format!("Agent: {t}"))
                }
                _ => None,
            }
        })
        .collect();
    let skip = rows.len().saturating_sub(n);
    rows.split_off(skip)
}

pub(crate) fn minutes(now: i64, since: i64) -> String {
    let m = ((now - since) / 60_000).max(0);
    if m < 1 {
        "now".to_string()
    } else {
        format!("{m} min")
    }
}

/// Reload a list now and then.
fn ticker<V: 'static>(
    cx: &mut Context<V>,
    every: Duration,
    f: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> GpuiTask<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(every).await;
            if this.update(cx, |t, cx| f(t, cx)).is_err() {
                break;
            }
        }
    })
}

// =============================================================================== Inbox ====

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InboxTab {
    All,
    Waiting,
    Errors,
    Reviews,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Details,
    Files,
    Chat,
}

pub struct BridgeInbox {
    model: Entity<Model>,
    data: Option<Rc<InboxList>>,
    tab: InboxTab,
    side: Side,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
    _obs: gpui::Subscription,
}

impl EventEmitter<BridgeEvent> for BridgeInbox {}

impl BridgeInbox {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |this, _, cx| this.refresh(cx));
        let tick = ticker(cx, Duration::from_secs(5), |t: &mut Self, cx| t.refresh(cx));
        let mut this = Self {
            model,
            data: None,
            tab: InboxTab::All,
            side: Side::Details,
            selected: None,
            busy: false,
            _tick: tick,
            _obs: obs,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let call = self
            .model
            .read(cx)
            .call::<method::InboxListMethod>(InboxListParams::default(), cx);
        cx.spawn(async move |this, cx| {
            let list = call.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if let Ok(list) = list {
                    let changed = this.data.as_deref() != Some(&list);
                    this.data = Some(Rc::new(list));
                    if changed {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// The items of the tab, and the one on screen.
    fn items(&self) -> Vec<InboxItem> {
        let Some(d) = &self.data else {
            return Vec::new();
        };
        let f = |filter: InboxFilter| {
            d.items
                .iter()
                .filter(|i| filter.matches(i.kind))
                .cloned()
                .collect()
        };
        match self.tab {
            InboxTab::All => d.items.clone(),
            InboxTab::Waiting => f(InboxFilter::Answers),
            InboxTab::Errors => f(InboxFilter::Errors),
            InboxTab::Reviews => f(InboxFilter::Reviews),
            InboxTab::Done => d.done.clone(),
        }
    }
}

fn kind_color(k: InboxKind) -> u32 {
    match k {
        InboxKind::Error => GD,
        _ => RS,
    }
}

impl Render for BridgeInbox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let art = Art::of(window);
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let items = self.items();
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| items.iter().find(|i| &i.id == id))
            .or(items.first())
            .cloned();
        let all = self.data.as_ref().map_or(0, |d| d.items.len());
        let count = |f: InboxFilter| {
            self.data
                .as_ref()
                .map_or(0, |d| d.items.iter().filter(|i| f.matches(i.kind)).count())
        };
        let done = self.data.as_ref().map_or(0, |d| d.done.len());
        let oldest = self
            .data
            .as_ref()
            .and_then(|d| d.items.iter().map(|i| i.since).min())
            .map_or("none".to_string(), |s| minutes(now, s));
        let stats = vec![
            (
                "Waiting for you".to_string(),
                count(InboxFilter::Answers).to_string(),
                RS,
            ),
            (
                "Errors".to_string(),
                count(InboxFilter::Errors).to_string(),
                GD,
            ),
            ("Oldest".to_string(), oldest, TXT),
            ("Done today".to_string(), done.to_string(), TXT),
        ];

        let mut left_tabs: Vec<AnyElement> = Vec::new();
        for (t, label, color) in [
            (InboxTab::All, format!("All {all:02}"), RS),
            (
                InboxTab::Waiting,
                format!("Waiting {:02}", count(InboxFilter::Answers)),
                RS,
            ),
            (
                InboxTab::Errors,
                format!("Errors {:02}", count(InboxFilter::Errors)),
                GD,
            ),
            (
                InboxTab::Reviews,
                format!("Reviews {:02}", count(InboxFilter::Reviews)),
                SA,
            ),
            (InboxTab::Done, format!("Done {done:02}"), SA),
        ] {
            left_tabs.push(
                tab(format!("li-tab-{label}"), &label, color, self.tab == t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = t;
                        this.selected = None;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }

        // The left island: the decision on screen, then the next in line.
        let mut left = div().flex().flex_col();
        let mut right = div().flex().flex_col().gap(px(12.));
        let mut right_title = "Detail".to_string();
        if let Some(it) = &cur {
            let at = items.iter().position(|i| i.id == it.id).unwrap_or(0);
            left = left
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            sec(format!(
                                "{} {:02} of {:02} · {}",
                                if self.tab == InboxTab::Done {
                                    "Done"
                                } else {
                                    "Item"
                                },
                                at + 1,
                                items.len(),
                                it.kind.label().to_lowercase()
                            ))
                            .text_color(rgb(kind_color(it.kind))),
                        )
                        .child(th(format!("Waits {}", minutes(now, it.since)))),
                )
                .child(big(&it.title).mt(px(12.)))
                .child(
                    div()
                        .mt(px(6.))
                        .font_family(BODY)
                        .text_size(px(S_SMALL))
                        .text_color(rgb(MUTED))
                        .child(format!(
                            "{} · {} / {}",
                            it.agents
                                .first()
                                .map_or(String::new(), |a| a.provider.clone()),
                            it.codebase,
                            it.part
                        )),
                );
            let mut pills = div().mt(px(22.)).flex().gap(px(8.));
            if let Some(result) = &it.result {
                pills = pills.child(th(format!("Done · {result}")));
            } else {
                let n = it.options.len();
                for (k, o) in it.options.iter().enumerate() {
                    let (id, opt) = (it.id.clone(), o.clone());
                    pills = pills.child(
                        pill(
                            format!("li-opt-{k}"),
                            option_color(k, n),
                            &option_text(k, o),
                        )
                        .px(px(18.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            act(
                                &this.model,
                                Action::InboxAnswer {
                                    id: id.clone(),
                                    option: Some(opt.clone()),
                                    text: None,
                                },
                                cx,
                            );
                            this.refresh(cx);
                        })),
                    );
                }
            }
            left = left.child(pills);
            let rest: Vec<&InboxItem> = items.iter().filter(|i| i.id != it.id).collect();
            let next_rows = ((((body_h(art) - 250.) / 66.).floor()) as usize).clamp(2, 6);
            if !rest.is_empty() {
                let mut list = div()
                    .mt(px(PAD))
                    .flex()
                    .flex_col()
                    .child(sec("Next in line").mb(px(12.)));
                for (k, o) in rest.iter().take(next_rows).enumerate() {
                    let id = o.id.clone();
                    list = list.child(
                        row(
                            format!("li-row-{k}"),
                            &format!(
                                "{:02}",
                                items.iter().position(|i| i.id == o.id).unwrap_or(0) + 1
                            ),
                            kind_color(o.kind),
                            &o.title,
                            &format!("{} · {}", o.codebase, o.kind.label()),
                            &minutes(now, o.since),
                            kind_color(o.kind),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = Some(id.clone());
                            cx.notify();
                        })),
                    );
                }
                left = left.child(list);
            }

            // The right island.
            right_title = it.kind.heading().to_string();
            match self.side {
                Side::Details => {
                    right = right
                        .child(sec("Why it asks"))
                        .child(prose(if it.preview.is_empty() {
                            &it.text
                        } else {
                            &it.preview
                        }))
                        .child(sec("Command or question"))
                        .child(code(&it.text, RS));
                    if !it.steps.is_empty() {
                        right = right.child(sec("Plan")).children(
                            it.steps
                                .iter()
                                .take(3)
                                .enumerate()
                                .map(|(k, s)| one_line(&format!("{}. {s}", k + 1))),
                        );
                    }
                    let chat = chat_text(&it.tail, 3);
                    if !chat.is_empty() {
                        right = right
                            .child(sec("Latest chat"))
                            .children(chat.iter().map(|r| one_line(r)));
                    }
                    if !it.files.is_empty() {
                        right = right
                            .child(sec(format!("Files changed · {}", it.files.len())))
                            .children(it.files.iter().take(3).map(|f| code(f, MUTED).truncate()));
                    }
                }
                Side::Files => {
                    right = right.child(sec(format!("Files changed · {}", it.files.len())));
                    for f in it.files.iter().take(12) {
                        right = right.child(code(f, TXT));
                    }
                }
                Side::Chat => {
                    right = right.child(sec("Last rows of the chat"));
                    for r in chat_text(&it.tail, 8) {
                        right = right.child(prose(&r));
                    }
                }
            }
        } else {
            left = left.child(big("Nothing waits for you"));
        }
        let open = cur.as_ref().map(|i| i.session.clone());
        let mut right_tabs: Vec<AnyElement> = Vec::new();
        for (s, label) in [
            (Side::Details, "Details"),
            (Side::Files, "Files"),
            (Side::Chat, "Chat"),
        ] {
            right_tabs.push(
                tab(format!("ri-tab-{label}"), label, PK, self.side == s)
                    .h(px(40.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.side = s;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        right_tabs.push(
            tab("ri-open", "Open session ›", OR, false)
                .h(px(40.))
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(id) = &open {
                        cx.emit(BridgeEvent::OpenSession(id.clone()));
                    }
                }))
                .into_any_element(),
        );
        island(
            art,
            Island {
                stats,
                left_title: "Inbox",
                left_color: RS,
                left_tabs,
                left: left.into_any_element(),
                right_title,
                right_tabs,
                right: right.into_any_element(),
            },
        )
    }
}

// =============================================================================== Tasks ====

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskTab {
    Queued,
    Running,
    Review,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskSide {
    Plan,
    Sessions,
}

/// A task action of a task screen: start a planned task, or move a task in Review to Done.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskAct {
    Start,
    Done,
}

impl TaskAct {
    /// The action that fits the column of a task, if there is one.
    pub(crate) fn of(column: TaskColumn) -> Option<Self> {
        match column {
            TaskColumn::Planned => Some(Self::Start),
            TaskColumn::Review => Some(Self::Done),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Done => "Done",
        }
    }
}

/// Call the daemon for a task action. A failure shows as a notice. Then `after` runs.
pub(crate) fn task_act<V: 'static>(
    model: &Entity<Model>,
    id: &str,
    act: TaskAct,
    after: fn(&mut V, &mut Context<V>),
    cx: &mut Context<V>,
) {
    let id = id.to_string();
    let m = model.read(cx);
    let call = match act {
        TaskAct::Start => {
            let p = TaskStartParams { id, composer: None };
            let c = m.call_notify::<method::TasksStart>(p, cx);
            cx.background_spawn(async move { c.await.map(|_| ()) })
        }
        TaskAct::Done => {
            let p = TaskUpdateParams {
                id,
                column: Some(TaskColumn::Done),
                ..Default::default()
            };
            let c = m.call_notify::<method::TasksUpdate>(p, cx);
            cx.background_spawn(async move { c.await.map(|_| ()) })
        }
    };
    cx.spawn(async move |this, cx| {
        call.await;
        let _ = this.update(cx, after);
    })
    .detach();
}

pub struct BridgeTasks {
    model: Entity<Model>,
    data: Option<Rc<TaskList>>,
    tab: TaskTab,
    side: TaskSide,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
}

impl EventEmitter<BridgeEvent> for BridgeTasks {}

impl BridgeTasks {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let tick = ticker(cx, Duration::from_secs(5), |t: &mut Self, cx| t.refresh(cx));
        let mut this = Self {
            model,
            data: None,
            tab: TaskTab::Running,
            side: TaskSide::Plan,
            selected: None,
            busy: false,
            _tick: tick,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let call = self
            .model
            .read(cx)
            .call::<method::TasksList>(TaskListParams::default(), cx);
        cx.spawn(async move |this, cx| {
            let list = call.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if let Ok(list) = list {
                    let changed = this.data.as_deref() != Some(&list);
                    this.data = Some(Rc::new(list));
                    if changed {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Load the list again after a task action.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.busy = false;
        self.refresh(cx);
    }

    fn of(&self, tab: TaskTab) -> Vec<Task> {
        let Some(d) = &self.data else {
            return Vec::new();
        };
        d.tasks
            .iter()
            .filter(|t| match tab {
                TaskTab::Queued => t.column == TaskColumn::Planned,
                TaskTab::Running => matches!(t.column, TaskColumn::Working | TaskColumn::Waiting),
                TaskTab::Review => t.column == TaskColumn::Review,
                TaskTab::Done => t.column == TaskColumn::Done,
            })
            .cloned()
            .collect()
    }
}

fn step_color(s: StepState) -> u32 {
    match s {
        StepState::Done => PK,
        StepState::Working => OR,
        StepState::Waiting => RS,
        StepState::NotStarted => GY,
    }
}

fn step_word(s: StepState) -> &'static str {
    match s {
        StepState::Done => "Done",
        StepState::Working => "Working",
        StepState::Waiting => "Waits for you",
        StepState::NotStarted => "Queued",
    }
}

impl Render for BridgeTasks {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let art = Art::of(window);
        let (autopilot, queued_n) = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map_or((false, 0), |w| (w.autopilot.on, w.queued));
        let n = |t| self.of(t).len();
        let stats = vec![
            ("Queued".to_string(), n(TaskTab::Queued).to_string(), TXT),
            ("Running".to_string(), n(TaskTab::Running).to_string(), OR),
            ("In review".to_string(), n(TaskTab::Review).to_string(), GD),
            ("Done".to_string(), n(TaskTab::Done).to_string(), TXT),
        ];
        let mut left_tabs: Vec<AnyElement> = Vec::new();
        for (t, label, color) in [
            (TaskTab::Queued, "Queued", PK),
            (TaskTab::Running, "Running", OR),
            (TaskTab::Review, "Review", GD),
            (TaskTab::Done, "Done", SA),
        ] {
            left_tabs.push(
                tab(
                    format!("lt-tab-{label}"),
                    &format!("{label} {:02}", n(t)),
                    color,
                    self.tab == t,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = t;
                    this.selected = None;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        let tasks = self.of(self.tab);
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.id == id))
            .or(tasks.first())
            .cloned();
        let mut left = div().flex().flex_col().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .mb(px(12.))
                .child(sec(format!(
                    "{} · {} tasks",
                    ["Queued", "Running", "Review", "Done"][self.tab as usize],
                    tasks.len()
                )))
                .child(
                    pill(
                        "lt-autopilot",
                        PK,
                        &format!(
                            "Autopilot {} · {queued_n} queued",
                            if autopilot { "on" } else { "off" }
                        ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        act(
                            &this.model,
                            Action::Autopilot(AutopilotPatch {
                                on: Some(!autopilot),
                                ..AutopilotPatch::default()
                            }),
                            cx,
                        );
                    })),
                ),
        );
        // Four whole rows; the window follows the selected task.
        let cur_i = cur
            .as_ref()
            .and_then(|c| tasks.iter().position(|t| t.id == c.id))
            .unwrap_or(0);
        let fit_rows = ((((body_h(art) - 80.) / 65.).floor()) as usize).clamp(3, 9);
        let start = cur_i
            .saturating_sub(fit_rows - 1)
            .min(tasks.len().saturating_sub(fit_rows));
        if start > 0 {
            let prev = tasks[start - 1].id.clone();
            left = left.child(more_row("lt-earlier", format!("{start} earlier")).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.selected = Some(prev.clone());
                    cx.notify();
                }),
            ));
        }
        for (k, t) in tasks.iter().enumerate().skip(start).take(fit_rows) {
            let id = t.id.clone();
            let c = if t.waiting_for.is_some() { RS } else { OR };
            let mut r = row(
                format!("lt-row-{k}"),
                &format!("{:02}", k + 1),
                c,
                &t.title,
                &format!(
                    "{} · {}",
                    t.agents.first().cloned().unwrap_or_default(),
                    t.codebase
                ),
                &format!("step {}", (t.step + 1).min(5)),
                c,
                cur.as_ref().is_some_and(|x| x.id == t.id),
            );
            r = r.on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(id.clone());
                cx.notify();
            }));
            left = left.child(r);
        }
        let hidden = tasks.len().saturating_sub(start + fit_rows);
        if hidden > 0 {
            let next = tasks[start + fit_rows].id.clone();
            left = left.child(more_row("lt-more", format!("+{hidden} more")).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.selected = Some(next.clone());
                    cx.notify();
                }),
            ));
        }

        let mut right = div().flex().flex_col().gap(px(14.));
        let mut open: Option<String> = None;
        if let Some(t) = &cur {
            open = t.sessions.first().map(|s| s.session.clone());
            let latest: Vec<String> = self
                .model
                .read(cx)
                .war
                .as_ref()
                .and_then(|w| {
                    t.sessions
                        .iter()
                        .find_map(|s| w.sessions.iter().find(|p| p.id == s.session))
                })
                .map(|p| chat_text(&p.tail, 3))
                .unwrap_or_default();
            right = right.child(big(&t.title)).child(th(format!(
                "{} · {} / {} · ${:.2}",
                t.agents.first().cloned().unwrap_or_default(),
                t.codebase,
                t.part,
                t.cost
            )));
            match self.side {
                TaskSide::Plan => {
                    right = right.child(sec(format!("Plan · {} steps", t.steps.len())).mt(px(16.)));
                    // Only whole rows: the window starts one step before the first open step.
                    let fit = ((((body_h(art) - 150. - 110.) / 54.).floor()) as usize).clamp(1, 12);
                    // All steps, from step 1; the list scrolls when it is longer than the fit.
                    let mut list = div()
                        .id("lt-plan-list")
                        .flex()
                        .flex_col()
                        .max_h(px(fit as f32 * 54.))
                        .overflow_y_scroll();
                    for (k, s) in t.steps.iter().enumerate() {
                        list = list.child(
                            div()
                                .flex_none()
                                .py(px(6.))
                                .border_t(px(3.))
                                .border_color(rgb(RULE))
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .child(
                                    div()
                                        .w(px(32.))
                                        .h(px(24.))
                                        .rounded_l(px(12.))
                                        .bg(rgb(step_color(s.state)))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .font_family(LABEL)
                                        .text_size(px(S_SMALL))
                                        .text_color(rgb(BLACK))
                                        .child((k + 1).to_string()),
                                )
                                .child(div().flex_1().child(prose(&s.name)))
                                .child(
                                    div()
                                        .font_family(LABEL)
                                        .text_size(px(S_SMALL))
                                        .text_color(rgb(if s.state == StepState::Waiting {
                                            RS
                                        } else {
                                            MUTED
                                        }))
                                        .child(step_word(s.state).to_uppercase()),
                                ),
                        );
                    }
                    right = right.child(list);
                    if t.steps.len() > fit {
                        right = right
                            .child(th(format!("Scroll for {} more steps", t.steps.len() - fit)));
                    }
                    if !latest.is_empty() {
                        right = right
                            .child(sec("Latest output"))
                            .children(latest.iter().map(|r| one_line(r)));
                    }
                }
                TaskSide::Sessions => {
                    right =
                        right.child(sec(format!("Sessions · {}", t.sessions.len())).mt(px(16.)));
                    let fit = ((((body_h(art) - 150.) / 78.).floor()) as usize).clamp(1, 8);
                    for (k, s) in t.sessions.iter().enumerate().take(fit) {
                        let id = s.session.clone();
                        right = right.child(
                            row(
                                format!("lt-s-{k}"),
                                &format!("{:02}", k + 1),
                                PK,
                                &s.title,
                                &s.role,
                                "",
                                MUTED,
                                false,
                            )
                            .on_click(cx.listener(
                                move |_, _, _, cx| cx.emit(BridgeEvent::OpenSession(id.clone())),
                            )),
                        );
                    }
                    if t.sessions.len() > fit {
                        right =
                            right.child(th(format!("+{} more sessions", t.sessions.len() - fit)));
                    }
                }
            }
        } else {
            right = right.child(big("No task here"));
        }
        let mut right_tabs: Vec<AnyElement> = Vec::new();
        for (s, label) in [(TaskSide::Plan, "Plan"), (TaskSide::Sessions, "Sessions")] {
            right_tabs.push(
                tab(format!("rt-tab-{label}"), label, PK, self.side == s)
                    .h(px(40.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.side = s;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        if let Some((t, a)) = cur
            .as_ref()
            .and_then(|t| TaskAct::of(t.column).map(|a| (t, a)))
        {
            let id = t.id.clone();
            right_tabs.push(
                tab("rt-act", &format!("{} ›", a.label()), GD, false)
                    .h(px(40.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        task_act(&this.model, &id, a, Self::reload, cx);
                    }))
                    .into_any_element(),
            );
        }
        right_tabs.push(
            tab("rt-open", "Open session ›", OR, false)
                .h(px(40.))
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(id) = &open {
                        cx.emit(BridgeEvent::OpenSession(id.clone()));
                    }
                }))
                .into_any_element(),
        );
        island(
            art,
            Island {
                stats,
                left_title: "Tasks",
                left_color: OR,
                left_tabs,
                left: left.into_any_element(),
                right_title: "Task".to_string(),
                right_tabs,
                right: right.into_any_element(),
            },
        )
    }
}

// =============================================================================== Repos ====

pub struct BridgeRepos {
    model: Entity<Model>,
    selected: Option<String>,
    /// 0 all codebases, 1 with a live session, 2 with a session that waits.
    filter: u8,
    _obs: gpui::Subscription,
}

impl EventEmitter<BridgeEvent> for BridgeRepos {}

impl BridgeRepos {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            selected: None,
            filter: 0,
            _obs: obs,
        }
    }
}

impl Render for BridgeRepos {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let art = Art::of(window);
        let Some(war) = self.model.read(cx).war.clone() else {
            return island(
                art,
                Island {
                    stats: Vec::new(),
                    left_title: "Repos",
                    left_color: PK,
                    left_tabs: Vec::new(),
                    left: big("No data yet").into_any_element(),
                    right_title: "Codebase".into(),
                    right_tabs: Vec::new(),
                    right: div().into_any_element(),
                },
            );
        };
        let live = |c: &str| {
            war.sessions
                .iter()
                .filter(|s| s.codebase == c && s.state.is_live())
                .count()
        };
        let waits = |c: &str| {
            war.sessions
                .iter()
                .filter(|s| s.codebase == c && s.ask.is_some())
                .count()
        };
        let kept = |f: u8, c: &str| match f {
            1 => live(c) > 0,
            2 => waits(c) > 0,
            _ => true,
        };
        let list: Vec<_> = war
            .codebases
            .iter()
            .filter(|c| kept(self.filter, &c.id))
            .collect();
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| list.iter().copied().find(|c| &c.id == id))
            .or(list.first().copied())
            .cloned();
        let mut left_tabs: Vec<AnyElement> = Vec::new();
        for (f, label, color) in [(0u8, "All", PK), (1, "Live", OR), (2, "Waiting", RS)] {
            let n = war.codebases.iter().filter(|c| kept(f, &c.id)).count();
            left_tabs.push(
                tab(
                    format!("lr-tab-{label}"),
                    &format!("{label} {n:02}"),
                    color,
                    self.filter == f,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.filter = f;
                    this.selected = None;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        let stats = vec![
            (
                "Codebases".to_string(),
                war.codebases.len().to_string(),
                TXT,
            ),
            (
                "Live sessions".to_string(),
                war.counts().live.to_string(),
                OR,
            ),
            (
                "Waiting".to_string(),
                war.counts().needs_you.to_string(),
                RS,
            ),
            (
                "Worktrees".to_string(),
                war.codebases
                    .iter()
                    .map(|c| c.worktrees)
                    .sum::<u32>()
                    .to_string(),
                TXT,
            ),
        ];
        let mut left = div()
            .flex()
            .flex_col()
            .child(sec("Codebases · click to show").mb(px(12.)));
        // Whole rows only (60 px each); a "+N more" line (24 px) when they do not all fit.
        let left_h = body_h(art) - 40.;
        const ROW_H: f32 = 48.;
        let n = list.len();
        let shown = if (n as f32) * ROW_H <= left_h {
            n
        } else {
            (((left_h - 64.) / ROW_H).floor() as usize).max(1)
        };
        // The window follows the selected codebase.
        let cur_i = cur
            .as_ref()
            .and_then(|c| list.iter().position(|x| x.id == c.id))
            .unwrap_or(0);
        let start = cur_i
            .saturating_sub(shown.saturating_sub(1))
            .min(n.saturating_sub(shown));
        if start > 0 {
            let prev = list[start - 1].id.clone();
            left = left.child(more_row("lr-earlier", format!("{start} earlier")).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.selected = Some(prev.clone());
                    cx.notify();
                }),
            ));
        }
        for (k, c) in list.iter().copied().enumerate().skip(start).take(shown) {
            let id = c.id.clone();
            let w = waits(&c.id);
            left = left.child(
                row(
                    format!("lr-row-{k}"),
                    &format!("{:02}", k + 1),
                    if w > 0 { RS } else { PK },
                    &c.id,
                    "",
                    &format!("{} · {} live", c.branch, live(&c.id)),
                    if w > 0 { RS } else { MUTED },
                    cur.as_ref().is_some_and(|x| x.id == c.id),
                )
                .min_h(px(ROW_H))
                .py(px(4.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        if n > start + shown {
            let next = list[start + shown].id.clone();
            left = left.child(
                more_row("lr-more", format!("+{} more", n - start - shown)).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.selected = Some(next.clone());
                        cx.notify();
                    },
                )),
            );
        }
        let mut right = div().flex().flex_col().gap(px(14.));
        let mut open: Option<String> = None;
        if let Some(c) = &cur {
            open = war
                .sessions
                .iter()
                .find(|s| s.codebase == c.id && s.state.is_live())
                .map(|s| s.id.clone());
            right = right
                .child(big(&c.id))
                .child(th(format!(
                    "{} · {} worktrees · {}",
                    c.branch, c.worktrees, c.daemon
                )))
                .child(sec(format!("Parts · {}", c.parts.len())).mt(px(12.)));
            for p in c.parts.iter().take(1) {
                right = right.child(
                    prose(&format!("{} · {}", p.name, p.paths.join(", ")))
                        .truncate()
                        .h(px(LH_BODY)),
                );
            }
            if c.parts.len() > 1 {
                right = right.child(th(format!("+{} more parts", c.parts.len() - 1)));
            }
            let sessions: Vec<&Pane> = war
                .sessions
                .iter()
                .filter(|s| s.codebase == c.id && s.state.is_live())
                .collect();
            right = right.child(sec(format!("Live sessions · {}", sessions.len())).mt(px(12.)));
            // Whole rows only (60 px each), then a "+N more" line.
            let rows = ((((body_h(art) - 248. - 40.) / 60.).floor()) as usize).clamp(1, 6);
            let mut slist = div().id("lr-sessions").flex().flex_col();
            for (k, s) in sessions.iter().enumerate().take(rows) {
                let id = s.id.clone();
                slist =
                    slist.child(
                        row(
                            format!("lr-s-{k}"),
                            &format!("{:02}", k + 1),
                            state_color(s),
                            &s.title,
                            &chat_text(&s.tail, 1)
                                .pop()
                                .unwrap_or_else(|| s.provider.clone()),
                            &s.provider,
                            MUTED,
                            false,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(BridgeEvent::OpenSession(id.clone()))
                        })),
                    );
            }
            right = right.child(slist);
            if sessions.len() > rows {
                right = right.child(th(format!("+{} more sessions", sessions.len() - rows)));
            }
        }
        let right_tabs = vec![
            tab("rr-open", "Open session ›", OR, false)
                .h(px(40.))
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(id) = &open {
                        cx.emit(BridgeEvent::OpenSession(id.clone()));
                    }
                }))
                .into_any_element(),
        ];
        island(
            art,
            Island {
                stats,
                left_title: "Repos",
                left_color: PK,
                left_tabs,
                left: left.into_any_element(),
                right_title: "Codebase".into(),
                right_tabs,
                right: right.into_any_element(),
            },
        )
    }
}

// ============================================================================ Settings ====

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Looks,
    Autopilot,
    Providers,
}

pub struct BridgeSettings {
    model: Entity<Model>,
    section: Section,
    _obs: gpui::Subscription,
}

impl EventEmitter<BridgeEvent> for BridgeSettings {}

impl BridgeSettings {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            section: Section::Looks,
            _obs: obs,
        }
    }
}

impl Render for BridgeSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let art = Art::of(window);
        let war = self.model.read(cx).war.clone();
        let (auto, max, park) = war.as_ref().map_or((false, 0, 0), |w| {
            (
                w.autopilot.on,
                w.autopilot.max_running,
                w.autopilot.park_after_min,
            )
        });
        let stats = vec![
            (
                "Look".to_string(),
                crate::look::name(crate::theme::tk().look).to_string(),
                OR,
            ),
            (
                "Autopilot".to_string(),
                if auto { "on" } else { "off" }.to_string(),
                TXT,
            ),
            ("Running at most".to_string(), max.to_string(), TXT),
            ("Parks after".to_string(), format!("{park} min"), TXT),
        ];
        let mut left_tabs: Vec<AnyElement> = Vec::new();
        for (s, label) in [
            (Section::Looks, "Looks"),
            (Section::Autopilot, "Autopilot"),
            (Section::Providers, "Providers"),
        ] {
            left_tabs.push(
                tab(format!("ls-tab-{label}"), label, PK, self.section == s)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.section = s;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        // "All settings": the full settings screen (providers, devices, machines, notifications).
        left_tabs.push(
            tab("ls-tab-Other", "All settings ›", PK, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(BridgeEvent::AllSettings)))
                .into_any_element(),
        );
        left_tabs.push(
            tab("ls-tab-Pair", "Pair a phone ›", PK, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(BridgeEvent::PairPhone)))
                .into_any_element(),
        );
        let mut left = div().flex().flex_col();
        let mut right = div().flex().flex_col().gap(px(10.));
        match self.section {
            Section::Looks => {
                left = left.child(sec("Looks · click to switch").mb(px(8.)));
                for (k, (look, name)) in crate::look::LOOKS.into_iter().enumerate() {
                    let on = look == crate::theme::tk().look;
                    left = left.child(
                        row(
                            format!("ls-look-{k}"),
                            &format!("{:02}", k + 1),
                            if on { GD } else { PK },
                            name,
                            look_card(look).0,
                            if on { "On" } else { "" },
                            GD,
                            on,
                        )
                        .min_h(px(50.))
                        .py(px(3.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            act(&this.model, Action::SetLook(look), cx);
                        })),
                    );
                }
                let now = crate::theme::tk().look;
                let (about, scale, swatches) = look_card(now);
                // The Bridge scale comes from the real constants.
                let scale = if now == aivory_protocol::Look::Bridge {
                    format!("Antonio 64 · {S_TITLE} · {S_SECTION} · {S_SMALL} · Plex Mono {S_CODE}")
                } else {
                    scale.to_string()
                };
                let mut chips = div().flex().gap(px(8.));
                for c in swatches {
                    chips = chips.child(div().w(px(56.)).h(px(28.)).rounded_l(px(14.)).bg(rgb(c)));
                }
                let bg = if now == aivory_protocol::Look::Bridge {
                    BLACK
                } else {
                    0x101010
                };
                let mut sizes = div().flex().items_end().gap(px(18.));
                for (txt, size) in [("Aa", S_TITLE), ("Aa", S_BODY), ("Aa", S_SMALL)] {
                    sizes = sizes.child(
                        div()
                            .font_family(LABEL)
                            .text_size(px(size))
                            .line_height(px(size + 6.))
                            .text_color(rgb(TXT))
                            .child(format!("{txt} {size}")),
                    );
                }
                right = right
                    .child(big(crate::look::name(now)))
                    .child(look_preview(crate::look::name(now), swatches, bg))
                    .child(prose(about))
                    .child(sec("Type scale"))
                    .child(sizes)
                    .child(code(&scale, TXT))
                    .child(sec("Colors"))
                    .child(chips)
                    .child(
                        div()
                            .font_family(BODY)
                            .text_size(px(S_SMALL))
                            .line_height(px(LH_BODY))
                            .text_color(rgb(MUTED))
                            .child("A look replaces the frame, colors and fonts of every screen."),
                    );
            }
            Section::Autopilot => {
                left = left.child(sec("Autopilot").mb(px(12.)));
                left = left.child(
                    div()
                        .mt(px(14.))
                        .flex()
                        .gap(px(8.))
                        .child(
                            pill(
                                "ls-auto",
                                if auto { OR } else { PK },
                                if auto {
                                    "Autopilot on"
                                } else {
                                    "Autopilot off"
                                },
                            )
                            .px(px(18.))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    act(
                                        &this.model,
                                        Action::Autopilot(AutopilotPatch {
                                            on: Some(!auto),
                                            ..AutopilotPatch::default()
                                        }),
                                        cx,
                                    );
                                },
                            )),
                        )
                        .child(
                            pill("ls-less", PK, "Fewer")
                                .px(px(18.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    act(
                                        &this.model,
                                        Action::Autopilot(AutopilotPatch {
                                            max_running: Some(max.saturating_sub(1).max(1)),
                                            ..AutopilotPatch::default()
                                        }),
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            pill("ls-more", PK, "More")
                                .px(px(18.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    act(
                                        &this.model,
                                        Action::Autopilot(AutopilotPatch {
                                            max_running: Some((max + 1).min(64)),
                                            ..AutopilotPatch::default()
                                        }),
                                        cx,
                                    );
                                })),
                        ),
                );
                right = right
                    .child(big(&format!("Up to {max} sessions run at once")))
                    .child(prose(&format!(
                        "Autopilot is {}. A session parks after {park} minutes without work.",
                        if auto { "on" } else { "off" }
                    )));
            }
            Section::Providers => {
                left = left.child(sec("Usage windows").mb(px(12.)));
                if let Some(w) = &war {
                    for (k, u) in w.usage.iter().enumerate() {
                        left = left.child(row(
                            format!("ls-use-{k}"),
                            &format!("{:02}", k + 1),
                            if u.pct >= 80 { RS } else { PK },
                            &format!("{} {}", u.provider, u.label),
                            if u.estimate {
                                "estimate"
                            } else {
                                "from the provider"
                            },
                            &format!("{}%", u.pct),
                            if u.pct >= 80 { RS } else { TXT },
                            false,
                        ));
                    }
                }
                right = right.child(big("Providers and models")).child(prose(
                    "Models, effort and permission mode per provider are set under All settings.",
                ));
            }
        }
        let right_tabs: Vec<AnyElement> = Vec::new();
        island(
            art,
            Island {
                stats,
                left_title: "Settings",
                left_color: PK,
                left_tabs,
                left: left.into_any_element(),
                right_title: "Detail".into(),
                right_tabs,
                right: right.into_any_element(),
            },
        )
    }
}
