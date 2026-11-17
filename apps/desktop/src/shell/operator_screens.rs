//! The Operator look of the screens Inbox, Tasks, Repos and Settings, and the frame of Focus,
//! Lens and the full Settings. The grammar is the operator console: a title line with the
//! F-key, blue screens for information and decisions, green screens for agents, monospace
//! text. Every screen has a menu (blue, left), a list and a detail screen. The menu is dim
//! until the pointer comes (one focus: the detail). Every control calls the daemon through
//! `actions.rs`, exactly as the Bridge screens do.

use std::rc::Rc;
use std::time::Duration;

use aivory_protocol::rpc::{
    AutopilotPatch, InboxListParams, SettingsSetParams, TaskListParams, method,
};
use aivory_protocol::{
    Effort, InboxFilter, InboxItem, InboxKind, InboxList, LineKind, Mode, ProviderPrefs,
    SessionState, SettingsView, StepState, Task, TaskColumn, TaskList,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, Stateful, Task as GpuiTask, Window, div, px,
    relative, rgb,
};

use crate::actions::{Action, act};
use crate::bridge_screens::{TaskAct, task_act};
use crate::model::Model;
use crate::ui;
use crate::warroom::skins::operator::kit::*;

/// The gap between the screens.
const GAP: f32 = 20.;
/// The inner padding of a screen.
const PAD: f32 = 24.;
/// The soft red of an error. Color is for state only.
const ERR: u32 = 0xFF8A80;

/// What a screen asks the shell to do.
#[derive(Debug, Clone)]
pub enum OperatorEvent {
    /// Open the pairing card of the full settings screen.
    PairPhone,
    OpenSession(String),
    /// The full Settings screen.
    AllSettings,
}

/// The Operator screens, made when they first show, then kept.
#[derive(Default)]
pub(super) struct OperatorScreens {
    pub inbox: Option<(Entity<OperatorInbox>, gpui::Subscription)>,
    pub tasks: Option<(Entity<OperatorTasks>, gpui::Subscription)>,
    pub repos: Option<(Entity<OperatorRepos>, gpui::Subscription)>,
    pub settings: Option<(Entity<OperatorSettings>, gpui::Subscription)>,
    pub focus: Option<(
        Entity<super::operator_focus::OperatorFocus>,
        gpui::Subscription,
    )>,
    pub lens: Option<(
        Entity<super::operator_lens::OperatorLens>,
        gpui::Subscription,
    )>,
    /// The codebase, file and session that the Lens shows next.
    pub lens_target: Option<(String, String, Option<String>)>,
}

// ---- parts --------------------------------------------------------------------------------

/// The title line of a screen: the F-key and the title on the left, facts on the right.
pub(super) fn head_line(fkey: &str, title: &str, stats: Vec<(String, String)>) -> Div {
    let line = div()
        .flex_none()
        .h(px(68.))
        .flex()
        .items_center()
        .gap(px(28.))
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_size(ts(T_TITLE))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(GREEN_HI))
                .child(format!("{fkey} // {}", title.to_uppercase())),
        )
        .child(div().flex_1());
    // The facts wrap into two lines when the window is narrow; none is cut at the edge.
    let mut facts = div()
        .min_w(px(0.))
        .flex()
        .flex_wrap()
        .justify_end()
        .gap_x(px(20.))
        .gap_y(px(2.));
    for (k, v) in stats {
        facts = facts.child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child(k.to_uppercase()),
                )
                .child(
                    div()
                        .text_size(ts(T_ENTRY))
                        .text_color(rgb(BLUE_HI))
                        .child(v),
                ),
        );
    }
    line.child(facts)
}

/// The layout of a screen: the title line over a row of screens.
pub(super) fn page(head: Div, columns: Vec<AnyElement>) -> Div {
    page_h(head, columns, None)
}

/// A page whose columns use the full height of the window.
pub(super) fn page_full(head: Div, columns: Vec<AnyElement>) -> Div {
    page_h(head, columns, None)
}

fn page_h(head: Div, columns: Vec<AnyElement>, cap: Option<f32>) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .relative()
        .px(px(28.))
        .pb(px(14.))
        .bg(rgb(PAGE))
        .child(room(0.5))
        .font_family(FONT)
        .text_size(ts(T_ENTRY))
        .text_color(rgb(GREEN))
        .child(head)
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .when_some(cap, |d, c| d.max_h(px(c)))
                .flex()
                .gap(px(GAP))
                .children(columns),
        )
}

/// A screen with a section title and a scrolling body. Blue = information, green = agent.
pub(super) fn mon(
    id: &'static str,
    blue: bool,
    title: &str,
    badge: &str,
    foot_text: &str,
    body: Div,
) -> Div {
    let inner = div()
        .id(id)
        .absolute()
        .inset_0()
        .px(px(PAD))
        .py(px(PAD - 6.))
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .text_size(ts(T_ENTRY))
        .line_height(relative(1.45))
        .child(
            div()
                .flex_none()
                .pb(px(12.))
                .text_size(ts(T_SECTION))
                .text_color(rgb(if blue { BLUE_HI } else { GREEN_HI }))
                .child(title.to_uppercase()),
        )
        .child(body);
    bezel(if blue { BLUE_RING } else { GREEN_MID })
        .min_h(px(0.))
        .child(screen(blue, inner))
        .child(foot(
            badge,
            if blue { BLUE } else { GREEN },
            vec![(foot_text.to_string(), FOOT)],
            30.,
        ))
}

/// A menu screen: dim until the pointer comes.
pub(super) fn menu_col(id: &'static str, inner: Div) -> AnyElement {
    div()
        .id(id)
        .flex_none()
        .w(px(270.))
        .flex()
        .flex_col()
        .opacity(0.72)
        .hover(|s| s.opacity(1.))
        .child(inner.flex_1())
        .into_any_element()
}

