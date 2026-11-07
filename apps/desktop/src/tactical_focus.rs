//! The Tactical Focus screen: one contact in full. The menu holds the contact tabs. The left
//! panel is the intercept log (parsed by the console display, no raw markdown), the order
//! request, and the composer with its mode and effort. The right panel lists the changes of the
//! session: the files and the diff of the picked one. Every control goes through `act`.

use std::time::{Duration, Instant};

use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::session::{DiffFile, DiffLineKind, SessionDiff};
use aivory_protocol::{AskKind, ComposerPrefs, Effort, Mode, Pane};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, ScrollHandle, Subscription, Window, div,
    px,
};
use gpui_base::input::{InputEvent, InputState};

use crate::actions::{Action, act};
use crate::console::display::{DKind, last};
use crate::model::Model;
use crate::tactical_kit::{
    self as tac, Btn, Frame, c, dots, entry, frame, label, navb, section, tbox, trow,
};
use crate::ui;

/// How old a diff gets before the view asks again.
const TTL: Duration = Duration::from_secs(8);

#[derive(Debug, Clone)]
pub enum TacFocusEvent {
    /// Back to the war room.
    Back,
    /// Show another contact.
    Open(String),
    /// The dossier of the contact.
    Dossier(String),
    /// Open a file of the contact in the Lens.
    OpenFile { session: String, path: String },
}

impl EventEmitter<TacFocusEvent> for TacticalFocus {}

pub struct TacticalFocus {
    model: Entity<Model>,
    id: String,
    input: Entity<InputState>,
    diff: Option<(Option<SessionDiff>, Instant)>,
    busy: bool,
    /// The path of the file that the diff shows. The first file when `None`.
    file: Option<String>,
    /// The mode and the effort that the next message carries; the pane's own when `None`.
    prefs: Option<ComposerPrefs>,
    scroll: ScrollHandle,
    scrolled: String,
    _subs: Vec<Subscription>,
}

