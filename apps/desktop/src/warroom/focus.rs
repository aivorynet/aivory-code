//! The focus panel of war room v2: one session on the right, with a Chat tab (the transcript)
//! and a Code tab (the changed files and their diff), the open ask, and the composer.

use std::time::{Duration, Instant};

use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::{Pane, SessionState, WarRoom};
use gpui::Focusable;
use gpui::prelude::*;
use gpui::{Context, Div, FontWeight, SharedString, Window, div, px, relative, rgb};

use super::decisions::{decisions, diff_rows};
use super::{BodyKey, FocusTab, WarRoomEvent, WarRoomScreen};
use crate::theme::*;
use crate::ui;

/// How long a read diff counts as fresh.
const DIFF_TTL: Duration = Duration::from_secs(20);

/// The focus panel width for a window width: 42 % of the room beside the rail, 520-680 px.
pub(super) fn focus_width(window: f32) -> f32 {
    ((window - 76.) * 0.42).round().clamp(520., 680.)
}

impl WarRoomScreen {
    /// Open a session in the focus panel. The Overview column closes.
    pub(super) fn open_focus(&mut self, id: &str, tab: FocusTab, cx: &mut Context<Self>) {
        self.focus_session = Some(id.to_string());
        self.focus_tab = tab;
        self.focus_file = None;
        self.walk = false;
        self.open = super::Overlay::None;
        cx.notify();
    }

    pub(super) fn close_focus(&mut self, cx: &mut Context<Self>) {
        self.focus_session = None;
        self.walk = false;
        cx.notify();
    }

    /// After an answer: in "one by one" mode the panel moves to the next waiting session.
    pub(super) fn answered(&mut self, session: &str, cx: &mut Context<Self>) {
        if self.context.as_deref() == Some(session) {
            self.context = None;
        }
        if self.walk && self.focus_session.as_deref() == Some(session) {
            let next = self.war.as_ref().and_then(|w| {
                decisions(w)
                    .into_iter()
                    .find(|s| s.id != session)
                    .map(|s| s.id.clone())
            });
            match next {
                Some(id) => {
                    self.focus_session = Some(id);
                    self.focus_file = None;
                }
                None => self.walk = false,
            }
        }
        cx.notify();
    }