/// An entry of a menu: an arrow, the name, the count.
pub(super) fn menu_item(id: String, text: &str, count: Option<usize>, on: bool) -> Stateful<Div> {
    div()
        .id(eid(id))
        .flex_none()
        .h(px(40.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(10.))
        .cursor_pointer()
        .when(on, |d| {
            d.bg(rgb(BLUE))
                .text_color(rgb(BLUE_INK))
                .font_weight(gpui::FontWeight::SEMIBOLD)
        })
        .when(!on, |d| d.hover(|h| h.bg(rgb(BLUE_DIM))))
        .child(if on { "▶" } else { " " })
        .child(div().flex_1().child(text.to_uppercase()))
        .children(count.map(|n| format!("{n:02}")))
}

/// A row of a list: number, title, a sub line, and a word on the right.
pub(super) fn list_row(
    id: String,
    no: usize,
    title: &str,
    sub: &str,
    right: &str,
    color: u32,
    on: bool,
) -> Stateful<Div> {
    // A long description wraps to two lines. A short one and a path stay on one line.
    let wrap = sub.len() > 48 && sub.contains(' ');
    div()
        .id(eid(id))
        .flex_none()
        .min_h(px(52.))
        .py(px(4.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(12.))
        .cursor_pointer()
        .when(on, |d| d.bg(rgb(SELECT)))
        .when(!on, |d| d.hover(|h| h.bg(hca(BLUE_DIM, 150))))
        .child(
            div()
                .flex_none()
                .w(px(24.))
                .text_color(rgb(if on { BLUE_INK } else { color }))
                .child(format!("{no:02}")),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(rgb(if on { BLUE_INK } else { BLUE_HI }))
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .when(!wrap, |d| d.whitespace_nowrap().text_ellipsis())
                        .when(wrap, |d| d.max_h(px(30.)))
                        .text_size(ts(T_LABEL))
                        .text_color(rgb(if on { 0x1D3A50 } else { BLUE_SOFT }))
                        .child(sub.to_string()),
                ),
        )
        .child(if right.ends_with('›') {
            // An action word: it looks like a button and lights on hover of the row.
            div()
                .flex_none()
                .h(px(26.))
                .px(px(10.))
                .flex()
                .items_center()
                .text_size(ts(T_LABEL))
                .text_color(rgb(BLUE_HI))
                .bg(hca(BLUE_RING, 70))
                .child(right.to_uppercase())
        } else {
            div()
                .flex_none()
                .text_size(ts(T_LABEL))
                .text_color(rgb(if on { BLUE_INK } else { color }))
                .child(right.to_string())
        })
}

/// The lit row of a list and the lit key tab: reverse video, like the operator menu.
const SELECT: u32 = 0xA9D8F2;

/// A filter as a key of the console: "[A] All 6". The lit one is reverse video.
pub(super) fn tab_key(id: String, label: &str, count: usize, on: bool) -> Stateful<Div> {
    // Each key is unique on its screen: Review takes V, as R is Running or Reviews.
    let key = match label {
        "Review" => 'V',
        _ => label.chars().next().unwrap_or(' ').to_ascii_uppercase(),
    };
    div()
        .id(eid(id))
        .flex_none()
        .h(px(30.))
        .px(px(12.))
        .flex()
        .items_center()
        .cursor_pointer()
        .text_size(ts(T_ENTRY))
        .when(on, |d| d.bg(rgb(SELECT)).text_color(rgb(BLUE_INK)))
        .when(!on, |d| {
            d.border_1()
                .border_color(hca(BLUE, 90))
                .text_color(rgb(BLUE_HI))
                .hover(|h| h.bg(hca(BLUE_DIM, 150)))
        })
        .child(format!("[{key}] {label} {count}"))
}

/// The column names over a table of the console.
pub(super) fn table_head(cols: &[(&str, f32)]) -> Div {
    let mut row = div()
        .flex_none()
        .flex()
        .px(px(10.))
        .pb(px(6.))
        .mb(px(4.))
        .border_b_1()
        .border_color(hca(BLUE, 76))
        .text_size(ts(T_LABEL))
        .text_color(rgb(BLUE_SOFT));
    for (name, w) in cols {
        row = if *w > 0. {
            row.child(div().flex_none().w(px(*w)).child(name.to_string()))
        } else {
            row.child(div().flex_1().child(name.to_string()))
        };
    }
    row
}

/// A button: an operator command. `strong` = lit.
pub(super) fn btn(id: String, text: &str, strong: bool) -> Stateful<Div> {
    div()
        .id(eid(id))
        .flex_none()
        .h(px(34.))
        .px(px(16.))
        .flex()
        .items_center()
        .cursor_pointer()
        .text_size(ts(T_ENTRY))
        .when(strong, |d| {
            d.bg(rgb(BLUE))
                .text_color(rgb(BLUE_INK))
                .font_weight(gpui::FontWeight::SEMIBOLD)
        })
        .when(!strong, |d| {
            d.bg(hca(BLUE_RING, 70))
                .text_color(rgb(BLUE_HI))
                .hover(|h| h.bg(rgb(BLUE)).text_color(rgb(BLUE_INK)))
        })
        .child(text.to_uppercase())
}

/// A line of body text.
pub(super) fn prose(text: &str) -> Div {
    div().text_color(rgb(BLUE)).child(text.to_string())
}

/// A small caption in the body.
pub(super) fn caption(text: &str) -> Div {
    div()
        .flex_none()
        .mt(px(10.))
        .text_size(ts(T_LABEL))
        .text_color(rgb(BLUE_SOFT))
        .child(text.to_uppercase())
}

/// A block of code or a path.
pub(super) fn code(text: &str) -> Div {
    div()
        .px(px(12.))
        .py(px(8.))
        .border_l_2()
        .border_color(rgb(BLUE_RING))
        .bg(rgb(0x021024))
        .text_color(rgb(BLUE_HI))
        .child(text.to_string())
}

/// The lamp color of a session state.
fn state_color(state: SessionState) -> u32 {
    match state {
        SessionState::Working => GREEN,
        SessionState::NeedsYou => BLUE_HI,
        SessionState::Error => ERR,
        _ => BLUE_SOFT,
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

fn minutes(now: i64, since: i64) -> String {
    let m = ((now - since) / 60_000).max(0);
    if m < 1 {
        "now".to_string()
    } else {
        format!("{m} min")
    }
}

/// The last prose rows of a transcript as `You: text` lines.
fn chat_text(tail: &[aivory_protocol::Line], n: usize) -> Vec<String> {
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

// ================================================================================= Inbox ====

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

pub struct OperatorInbox {
    model: Entity<Model>,
    data: Option<Rc<InboxList>>,
    tab: InboxTab,
    side: Side,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
    _obs: gpui::Subscription,
}

impl EventEmitter<OperatorEvent> for OperatorInbox {}

impl OperatorInbox {
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

    /// The items of the tab.
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
        InboxKind::Error => ERR,
        _ => BLUE_HI,
    }
}

impl Render for OperatorInbox {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let items = self.items();
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| items.iter().find(|i| &i.id == id))
            .or(items.first())
            .cloned();
        let count = |f: InboxFilter| {
            self.data
                .as_ref()
                .map_or(0, |d| d.items.iter().filter(|i| f.matches(i.kind)).count())
        };
        let all = self.data.as_ref().map_or(0, |d| d.items.len());
        let done = self.data.as_ref().map_or(0, |d| d.done.len());
        let oldest = self
            .data
            .as_ref()
            .and_then(|d| d.items.iter().map(|i| i.since).min())
            .map_or("none".to_string(), |s| minutes(now, s));
        let head = head_line(
            "F2",
            "Inbox",
            vec![
                ("waiting".into(), count(InboxFilter::Answers).to_string()),
                ("errors".into(), count(InboxFilter::Errors).to_string()),
                ("oldest".into(), oldest),
                ("done today".into(), done.to_string()),
            ],
        );

        // The filters: key tabs at the top of the console.
        let mut tabs = div().flex_none().flex().flex_wrap().gap(px(6.)).pb(px(14.));
        for (t, label, n) in [
            (InboxTab::All, "All", all),
            (InboxTab::Waiting, "Waiting", count(InboxFilter::Answers)),
            (InboxTab::Errors, "Errors", count(InboxFilter::Errors)),
            (InboxTab::Reviews, "Reviews", count(InboxFilter::Reviews)),
            (InboxTab::Done, "Done", done),
        ] {
            tabs = tabs.child(
                tab_key(format!("mi-tab-{label}"), label, n, self.tab == t).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.tab = t;
                        this.selected = None;
                        cx.notify();
                    },
                )),
            );
        }

        // The list.
        let mut list = div().flex().flex_col().child(tabs).child(table_head(&[
            ("NO", 36.),
            ("SESSION", 0.),
            ("AGE", 64.),
        ]));
        for (k, it) in items.iter().take(12).enumerate() {
            let id = it.id.clone();
            list = list.child(
                list_row(
                    format!("mi-row-{k}"),
                    k + 1,
                    &it.title,
                    &format!("{} · {}", it.codebase, it.kind.label()),
                    &minutes(now, it.since),
                    kind_color(it.kind),
                    cur.as_ref().is_some_and(|c| c.id == it.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        if items.is_empty() {
            list = list.child(prose("Nothing waits for you."));
        }
        let list = mon(
            "mi-list",
            true,
            &format!("Inbox · {} open · oldest first", items.len()),
            "2",
            "Inbox",
            list,
        )
        .w(px(720.))
        .flex_none()
        .into_any_element();

        // The detail: the decision, its details, and the way to the session.
        let mut body = div().flex().flex_col().gap(px(12.));
        let mut title = "Detail".to_string();
        let open: Option<String>;
        if let Some(it) = &cur {
            title = it.kind.heading().to_string();
            open = Some(it.session.clone());
            let at = items.iter().position(|i| i.id == it.id).unwrap_or(0);
            body = body
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_size(ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child(format!(
                            "{} {:02} of {:02}",
                            if self.tab == InboxTab::Done {
                                "DONE"
                            } else {
                                "DECISION"
                            },
                            at + 1,
                            items.len()
                        ))
                        .child(format!("waits {}", minutes(now, it.since))),
                )
                .child(
                    div()
                        .text_size(ts(T_SECTION))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(BLUE_HI))
                        .line_height(relative(1.4))
                        .child(it.title.clone()),
                )
                .child(
                    div()
                        .text_size(ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child(format!(
                            "{} · {} / {}",
                            it.agents
                                .first()
                                .map_or(String::new(), |a| a.provider.clone()),
                            it.codebase,
                            it.part
                        )),
                );
            if let Some(result) = &it.result {
                body = body.child(prose(&format!("Done · {result}")));
            } else {
                let mut options = div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(caption("Your decision"));
                for (k, o) in it.options.iter().take(5).enumerate() {
                    let (id, opt) = (it.id.clone(), o.clone());
                    options = options.child(
                        btn(format!("mi-opt-{k}"), &format!("{}  {}", k + 1, o), k == 0).on_click(
                            cx.listener(move |this, _, _, cx| {
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
                            }),
                        ),
                    );
                }
                body = body.child(options);
            }
            let mut tabs = div().flex().gap(px(6.)).mt(px(6.));
            for (s, label) in [
                (Side::Details, "Details"),
                (Side::Files, "Files"),
                (Side::Chat, "Chat"),
            ] {
                tabs = tabs.child(
                    btn(format!("mi-side-{label}"), label, self.side == s).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.side = s;
                            cx.notify();
                        },
                    )),
                );
            }
            body = body.child(tabs);
            match self.side {
                Side::Details => {
                    body = body
                        .child(caption("Why it asks"))
                        .child(prose(if it.preview.is_empty() {
                            &it.text
                        } else {
                            &it.preview
                        }))
                        .child(caption("Command or question"))
                        .child(code(&it.text));
                    if !it.steps.is_empty() {
                        body = body.child(caption("Plan")).children(
                            it.steps
                                .iter()
                                .take(6)
                                .enumerate()
                                .map(|(k, s)| prose(&format!("{}. {s}", k + 1))),
                        );
                    }
                }
                Side::Files => {
                    body = body.child(caption(&format!("Files changed · {}", it.files.len())));
                    for f in it.files.iter().take(12) {
                        body = body.child(prose(f));
                    }
                }
                Side::Chat => {
                    body = body.child(caption("Last rows of the chat"));
                    for r in chat_text(&it.tail, 8) {
                        body = body.child(prose(&r));
                    }
                }
            }
            body = body.child(div().mt(px(8.)).child(
                btn("mi-open".into(), "Open session ›", false).on_click(cx.listener(
                    move |_, _, _, cx| {
                        if let Some(id) = &open {
                            cx.emit(OperatorEvent::OpenSession(id.clone()));
                        }
                    },
                )),
            ));
        } else {
            body = body.child(prose("Nothing waits for you."));
        }
        let detail = mon("mi-detail", true, &title, "3", "Decision", body)
            .flex_1()
            .min_h(px(0.));
        // Under the decision: the chat of that session on a green agent screen, with the
        // prompt line that opens the session to answer there.
        let (chat_title, chat_lines, chat_open, prompt) = match &cur {
            Some(it) => (
                format!("Chat · {}", it.title),
                chat_text(&it.tail, 10),
                Some(it.session.clone()),
                format!(
                    "{}@{}:~$",
                    it.agents.first().map_or("agent", |a| a.provider.as_str()),
                    it.codebase
                ),
            ),
            None => (
                "Chat".to_string(),
                Vec::new(),
                None,
                "operator:~$".to_string(),
            ),
        };
        let mut chat = div().flex().flex_col().gap(px(4.));
        for line in &chat_lines {
            chat = chat.child(div().text_color(rgb(GREEN)).child(line.clone()));
        }
        if chat_lines.is_empty() {
            chat = chat.child(div().text_color(rgb(GREEN_MID)).child("No chat yet."));
        }
        chat = chat.child(
            div()
                .id("mi-chat-prompt")
                .mt(px(10.))
                .pt(px(10.))
                .border_t_1()
                .border_color(hca(GREEN, 64))
                .flex()
                .gap(px(8.))
                .cursor_pointer()
                .child(div().text_color(rgb(GREEN_HI)).child(prompt))
                .child(
                    div()
                        .text_color(rgb(GREEN_MID))
                        .hover(|h| h.text_color(rgb(GREEN_HI)))
                        .child("answer the agent in its session ›"),
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(id) = &chat_open {
                        cx.emit(OperatorEvent::OpenSession(id.clone()));
                    }
                })),
        );
        let chat = mon("mi-chat", false, &chat_title, "4", &chat_title, chat)
            .flex_none()
            .h(px(320.));
        let right = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(detail)
            .child(chat)
            .into_any_element();
        page(head, vec![list.into_any_element(), right])
    }
}