impl TacticalFocus {
    pub fn new(
        model: Entity<Model>,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("a note for the next turn"));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.send(window, cx);
            }
        });
        Self {
            model,
            id,
            input,
            diff: None,
            busy: false,
            file: None,
            prefs: None,
            scroll: ScrollHandle::new(),
            scrolled: String::new(),
            _subs: vec![observe, events],
        }
    }

    /// Show another contact.
    pub fn show(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.id != id {
            self.id = id.to_string();
            self.diff = None;
            self.file = None;
            self.prefs = None;
            self.scrolled.clear();
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
        let text = self.input.read(cx).value().trim().to_string();
        let Some(pane) = self.pane(cx) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        let action = match &pane.ask {
            Some(ask) => Action::Answer {
                id: pane.id.clone(),
                ask: ask.id.clone(),
                option: None,
                text: Some(text),
            },
            _ => Action::Send {
                id: pane.id.clone(),
                text,
                composer: self.prefs.clone().filter(|p| *p != pane.composer),
            },
        };
        act(&self.model, action, cx);
        self.input.update(cx, |s, cx| s.clean(window, cx));
    }

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

fn next_mode(m: Mode) -> Mode {
    match m {
        Mode::Plan => Mode::Edit,
        Mode::Edit => Mode::Auto,
        Mode::Auto => Mode::Plan,
    }
}

fn next_effort(e: Effort) -> Effort {
    match e {
        Effort::Low => Effort::Medium,
        Effort::Medium => Effort::High,
        Effort::High => Effort::Max,
        Effort::Max => Effort::Low,
    }
}

impl Render for TacticalFocus {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let back = navb("tf-back", "< OPS", false)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(TacFocusEvent::Back)))
            .into_any_element();
        let Some(p) = self.pane(cx) else {
            return frame(Frame {
                title: "FOCUS",
                kicker: "THIS CONTACT IS GONE".into(),
                stats: Vec::new(),
                menu: vec![back],
                body: entry("This session is gone.").into_any_element(),
            });
        };
        self.ensure(cx);
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let waits = p.ask.is_some();
        let call = format!("{}/{}", p.codebase, p.title).to_uppercase();
        let prefs = self.prefs.clone().unwrap_or_else(|| p.composer.clone());

        // ---- the menu: the contact tabs with full names ------------------------------------
        let mut contacts: Vec<(String, String, bool)> = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map(|w| {
                w.sessions
                    .iter()
                    .filter(|s| s.state.is_live())
                    .map(|s| {
                        let name = if s.title.is_empty() { &s.id } else { &s.title };
                        (s.id.clone(), name.to_uppercase(), s.ask.is_some())
                    })
                    .collect()
            })
            .unwrap_or_default();
        contacts.sort_by(|a, b| (!a.2, &a.0).cmp(&(!b.2, &b.0)));
        // Room for the tabs: the window less the page padding and the buttons at the right.
        let room = f32::from(window.viewport_size().width) - 64. - 260.;
        let mut used = 0.;
        let mut shown: Vec<&(String, String, bool)> = Vec::new();
        if let Some(cur) = contacts.iter().find(|x| x.0 == self.id) {
            used += cur.1.chars().count() as f32 * 7.5 + 40.;
            shown.push(cur);
        }
        for x in contacts.iter().filter(|x| x.0 != self.id) {
            let w = x.1.chars().count() as f32 * 7.5 + 40.;
            if used + w + 60. > room {
                break;
            }
            used += w;
            shown.push(x);
        }
        let hidden: Vec<&(String, String, bool)> = contacts
            .iter()
            .filter(|x| !shown.iter().any(|s| s.0 == x.0))
            .collect();
        let mut menu: Vec<AnyElement> = Vec::new();
        for (k, x) in shown.iter().enumerate() {
            let sid = x.0.clone();
            let mut tab = navb(format!("tf-tab-{k}"), &x.1, x.0 == self.id);
            if x.2 && x.0 != self.id {
                tab = tab.text_color(c(tac::WHITE, 1.));
            }
            menu.push(
                tab.on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(TacFocusEvent::Open(sid.clone()));
                }))
                .into_any_element(),
            );
        }
        if let Some(first) = hidden.first() {
            let sid = first.0.clone();
            menu.push(
                navb("tf-more", &format!("+{} >", hidden.len()), false)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(TacFocusEvent::Open(sid.clone()));
                    }))
                    .into_any_element(),
            );
        }
        menu.push(div().flex_1().into_any_element());
        let dossier_id = p.id.clone();
        menu.push(
            navb("tf-dossier", "DOSSIER >", false)
                .border_1()
                .border_color(c(tac::INK, 1.))
                .text_color(c(tac::INK, 1.))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(TacFocusEvent::Dossier(dossier_id.clone()));
                }))
                .into_any_element(),
        );
        menu.push(back);

        // ---- the intercept log -------------------------------------------------------------
        let rows = last(&p.tail, 80);
        let key = format!("{}:{}", p.id, rows.len());
        if self.scrolled != key {
            self.scroll.scroll_to_bottom();
            self.scrolled = key;
        }
        let mut log = div()
            .id("tf-log")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .gap(px(6.))
            .pr(px(12.));
        for d in rows.iter().filter(|d| d.kind != DKind::Meta) {
            let verb = match (d.first, d.kind) {
                (false, _) => String::new(),
                (true, DKind::You) => "ORD".to_string(),
                (true, DKind::Tool) => dots(
                    &d.label
                        .clone()
                        .unwrap_or_else(|| "TOOL".into())
                        .to_uppercase(),
                    6,
                ),
                (true, DKind::Error) => "ERR".to_string(),
                (true, DKind::Output | DKind::Meta) => String::new(),
                (true, _) => "SAY".to_string(),
            };
            let (text, color, bold) = match d.kind {
                DKind::You => (d.text.clone(), tac::INK, false),
                DKind::Tool => (d.text.clone(), tac::TEXT, false),
                DKind::Output => (
                    format!(
                        "\u{2514} {}",
                        dots(d.text.lines().next().unwrap_or(""), 140)
                    ),
                    tac::LABEL,
                    false,
                ),
                DKind::Error => (d.text.clone(), tac::WHITE, false),
                DKind::Heading => (d.text.clone(), tac::INK, true),
                DKind::Code | DKind::Table => (d.text.clone(), tac::MID, false),
                _ => (d.text.clone(), tac::TEXT, false),
            };
            log = log.child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(12.))
                    .text_size(ui::ts(tac::S_LABEL + 1.))
                    .line_height(ui::ts(tac::S_LABEL + 1.) * 1.5)
                    .child(
                        div()
                            .flex_none()
                            .w(px(52.))
                            .text_color(c(tac::WHITE, 0.8))
                            .child(verb),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .when(bold, |d| d.font_weight(FontWeight::BOLD))
                            .text_color(c(color, 1.))
                            .child(text),
                    ),
            );
        }
        if rows.is_empty() {
            log = log.child(label("NO LINES YET"));
        }

        // ---- the order request -------------------------------------------------------------
        let ask_box = p.ask.as_ref().map(|ask| {
            let mut orders = div().flex().flex_wrap().gap(px(8.));
            if p.controlled {
                let n = ask.options.len();
                for (k, option) in ask.options.iter().enumerate() {
                    let (sid, aid, opt) = (p.id.clone(), ask.id.clone(), option.clone());
                    let kind = if k == 0 && !ask.risky {
                        Btn::On
                    } else if k + 1 == n && n > 1 {
                        Btn::Dim
                    } else {
                        Btn::Plain
                    };
                    orders = orders.child(
                        tac::order(
                            format!("tf-ans-{k}"),
                            &format!("[{}] {}", k + 1, dots(&option.to_uppercase(), 18)),
                            kind,
                        )
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
                orders = orders.child(label("ANSWER IT IN ITS TERMINAL"));
            }
            let question = match ask.kind {
                AskKind::Permission => format!("Allow {} to run: {}", p.provider, ask.text),
                AskKind::Plan => format!("Approve the plan: {}", ask.text),
                _ => ask.text.clone(),
            };
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(18.))
                .border_1()
                .border_color(c(tac::WHITE, 1.))
                .bg(c(tac::WHITE, 0.04))
                .child(
                    label(format!(
                        "ORDER REQUESTED · {}",
                        tac::clock_ms(now - p.since)
                    ))
                    .text_color(c(tac::WHITE, 1.)),
                )
                .child(entry(question).text_color(c(tac::WHITE, 1.)))
                .child(orders)
        });

        // ---- the composer ------------------------------------------------------------------
        let (mode_prefs, effort_prefs) = (prefs.clone(), prefs.clone());
        let composer = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .min_h(px(52.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(6.))
                    .border_1()
                    .border_color(c(tac::DIM, 1.))
                    .child(label(if waits { "ANSWER:" } else { "ORDER:" }))
                    .child(div().flex_1().min_w(px(0.)).child(self.input.clone()))
                    .child(
                        tac::order("tf-send", "SEND", Btn::Plain)
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        navb(
                            "tf-mode",
                            &format!("MODE: {}", format!("{:?}", prefs.mode).to_uppercase()),
                            false,
                        )
                        .border_1()
                        .border_color(c(tac::DIM, 1.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut next = mode_prefs.clone();
                            next.mode = next_mode(next.mode);
                            this.prefs = Some(next);
                            cx.notify();
                        })),
                    )
                    .child(
                        navb(
                            "tf-effort",
                            &format!("EFFORT: {}", format!("{:?}", prefs.effort).to_uppercase()),
                            false,
                        )
                        .border_1()
                        .border_color(c(tac::DIM, 1.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut next = effort_prefs.clone();
                            next.effort = next_effort(next.effort);
                            this.prefs = Some(next);
                            cx.notify();
                        })),
                    )
                    .child(label(format!(
                        "{} · {}",
                        prefs.provider.to_uppercase(),
                        dots(&prefs.model.to_uppercase(), 24)
                    ))),
            );

        let mut left = tbox("INTERCEPT LOG")
            .flex_1()
            .min_w(px(0.))
            .p(px(24.))
            .pt(px(28.))
            .gap(px(14.))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(section(call))
                    .child(label(format!(
                        "{} · {} · LINES {} · CONTEXT {}%",
                        p.provider.to_uppercase(),
                        p.model.to_uppercase(),
                        p.lines_total,
                        p.context_pct
                    ))),
            )
            .child(log);
        if let Some(a) = ask_box {
            left = left.child(a);
        }
        let left = left.child(composer);

        // ---- the changes -------------------------------------------------------------------
        let files: Vec<DiffFile> = self
            .diff
            .as_ref()
            .and_then(|(d, _)| d.as_ref())
            .map(|d| d.files.clone())
            .unwrap_or_default();
        let cur: Option<&DiffFile> = self
            .file
            .as_ref()
            .and_then(|f| files.iter().find(|x| &x.path == f))
            .or(files.first());
        let mut list = div()
            .id("tf-files")
            .flex_none()
            .max_h(px(220.))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for (k, f) in files.iter().enumerate() {
            let path = f.path.clone();
            list = list.child(
                trow(
                    format!("tf-file-{k}"),
                    tac::INK,
                    f.path.rsplit(['/', '\\']).next().unwrap_or(&f.path),
                    &f.path,
                    &format!("+{} -{}", f.added, f.removed),
                    tac::LABEL,
                    cur.is_some_and(|x| x.path == f.path),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.file = Some(path.clone());
                    cx.notify();
                })),
            );
        }
        let mut diff_view = div()
            .id("tf-diff")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if let Some(f) = cur {
            for l in f.lines.iter().take(150) {
                let (sign, color) = match l.kind {
                    DiffLineKind::Added => ("+", tac::INK),
                    DiffLineKind::Removed => ("-", tac::LABEL),
                    DiffLineKind::Hunk => ("@", tac::DIM),
                    DiffLineKind::Context => (" ", tac::MID),
                };
                diff_view = diff_view.child(
                    div()
                        .flex_none()
                        .flex()
                        .gap(px(8.))
                        .text_size(ui::ts(tac::S_LABEL))
                        .line_height(ui::ts(tac::S_LABEL) * 1.5)
                        .text_color(c(color, 1.))
                        .child(div().flex_none().w(px(10.)).child(sign))
                        .child(div().flex_1().min_w(px(0.)).child(l.text.clone())),
                );
            }
        }
        let open = cur.map(|f| f.path.clone());
        let sid = p.id.clone();
        let mut right = tbox(format!("CHANGES · {} FILES", files.len()))
            .flex_none()
            .w(px(460.))
            .p(px(24.))
            .pt(px(28.))
            .gap(px(12.));
        if files.is_empty() {
            right = right.child(entry("No file changed yet."));
        } else {
            right = right.child(list).child(diff_view).child(
                tac::order("tf-lens", "OPEN IN LENS >", Btn::Plain).on_click(cx.listener(
                    move |_, _, _, cx| {
                        if let Some(path) = &open {
                            cx.emit(TacFocusEvent::OpenFile {
                                session: sid.clone(),
                                path: path.clone(),
                            });
                        }
                    },
                )),
            );
        }

        frame(Frame {
            title: "FOCUS",
            kicker: format!(
                "{} · ONE CONTACT IN FULL",
                dots(&p.title.to_uppercase(), 60)
            ),
            stats: vec![
                (
                    if waits { "AWAITS ORDERS" } else { "ACTIVE" }.to_string(),
                    "STATE".to_string(),
                    if waits { tac::WHITE } else { tac::INK },
                ),
                (
                    format!("{}%", p.context_pct),
                    "CONTEXT".to_string(),
                    tac::INK,
                ),
                (
                    tac::clock_ms(now - p.since),
                    "IN STATE".to_string(),
                    tac::INK,
                ),
            ],
            menu,
            body: div()
                .size_full()
                .flex()
                .gap(px(32.))
                .child(left)
                .child(right)
                .into_any_element(),
        })
    }
}