    /// Read the diff of a session again when the last read is old.
    pub(super) fn ensure_diff(&mut self, session: &str, cx: &mut Context<Self>) {
        let stale = self
            .diffs
            .get(session)
            .is_none_or(|(_, at)| at.elapsed() > DIFF_TTL);
        if !stale || !self.diffs_busy.insert(session.to_string()) {
            return;
        }
        let task = self.model.read(cx).call::<method::SessionDiffMethod>(
            SessionRef {
                id: session.to_string(),
            },
            cx,
        );
        let id = session.to_string();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.diffs_busy.remove(&id);
                this.diffs.insert(id, (result.ok(), Instant::now()));
                cx.notify();
            });
        })
        .detach();
    }

    /// The panes in grid order, for the up and down arrows.
    fn focus_order(&self, war: &WarRoom) -> Vec<String> {
        self.groups
            .iter()
            .flat_map(|g| g.sessions.iter())
            .filter_map(|&i| war.sessions.get(i).map(|s| s.id.clone()))
            .collect()
    }

    pub(super) fn render_focus(
        &mut self,
        war: &WarRoom,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let id = self.focus_session.clone()?;
        let Some(pane) = war.session(&id).cloned() else {
            self.focus_session = None;
            return None;
        };
        self.ensure_diff(&id, cx);
        let tab = self.tabs.get(&id).copied().unwrap_or(0);
        let key = BodyKey {
            session: id.clone(),
            subagent: tab
                .checked_sub(1)
                .and_then(|i| pane.subagents.get(i))
                .map(|s| s.id.clone()),
            focus: true,
        };
        self.ensure_body(&key, &pane, cx);
        let input = self.composer_input(&pane, window, cx);
        let focused = input.focus_handle(cx).contains_focused(window, cx);
        let mut order = self.focus_order(war);
        if !order.contains(&id) {
            order.push(id.clone());
        }
        let ix = order.iter().position(|s| s == &id).unwrap_or(0);
        let n = order.len();
        let prev = order[(ix + n - 1) % n].clone();
        let next = order[(ix + 1) % n].clone();
        let diff = self.diffs.get(&id).and_then(|(d, _)| d.clone());
        let changed: Vec<_> = diff
            .as_ref()
            .map(|d| d.files.iter().filter(|f| !f.lines.is_empty()).collect())
            .unwrap_or_default();
        let (add, del) = changed
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.added, d + f.removed));
        let files_text = format!(
            "{} {} · +{add} −{del}",
            changed.len(),
            if changed.len() == 1 { "file" } else { "files" }
        );
        let session_ref = |id: &str| SessionRef { id: id.to_string() };
        let icon_button = |name: &str, icon: &'static str, tip: &'static str| {
            div()
                .id(SharedString::from(format!("f-{name}")))
                .size(px(28.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(tk().track)))
                .tooltip(ui::tip(tip))
                .child(ui::icon(icon, 13., rgb(tk().ink_3)))
        };
        let head = div()
            .flex_none()
            .pt(px(16.))
            .px(px(18.))
            .pb(px(12.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w(px(0.))
                    .child(ui::state_dot(pane.state, 8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(tk().display_heading)
                            .text_size(ui::ts(17.))
                            .font_weight(FontWeight::NORMAL)
                            .child(pane.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .mr(px(4.))
                            .font_family(tk().mono)
                            .text_size(ui::ts(T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(format!("{} of {n}", ix + 1))
                            .child(
                                icon_button("prev", "icons/chevron-up.svg", "Previous session")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.focus_session = Some(prev.clone());
                                        this.focus_file = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                icon_button("next", "icons/chevron-down.svg", "Next session")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.focus_session = Some(next.clone());
                                        this.focus_file = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        icon_button("pause", "icons/pause.svg", "Pause session").on_click({
                            let r = session_ref(&id);
                            cx.listener(move |this, _, _, cx| {
                                this.send::<method::SessionPause>(r.clone(), cx)
                            })
                        }),
                    )
                    .child(
                        icon_button("stop", "icons/stop.svg", "Stop session").on_click({
                            let r = session_ref(&id);
                            cx.listener(move |this, _, _, cx| {
                                this.send::<method::SessionStop>(r.clone(), cx)
                            })
                        }),
                    )
                    .child(
                        icon_button("deep", "icons/open.svg", "Open the session deep dive")
                            .on_click({
                                let id = id.clone();
                                cx.listener(move |_, _, _, cx| {
                                    cx.emit(WarRoomEvent::OpenSession(id.clone()))
                                })
                            }),
                    )
                    .child(
                        icon_button("close", "icons/close.svg", "Close")
                            .on_click(cx.listener(|this, _, _, cx| this.close_focus(cx))),
                    ),
            )
            .child(
                ui::button_sm("f-open-focus", "Open in Focus view", true)
                    .w_full()
                    .tooltip(ui::tip("Chat and code of this session (Enter)"))
                    .on_click({
                        let id = id.clone();
                        cx.listener(move |_, _, _, cx| cx.emit(WarRoomEvent::Focus(id.clone())))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(16.))
                    .min_w(px(0.))
                    .font_family(tk().mono)
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(
                        div()
                            .text_color(rgb(tk().ink))
                            .whitespace_nowrap()
                            .child(format!("{} / {}", pane.codebase, pane.part)),
                    )
                    .child(
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(format!(
                                "{} {} · {}",
                                pane.provider,
                                pane.model,
                                super::pane::meta_text(&pane, war.now)
                            )),
                    )
                    .child(div().flex_1())
                    .when(!pane.subagents.is_empty(), |d| {
                        d.child(self.subagent_switch(&pane, tab, cx))
                    }),
            );
        let tab_button = |which: FocusTab, label: &'static str, extra: Option<String>| {
            let on = self.focus_tab == which;
            div()
                .id(SharedString::from(format!("ftab-{label}")))
                .h_full()
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(ui::ts(T_SMALL))
                .text_color(rgb(if on { tk().ink } else { tk().ink_3 }))
                .when(on, |d| {
                    d.font_weight(FontWeight::MEDIUM)
                        .border_b_2()
                        .border_color(rgb(tk().ink))
                })
                .cursor_pointer()
                .child(label)
                .when_some(extra, |d, e| {
                    d.child(
                        div()
                            .font_family(tk().mono)
                            .font_weight(FontWeight::NORMAL)
                            .text_size(ui::ts(T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(e),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.focus_tab = which;
                    cx.notify();
                }))
        };
        let tabs = div()
            .flex_none()
            .h(px(40.))
            .px(px(12.))
            .flex()
            .gap(px(4.))
            .child(tab_button(FocusTab::Chat, "Chat", None))
            .child(tab_button(
                FocusTab::Code,
                "Code",
                (!changed.is_empty()).then_some(files_text),
            ));
        let body = match self.focus_tab {
            FocusTab::Chat => div()
                .flex_1()
                .min_h(px(0.))
                .px(px(6.))
                .pt(px(8.))
                .child(self.transcript(&key, cx)),
            FocusTab::Code => self.code_tab(&pane, diff.as_ref(), cx),
        };
        let mut panel = div()
            .flex_none()
            .w(px(width))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(tk().card))
            .shadow(ui::side_shadow(-8.))
            .child(head)
            .child(tabs)
            .child(body);
        if let Some(ask) = &pane.ask {
            panel = panel.child(self.focus_ask(&pane, ask, war, cx));
        }
        Some(panel.child(self.fold(&pane, true, focused, input, true, cx)))
    }

    /// The changed files on the left, the diff of the picked file on the right.
    fn code_tab(
        &self,
        pane: &Pane,
        diff: Option<&aivory_protocol::session::SessionDiff>,
        cx: &mut Context<Self>,
    ) -> Div {
        let files = diff.map(|d| d.files.clone()).unwrap_or_default();
        if files.is_empty() {
            return div()
                .flex_1()
                .p(px(18.))
                .text_size(ui::ts(T_SMALL))
                .text_color(rgb(tk().ink_3))
                .child(if diff.is_some() {
                    "No changed files yet."
                } else {
                    "Reads the diff…"
                });
        }
        let first_changed = files.iter().position(|f| !f.lines.is_empty()).unwrap_or(0);
        let pick = self
            .focus_file
            .unwrap_or(first_changed)
            .min(files.len() - 1);
        let mut tree = div()
            .id("ftree")
            .w(px(200.))
            .flex_none()
            .overflow_y_scroll()
            .py(px(12.))
            .px(px(8.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .font_family(tk().mono)
            .text_size(ui::ts(T_CAPTION))
            .child(
                div()
                    .px(px(8.))
                    .pb(px(8.))
                    .text_color(rgb(tk().ink))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{} / {}", pane.codebase, pane.part)),
            );
        for (i, f) in files.iter().enumerate() {
            let on = i == pick;
            let read_only = f.lines.is_empty();
            let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
            tree = tree.child(
                div()
                    .id(("ff", i))
                    .w_full()
                    .px(px(8.))
                    .py(px(6.))
                    .flex()
                    .justify_between()
                    .gap(px(8.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(rgb(tk().track)))
                    .hover(|s| s.bg(rgb(tk().track)))
                    .tooltip(ui::tip(f.path.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.focus_file = Some(i);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(rgb(if read_only { tk().ink_3 } else { tk().ink }))
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(if read_only { tk().ink_3 } else { tk().ok_text }))
                            .child(if read_only {
                                "read".to_string()
                            } else {
                                format!("+{} −{}", f.added, f.removed)
                            }),
                    ),
            );
        }
        let file = files[pick].clone();
        let session = pane.id.clone();
        let path = file.path.clone();
        let ask_path = file.path.clone();
        let agent = format!("Ask {}", pane.provider);
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(tk().mono)
                    .text_size(ui::ts(T_CAPTION))
                    .child(file.path.clone()),
            )
            .child(ui::button_sm("f-ask", agent, false).on_click({
                let session = session.clone();
                cx.listener(move |this, _, window, cx| {
                    if let Some(c) = this.inputs.get(&session) {
                        let text = format!("About @{ask_path}: ");
                        c.input.update(cx, |s, cx| s.set_value(text, window, cx));
                        c.input.focus_handle(cx).focus(window, cx);
                    }
                })
            }))
            .child(
                ui::button_sm("f-ide", "Open in IDE", false).on_click(cx.listener(
                    move |_, _, _, cx| {
                        cx.emit(WarRoomEvent::OpenFile {
                            session: session.clone(),
                            path: path.clone(),
                        })
                    },
                )),
            );
        div().flex_1().min_h(px(0.)).flex().child(tree).child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(head)
                .child(
                    div()
                        .id("fdiff")
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_y_scroll()
                        .child(diff_rows(&file.lines, false)),
                ),
        )
    }

    /// The ask card of the focus panel: a white card with a ring in the state color.
    fn focus_ask(
        &self,
        pane: &Pane,
        ask: &aivory_protocol::Ask,
        war: &WarRoom,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = if pane.state == SessionState::Error {
            tk().e
        } else {
            tk().q
        };
        let mut options = div().flex().flex_wrap().items_center().gap(px(6.));
        if pane.controlled {
            for (i, option) in ask.options.iter().enumerate() {
                options = options.child(self.answer_button(pane, ask, i, option, ("fask", i), cx));
            }
        } else {
            options = options.child(super::decisions::terminal_note());
        }
        if self.walk
            && let Some(next) = decisions(war).into_iter().find(|s| s.id != pane.id)
        {
            options = options.child(
                div()
                    .ml_auto()
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(format!("next: {}", next.title)),
            );
        }
        div()
            .flex_none()
            .mx(px(14.))
            .mb(px(4.))
            .py(px(14.))
            .px(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .bg(rgb(tk().card))
            .rounded(px(20.))
            .shadow(ui::ring(state, 1.5))
            .child(
                div()
                    .text_size(ui::ts(T_CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(state))
                    .child(ask.kind.label()),
            )
            .child(
                div()
                    .text_size(ui::ts(T_BODY))
                    .line_height(relative(1.5))
                    .when(ask.kind == aivory_protocol::AskKind::Permission, |d| {
                        d.font_family(tk().mono).text_size(ui::ts(T_SMALL))
                    })
                    .child(ask.text.clone()),
            )
            .when(!ask.steps.is_empty(), |d| {
                let mut steps = div().flex().flex_col().gap(px(2.));
                for (i, step) in ask.steps.iter().enumerate() {
                    steps = steps.child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .text_size(ui::ts(T_SMALL))
                            .child(
                                div()
                                    .text_color(rgb(tk().ink_3))
                                    .child(format!("{}.", i + 1)),
                            )
                            .child(div().flex_1().min_w(px(0.)).child(step.clone())),
                    );
                }
                d.child(steps)
            })
            .child(options)
    }
}