// ================================================================================= Tasks ====

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

pub struct OperatorTasks {
    model: Entity<Model>,
    data: Option<Rc<TaskList>>,
    tab: TaskTab,
    side: TaskSide,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
}

impl EventEmitter<OperatorEvent> for OperatorTasks {}

impl OperatorTasks {
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
        StepState::Done => BLUE_SOFT,
        StepState::Working => GREEN,
        StepState::Waiting => BLUE_HI,
        StepState::NotStarted => FOOT,
    }
}

fn step_word(s: StepState) -> &'static str {
    match s {
        StepState::Done => "done",
        StepState::Working => "working",
        StepState::Waiting => "waits for you",
        StepState::NotStarted => "queued",
    }
}

impl Render for OperatorTasks {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (autopilot, queued_n) = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map_or((false, 0), |w| (w.autopilot.on, w.queued));
        let n = |t| self.of(t).len();
        let head = head_line(
            "F3",
            "Tasks",
            vec![
                ("queued".into(), n(TaskTab::Queued).to_string()),
                ("running".into(), n(TaskTab::Running).to_string()),
                ("in review".into(), n(TaskTab::Review).to_string()),
                ("done".into(), n(TaskTab::Done).to_string()),
            ],
        );
        // The filters and the autopilot: keys at the top of the console.
        let mut tabs = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .pb(px(14.));
        for (t, label) in [
            (TaskTab::Queued, "Queued"),
            (TaskTab::Running, "Running"),
            (TaskTab::Review, "Review"),
            (TaskTab::Done, "Done"),
        ] {
            tabs = tabs.child(
                tab_key(format!("mt-tab-{label}"), label, n(t), self.tab == t).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.tab = t;
                        this.selected = None;
                        cx.notify();
                    }),
                ),
            );
        }
        tabs = tabs.child(
            div()
                .id("mt-autopilot")
                .flex_none()
                .h(px(30.))
                .px(px(12.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(ts(T_ENTRY))
                .border_1()
                .border_color(hca(if autopilot { GREEN } else { BLUE }, 120))
                .text_color(rgb(if autopilot { GREEN_HI } else { BLUE_HI }))
                .hover(|h| h.bg(hca(BLUE_DIM, 150)))
                .tooltip(ui::tip(&format!(
                    "{} · {queued_n} queued for autopilot",
                    if autopilot {
                        "Turn autopilot off"
                    } else {
                        "Turn autopilot on"
                    }
                )))
                .on_click(cx.listener(move |this, _, _, cx| {
                    act(
                        &this.model,
                        Action::Autopilot(AutopilotPatch {
                            on: Some(!autopilot),
                            ..AutopilotPatch::default()
                        }),
                        cx,
                    );
                }))
                .child(format!(
                    "[P] Autopilot {}",
                    if autopilot { "on" } else { "off" }
                )),
        );

        let tasks = self.of(self.tab);
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.id == id))
            .or(tasks.first())
            .cloned();
        let mut list = div().flex().flex_col().child(tabs);
        for (k, t) in tasks.iter().take(9).enumerate() {
            let id = t.id.clone();
            let c = if t.waiting_for.is_some() {
                BLUE_HI
            } else {
                GREEN
            };
            list = list.child(
                list_row(
                    format!("mt-row-{k}"),
                    k + 1,
                    &t.title,
                    &format!(
                        "{} · {}",
                        t.agents.first().cloned().unwrap_or_default(),
                        t.codebase
                    ),
                    &format!("step {}", (t.step + 1).min(5)),
                    c,
                    cur.as_ref().is_some_and(|x| x.id == t.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        if tasks.is_empty() {
            list = list.child(prose("No task here."));
        }
        let list = mon(
            "mt-list",
            true,
            &format!(
                "{} · {}",
                ["Queued", "Running", "Review", "Done"][self.tab as usize],
                tasks.len()
            ),
            "2",
            "tasks",
            list,
        )
        .w(px(430.))
        .flex_none()
        .into_any_element();

        let mut body = div().flex().flex_col().gap(px(12.));
        let mut title = "Task".to_string();
        let open: Option<String>;
        if let Some(t) = &cur {
            open = t.sessions.first().map(|s| s.session.clone());
            title = t.title.clone();
            body = body.child(
                div()
                    .text_size(ts(T_LABEL))
                    .text_color(rgb(BLUE_SOFT))
                    .child(format!(
                        "{} · {} / {} · ${:.2}",
                        t.agents.first().cloned().unwrap_or_default(),
                        t.codebase,
                        t.part,
                        t.cost
                    )),
            );
            let mut tabs = div().flex().gap(px(6.));
            for (s, label) in [(TaskSide::Plan, "Plan"), (TaskSide::Sessions, "Sessions")] {
                tabs = tabs.child(
                    btn(format!("mt-side-{label}"), label, self.side == s).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.side = s;
                            cx.notify();
                        },
                    )),
                );
            }
            if let Some(a) = TaskAct::of(t.column) {
                let id = t.id.clone();
                tabs = tabs.child(btn("mt-act".into(), a.label(), false).on_click(cx.listener(
                    move |this, _, _, cx| {
                        task_act(&this.model, &id, a, Self::reload, cx);
                    },
                )));
            }
            body = body.child(tabs);
            match self.side {
                TaskSide::Plan => {
                    body = body.child(caption(&format!("Plan · {} steps", t.steps.len())));
                    for (k, s) in t.steps.iter().enumerate() {
                        body = body.child(
                            div()
                                .flex_none()
                                .h(px(38.))
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .child(div().flex_none().size(px(9.)).bg(rgb(step_color(s.state))))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(24.))
                                        .text_color(rgb(BLUE_SOFT))
                                        .child(format!("{:02}", k + 1)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_color(rgb(BLUE_HI))
                                        .child(s.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(ts(T_LABEL))
                                        .text_color(rgb(step_color(s.state)))
                                        .child(step_word(s.state).to_uppercase()),
                                ),
                        );
                    }
                }
                TaskSide::Sessions => {
                    body = body.child(caption(&format!("Sessions · {}", t.sessions.len())));
                    for (k, s) in t.sessions.iter().enumerate() {
                        let id = s.session.clone();
                        body = body.child(
                            list_row(
                                format!("mt-s-{k}"),
                                k + 1,
                                &s.title,
                                &s.role,
                                "open ›",
                                GREEN,
                                false,
                            )
                            .on_click(cx.listener(
                                move |_, _, _, cx| cx.emit(OperatorEvent::OpenSession(id.clone())),
                            )),
                        );
                    }
                }
            }
            body = body.child(div().mt(px(8.)).child(
                btn("mt-open".into(), "Open session ›", false).on_click(cx.listener(
                    move |_, _, _, cx| {
                        if let Some(id) = &open {
                            cx.emit(OperatorEvent::OpenSession(id.clone()));
                        }
                    },
                )),
            ));
        } else {
            body = body.child(prose("No task here."));
        }
        let detail = mon("mt-detail", true, &title, "3", "plan of the task", body)
            .flex_1()
            .min_w(px(0.))
            .into_any_element();
        page(head, vec![list, detail])
    }
}

// ================================================================================= Repos ====

pub struct OperatorRepos {
    model: Entity<Model>,
    selected: Option<String>,
    _obs: gpui::Subscription,
}

impl EventEmitter<OperatorEvent> for OperatorRepos {}

impl OperatorRepos {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            selected: None,
            _obs: obs,
        }
    }
}

impl Render for OperatorRepos {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(war) = self.model.read(cx).war.clone() else {
            return page(
                head_line("F4", "Repos", Vec::new()),
                vec![
                    mon(
                        "mr-none",
                        true,
                        "Repos",
                        "1",
                        "no data",
                        prose("No data yet."),
                    )
                    .flex_1()
                    .into_any_element(),
                ],
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
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| war.codebase(id))
            .or(war.codebases.first())
            .cloned();
        let head = head_line(
            "F4",
            "Repos",
            vec![
                ("codebases".into(), war.codebases.len().to_string()),
                ("live sessions".into(), war.counts().live.to_string()),
                ("waiting".into(), war.counts().needs_you.to_string()),
                (
                    "worktrees".into(),
                    war.codebases
                        .iter()
                        .map(|c| c.worktrees)
                        .sum::<u32>()
                        .to_string(),
                ),
            ],
        );
        let mut list = div().flex().flex_col();
        for (k, c) in war.codebases.iter().enumerate() {
            let id = c.id.clone();
            let w = waits(&c.id);
            list = list.child(
                list_row(
                    format!("mr-row-{k}"),
                    k + 1,
                    &c.id,
                    &format!("{} · {}", c.branch, c.path),
                    &format!("{} live", live(&c.id)),
                    if w > 0 { BLUE_HI } else { BLUE_SOFT },
                    cur.as_ref().is_some_and(|x| x.id == c.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        let list = mon("mr-list", true, "Codebases", "1", "pick a codebase", list)
            .w(px(470.))
            .flex_none()
            .into_any_element();

        let mut body = div().flex().flex_col().gap(px(12.));
        let mut title = "Codebase".to_string();
        if let Some(c) = &cur {
            title = c.id.clone();
            let sessions: Vec<_> = war
                .sessions
                .iter()
                .filter(|s| s.codebase == c.id && s.state.is_live())
                .collect();
            body = body
                .child(
                    div()
                        .text_size(ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child(format!(
                            "{} · {} worktrees · {}",
                            c.branch, c.worktrees, c.daemon
                        )),
                )
                .child(code(&c.path))
                .child(caption(&format!("Parts · {}", c.parts.len())));
            for p in c.parts.iter().take(6) {
                body = body.child(prose(&format!("{} · {}", p.name, p.paths.join(", "))));
            }
            body = body.child(caption(&format!("Live sessions · {}", sessions.len())));
            for (k, s) in sessions.iter().take(6).enumerate() {
                let id = s.id.clone();
                body = body.child(
                    list_row(
                        format!("mr-s-{k}"),
                        k + 1,
                        &s.title,
                        &s.provider,
                        "open ›",
                        state_color(s.state),
                        false,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(OperatorEvent::OpenSession(id.clone()))
                    })),
                );
            }
            if sessions.is_empty() {
                body = body.child(prose("No live session in this codebase."));
            }
        }
        let detail = mon(
            "mr-detail",
            true,
            &title,
            "2",
            "facts of the codebase",
            body,
        )
        .flex_1()
        .min_w(px(0.))
        .into_any_element();
        page(head, vec![list, detail])
    }
}

// =============================================================================== Settings ====

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Looks,
    Autopilot,
    Providers,
    Models,
}

pub struct OperatorSettings {
    model: Entity<Model>,
    /// The settings and the providers of the daemon, for Models and effort.
    view: Option<SettingsView>,
    section: Section,
    /// The row of the list that the detail shows. The current look for the Looks section.
    sel: Option<usize>,
    _obs: gpui::Subscription,
}

impl EventEmitter<OperatorEvent> for OperatorSettings {}

impl OperatorSettings {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        let mut this = Self {
            model,
            view: None,
            section: Section::Looks,
            sel: None,
            _obs: obs,
        };
        this.refresh(cx);
        this
    }

    /// Read the settings and the providers from the daemon.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let call = self
            .model
            .read(cx)
            .call::<method::SettingsGet>(Default::default(), cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(view) = result {
                    this.view = Some(view);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Save one value of the settings and show what the daemon returns.
    fn save(&mut self, key: String, value: serde_json::Value, cx: &mut Context<Self>) {
        let call = self
            .model
            .read(cx)
            .call::<method::SettingsSet>(SettingsSetParams { key, value }, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(settings) = result
                    && let Some(view) = &mut this.view
                {
                    view.settings = settings;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// What a look is, in one calm sentence.
fn look_blurb(name: &str) -> &'static str {
    match name {
        "Minimal" => "The calm default: a light page, a rail on the left and round cards.",
        "Bridge" => {
            "Island frames on black with numbered frame segments. Informative, never overwhelming."
        }
        "Operator" => {
            "An operator console: blue screens for information, green screens for agents, F-keys in the top line."
        }
        "Neon" => {
            "A full-screen city. A district is a codebase, a tower is a session, neon panels float over it."
        }
        "Navigator" => {
            "A 3D file-system view is the navigation. IRIX-style windows and a toolchest bar."
        }
        "Holo" => "See-through glass panels over slow rings. One decision at a time.",
        "Tactical" => {
            "A radar: the bearing is the codebase, the range is the time since a session moved."
        }
        "Command" => {
            "A command deck: pixel-art rooms, steel panels. One decision at a time over a calm map."
        }
        _ => "A look for the whole app.",
    }
}

impl Render for OperatorSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let war = self.model.read(cx).war.clone();
        let (auto, max, park) = war.as_ref().map_or((false, 0, 0), |w| {
            (
                w.autopilot.on,
                w.autopilot.max_running,
                w.autopilot.park_after_min,
            )
        });
        let head = head_line(
            "F5",
            "Settings",
            vec![
                (
                    "look".into(),
                    crate::look::name(crate::theme::tk().look).to_string(),
                ),
                (
                    "autopilot".into(),
                    if auto { "on" } else { "off" }.to_string(),
                ),
                ("running at most".into(), max.to_string()),
                ("parks after".into(), format!("{park} min")),
            ],
        );
        let mut menu = div().flex().flex_col().gap(px(2.));
        for (s, label) in [
            (Section::Looks, "Looks"),
            (Section::Autopilot, "Autopilot"),
            (Section::Providers, "Providers"),
            (Section::Models, "Models and effort"),
        ] {
            menu = menu.child(
                menu_item(format!("ms-tab-{label}"), label, None, self.section == s).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.section = s;
                        this.sel = None;
                        cx.notify();
                    }),
                ),
            );
        }
        menu = menu.child(
            menu_item("ms-all".into(), "All settings ›", None, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(OperatorEvent::AllSettings))),
        );
        menu = menu.child(
            menu_item("ms-pair".into(), "Pair a phone ›", None, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(OperatorEvent::PairPhone))),
        );
        let menu = menu_col(
            "ms-menu",
            mon(
                "ms-menu-s",
                true,
                "Menu",
                "1",
                "↑ ↓ section · F1-F5 screens",
                menu,
            ),
        );

        let mut list = div().flex().flex_col();
        let mut body = div().flex().flex_col().gap(px(12.));
        let list_title: &str;
        let detail_title: String;
        match self.section {
            Section::Looks => {
                list_title = "Looks";
                let current = crate::look::LOOKS
                    .iter()
                    .position(|(l, _)| *l == crate::theme::tk().look)
                    .unwrap_or(0);
                let sel = self
                    .sel
                    .unwrap_or(current)
                    .min(crate::look::LOOKS.len() - 1);
                for (k, (_, name)) in crate::look::LOOKS.into_iter().enumerate() {
                    list = list.child(
                        list_row(
                            format!("ms-look-{k}"),
                            k + 1,
                            name,
                            look_blurb(name),
                            if k == current { "ON" } else { "" },
                            BLUE_HI,
                            k == sel,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sel = Some(k);
                            cx.notify();
                        })),
                    );
                }
                let (look, name) = crate::look::LOOKS[sel];
                detail_title = name.to_string();
                body = body
                    .child(prose(look_blurb(name)))
                    .child(caption("Status"))
                    .child(prose(if sel == current {
                        "This look is on now."
                    } else {
                        "This look is off. Switching changes the frame, the colors and the fonts of every screen."
                    }))
                    .when(sel != current, |d| {
                        d.child(
                            div().mt(px(8.)).child(
                                btn("ms-switch".into(), &format!("Switch to {name}"), true)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        act(&this.model, Action::SetLook(look), cx);
                                    })),
                            ),
                        )
                    })
                    .child(prose(
                        "The choice is saved in the settings of the daemon.",
                    ))
                    .child(caption("Keys"))
                    .child(prose("F1 war room · F2 inbox · F3 tasks · F4 repos · F5 settings"))
                    .child(prose("Up and Down pick a row. Enter opens it. Escape goes back."));
            }
            Section::Autopilot => {
                list_title = "Autopilot";
                let rows = [
                    (
                        "Autopilot",
                        "starts the queued tasks for you",
                        if auto { "on" } else { "off" }.to_string(),
                    ),
                    (
                        "Running at most",
                        "sessions at the same time",
                        max.to_string(),
                    ),
                    ("Parks after", "minutes without work", format!("{park} min")),
                ];
                let sel = self.sel.unwrap_or(0).min(rows.len() - 1);
                for (k, (t, sub, v)) in rows.iter().enumerate() {
                    list = list.child(
                        list_row(
                            format!("ms-auto-row-{k}"),
                            k + 1,
                            t,
                            sub,
                            v,
                            BLUE_HI,
                            k == sel,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sel = Some(k);
                            cx.notify();
                        })),
                    );
                }
                detail_title = rows[sel].0.to_string();
                let patch = |p: AutopilotPatch| Action::Autopilot(p);
                match sel {
                    0 => {
                        body = body
                            .child(prose(&format!(
                                "Autopilot is {}. When it is on, the daemon starts queued tasks while fewer than {max} sessions run.",
                                if auto { "on" } else { "off" }
                            )))
                            .child(
                                div().child(
                                    btn(
                                        "ms-auto".into(),
                                        if auto { "Turn off" } else { "Turn on" },
                                        !auto,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        act(
                                            &this.model,
                                            patch(AutopilotPatch {
                                                on: Some(!auto),
                                                ..AutopilotPatch::default()
                                            }),
                                            cx,
                                        );
                                    })),
                                ),
                            );
                    }
                    1 => {
                        body = body
                            .child(prose(&format!("Up to {max} sessions run at once.")))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.))
                                    .child(btn("ms-less".into(), "Fewer", false).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            act(
                                                &this.model,
                                                patch(AutopilotPatch {
                                                    max_running: Some(max.saturating_sub(1).max(1)),
                                                    ..AutopilotPatch::default()
                                                }),
                                                cx,
                                            );
                                        }),
                                    ))
                                    .child(btn("ms-more".into(), "More", false).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            act(
                                                &this.model,
                                                patch(AutopilotPatch {
                                                    max_running: Some((max + 1).min(64)),
                                                    ..AutopilotPatch::default()
                                                }),
                                                cx,
                                            );
                                        }),
                                    )),
                            );
                    }
                    _ => {
                        body = body.child(prose(&format!(
                            "A session parks after {park} minutes without work. A parked session frees its slot and can resume later."
                        )));
                    }
                }
            }
            Section::Models => {
                list_title = "Providers";
                let providers = self
                    .view
                    .as_ref()
                    .map(|v| v.providers.clone())
                    .unwrap_or_default();
                let sel = self.sel.unwrap_or(0).min(providers.len().saturating_sub(1));
                let prefs_of = |id: &str, first: Option<&String>| {
                    self.view
                        .as_ref()
                        .and_then(|v| v.settings.providers.get(id).cloned())
                        .unwrap_or_else(|| ProviderPrefs {
                            enabled: false,
                            model: first.cloned().unwrap_or_default(),
                            effort: Effort::High,
                            mode: Mode::Edit,
                        })
                };
                for (k, p) in providers.iter().enumerate() {
                    let prefs = prefs_of(&p.id, p.models.first());
                    list = list.child(
                        list_row(
                            format!("ms-prov-{k}"),
                            k + 1,
                            &p.name,
                            &format!("{} · {:?}", prefs.model, prefs.effort).to_lowercase(),
                            if prefs.enabled { "ON" } else { "OFF" },
                            if prefs.enabled { GREEN } else { BLUE_SOFT },
                            k == sel,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sel = Some(k);
                            cx.notify();
                        })),
                    );
                }
                match providers.get(sel) {
                    Some(p) => {
                        detail_title = p.name.clone();
                        let prefs = prefs_of(&p.id, p.models.first());
                        let mut models = p.models.clone();
                        if !models.contains(&prefs.model) {
                            models.insert(0, prefs.model.clone());
                        }
                        // One group of choices: a caption, then a button per value.
                        let group = |title: &str,
                                         id: String,
                                         key: String,
                                         choices: Vec<(String, serde_json::Value, bool)>| {
                            let mut row = div().flex().flex_wrap().gap(px(8.));
                            for (i, (label, value, on)) in choices.into_iter().enumerate() {
                                let key = key.clone();
                                row = row.child(
                                    btn(format!("{id}-{i}"), &label, on).on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            if !on {
                                                this.save(key.clone(), value.clone(), cx);
                                            }
                                        },
                                    )),
                                );
                            }
                            div()
                                .flex()
                                .flex_col()
                                .child(caption(title))
                                .child(div().mt(px(6.)).child(row))
                        };
                        let pid = p.id.clone();
                        let key = |field: &str| format!("providers.{pid}.{field}");
                        body = body
                            .child(prose(&p.auth))
                            .child(group(
                                "Use this provider",
                                format!("ms-pv-{pid}-on"),
                                key("enabled"),
                                vec![
                                    ("On".into(), serde_json::json!(true), prefs.enabled),
                                    ("Off".into(), serde_json::json!(false), !prefs.enabled),
                                ],
                            ))
                            .child(group(
                                "Default model",
                                format!("ms-pv-{pid}-model"),
                                key("model"),
                                models
                                    .iter()
                                    .map(|m| (m.clone(), serde_json::json!(m), *m == prefs.model))
                                    .collect(),
                            ))
                            .child(group(
                                "Effort",
                                format!("ms-pv-{pid}-effort"),
                                key("effort"),
                                [
                                    (Effort::Low, "Low"),
                                    (Effort::Medium, "Medium"),
                                    (Effort::High, "High"),
                                    (Effort::Max, "Max"),
                                ]
                                .into_iter()
                                .map(|(e, l)| {
                                    (l.to_string(), serde_json::json!(e), e == prefs.effort)
                                })
                                .collect(),
                            ))
                            .child(group(
                                "Permission mode",
                                format!("ms-pv-{pid}-mode"),
                                key("mode"),
                                [
                                    (Mode::Plan, "Plan"),
                                    (Mode::Edit, "Edit"),
                                    (Mode::Auto, "Auto"),
                                ]
                                .into_iter()
                                .map(|(m, l)| {
                                    (l.to_string(), serde_json::json!(m), m == prefs.mode)
                                })
                                .collect(),
                            ))
                            .child(
                                div()
                                    .mt(px(10.))
                                    .child(prose("A session can change these in its input box.")),
                            );
                    }
                    None => {
                        detail_title = "Providers".to_string();
                        list = list.child(prose("No provider data yet."));
                    }
                }
            }
            Section::Providers => {
                list_title = "Usage windows";
                let usage = war.as_ref().map(|w| w.usage.clone()).unwrap_or_default();
                let sel = self.sel.unwrap_or(0).min(usage.len().saturating_sub(1));
                for (k, u) in usage.iter().enumerate() {
                    list = list.child(
                        list_row(
                            format!("ms-use-{k}"),
                            k + 1,
                            &format!("{} {}", u.provider, u.label),
                            if u.estimate {
                                "estimate"
                            } else {
                                "from the provider"
                            },
                            &format!("{}%", u.pct),
                            if u.pct >= 80 { ERR } else { BLUE_HI },
                            k == sel,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sel = Some(k);
                            cx.notify();
                        })),
                    );
                }
                match usage.get(sel) {
                    Some(u) => {
                        detail_title = format!("{} {}", u.provider, u.label);
                        let now = war.as_ref().map_or(0, |w| w.now);
                        let reset = u.resets_at.map_or("unknown".to_string(), |r| {
                            let m = (r - now).max(0) / 60_000;
                            format!("{}h {:02}m", m / 60, m % 60)
                        });
                        let hot = u.pct >= 80;
                        body = body
                            .child(
                                div()
                                    .text_size(ts(T_TITLE))
                                    .text_color(rgb(if hot { ERR } else { BLUE_HI }))
                                    .child(format!("{}%", u.pct)),
                            )
                            .child(
                                div().h(px(10.)).bg(rgb(BLUE_DIM)).child(
                                    div()
                                        .h(px(10.))
                                        .w(relative(f32::from(u.pct.min(100)) / 100.))
                                        .bg(rgb(if hot { ERR } else { BLUE })),
                                ),
                            )
                            .child(prose(&format!("Frees up in {reset}.")))
                            .child(prose(if u.estimate {
                                "The value is an estimate from local transcripts."
                            } else {
                                "The value comes from the provider."
                            }))
                            .child(prose(
                                "Models, effort and permission mode per provider are in Models and effort.",
                            ));
                    }
                    None => {
                        detail_title = "Usage".to_string();
                        list = list.child(prose("No usage data yet."));
                    }
                }
            }
        }
        let list = mon("ms-list", true, list_title, "2", "↑ ↓ row · ↵ open", list)
            .w(px(470.))
            .flex_none()
            .into_any_element();
        let detail = mon(
            "ms-detail",
            true,
            &detail_title,
            "3",
            "↵ switch · Esc back",
            body,
        )
        .flex_1()
        .min_w(px(0.))
        .into_any_element();
        page(head, vec![menu, list, detail])
    }
}
