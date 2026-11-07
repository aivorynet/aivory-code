//! One session pane in the Genie chat style: a one-row header, a transcript that starts at the
//! top, the ask as a raised card under it, and the rounded input box.

use std::rc::Rc;
use std::time::Duration;

/// How long the ring of a pane that a header pill scrolled to stays.
pub(super) const SPOT_FOR: Duration = Duration::from_millis(2500);

use aivory_protocol::layout::PaneDensity;
use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::{Ask, AskKind, Effort, Mode, Pane, SessionState};
use gpui::Focusable;
use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt, AnyElement, App, Context, Div, FontWeight, MouseButton, SharedString,
    Window, div, ease_out_quint, list, px, relative, rgb, rgba,
};

use super::{BodyKey, ComposerMenu, Overlay, WarRoomEvent, WarRoomScreen};
use crate::compose;
use crate::console::{Frame, Hooks, Style};
use crate::theme::*;
use crate::ui;

impl WarRoomScreen {
    /// A text size of the pane: one step smaller in a compact or mini pane.
    fn pts(&self, size: f32) -> gpui::Pixels {
        let small = self.geom.compact || self.geom.density != PaneDensity::Normal;
        ui::ts(if small { size - 1. } else { size })
    }

    pub(super) fn render_pane(
        &mut self,
        pane: &Pane,
        now: aivory_protocol::UnixMs,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let g = self.geom;
        let t = g.theme;
        let id: SharedString = pane.id.clone().into();
        let hovered = self.hovered.as_ref() == Some(&id);
        let tab = self.tabs.get(&pane.id).copied().unwrap_or(0);
        let key = BodyKey {
            session: pane.id.clone(),
            subagent: tab
                .checked_sub(1)
                .and_then(|i| pane.subagents.get(i))
                .map(|s| s.id.clone()),
            focus: false,
        };
        self.ensure_body(&key, pane, cx);
        self.ensure_blocks(&pane.id, cx);
        // The focused session writes in the focus panel; its pane keeps no composer.
        let in_focus = self.focus_session.as_deref() == Some(pane.id.as_str());
        let input = (!in_focus).then(|| self.composer_input(pane, window, cx));
        let focused = input
            .as_ref()
            .is_some_and(|i| i.focus_handle(cx).contains_focused(window, cx));
        // The composer stays open while it holds text, a chip, or a running tool.
        let holds = input
            .as_ref()
            .is_some_and(|i| i.read(cx).value().chars().any(|c| !c.is_whitespace()))
            || self.tools.get(&pane.id).is_some_and(|t| t.active());
        let spot = self
            .spot
            .as_ref()
            .is_some_and(|(spot, at)| spot == &id && at.elapsed() < SPOT_FOR);
        let selected = self.selected.contains(&pane.id);
        let shadow = if selected {
            ui::ring(tk().ink, 2.)
        } else if in_focus || spot {
            ui::ring(tk().link, 2.)
        } else {
            // The state shows in the dot and the wait bar only; a ring per pane was too loud.
            if hovered {
                ui::card_shadow_hover()
            } else {
                ui::card_shadow()
            }
        };
        let hover_id = id.clone();
        let mut el = div()
            .id(id.clone())
            .relative()
            .flex()
            .flex_col()
            .h(px(g.pane_height))
            .min_w(px(0.))
            .rounded(tk().r_pane)
            .overflow_hidden()
            .bg(rgba(t.bg))
            .shadow(shadow)
            .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                let next = over.then(|| hover_id.clone());
                if this.hovered != next && (*over || this.hovered.as_ref() == Some(&hover_id)) {
                    this.hovered = next;
                    cx.notify();
                }
            }))
            .when_some(input.as_ref(), |d, input| {
                let focus = input.focus_handle(cx);
                d.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    focus.focus(window, cx)
                })
            })
            .child(self.pane_header(pane, hovered, tab, now, cx))
            .children(self.block_banner(pane, cx))
            .child(self.pane_body(&key, pane, cx));
        if let Some(ask) = &pane.ask {
            el = el.child(self.wait_bar(pane, ask, cx));
        }
        // Hover never changes the size of the composer, so nothing moves under the mouse;
        // only focus on the text field (or held content) opens it.
        if let Some(input) = input {
            let open = focused || holds;
            el = el.child(self.fold(pane, open, focused, input, false, cx));
        }
        el.into_any_element()
    }

    /// `Permission required · answer in Decisions`: a tinted bar over the composer. A click
    /// opens the context popup of the decision.
    fn wait_bar(&self, pane: &Pane, ask: &Ask, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let error = pane.state == SessionState::Error;
        let (bg, fg) = if error {
            (tk().e_bg, tk().e_text)
        } else {
            (tk().q_bg, tk().q_text)
        };
        let session = pane.id.clone();
        let mini = self.geom.density != PaneDensity::Normal;
        div()
            .id(SharedString::from(format!("wait-{}", pane.id)))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .mx(px(16.))
            .mb(px(4.))
            .px(px(12.))
            .py(px(9.))
            .rounded(tk().r_card)
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .text_size(self.pts(T_SMALL))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .hover(|s| s.underline())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.context = Some(session.clone());
                cx.notify();
            }))
            .child(
                ui::state_dot(pane.state, 8.).when(pane.state == SessionState::NeedsYou, |d| {
                    d.opacity(0.35 + 0.65 * self.breath_fast())
                }),
            )
            .child(if mini {
                ask.kind.label().to_string()
            } else {
                format!(
                    "{} · {}",
                    ask.kind.label(),
                    if pane.controlled {
                        "answer in Decisions"
                    } else {
                        "answer in its terminal"
                    }
                )
            })
    }

    /// Two lines (`.ph`): the select box or the state dot, the title, and the meta (hover shows
    /// pause, stop and the deep dive); then `codebase / part`, `agent model`, code and subagents.
    fn pane_header(
        &self,
        pane: &Pane,
        hovered: bool,
        tab: usize,
        now: aivory_protocol::UnixMs,
        cx: &mut Context<Self>,
    ) -> Div {
        let g = self.geom;
        let t = g.theme.header();
        let mini = g.density != PaneDensity::Normal;
        let micro = g.density == PaneDensity::Micro;
        let selected = self.selected.contains(&pane.id);
        let select_mode = !self.selected.is_empty();
        let show_check = hovered || select_mode || selected;
        let check = {
            let session = pane.id.clone();
            div()
                .id(SharedString::from(format!("ck-{}", pane.id)))
                .flex_none()
                .size(px(15.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .border_1()
                .border_color(rgba(if selected { t.text } else { t.hover }))
                .when(selected, |d| d.bg(rgba(t.text)))
                .cursor_pointer()
                .tooltip(ui::tip("Select session"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.selected.remove(&session) {
                        this.selected.insert(session.clone());
                    }
                    cx.notify();
                }))
                .when(selected, |d| {
                    d.child(div().text_size(px(10.)).text_color(rgba(t.bg)).child("✓"))
                })
        };
        let title = {
            let session = pane.id.clone();
            div()
                .id(SharedString::from(format!("title-{}", pane.id)))
                .flex_1()
                .min_w(px(0.))
                .when(micro, |d| d.invisible())
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(tk().display_heading)
                .text_size(self.pts(16.))
                .font_weight(FontWeight::NORMAL)
                .text_color(rgba(if pane.state == SessionState::Idle {
                    t.dim
                } else {
                    t.text
                }))
                .cursor_pointer()
                .hover(|s| s.underline())
                .tooltip(ui::tip("Open it on the right"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_focus(&session, super::FocusTab::Chat, cx)
                }))
                .child(pane.title.clone())
        };
        let action = |name: &str, icon: &'static str, tip: &'static str| {
            div()
                .id(SharedString::from(format!("{name}-{}", pane.id)))
                .flex_none()
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.ghost_hover)))
                .tooltip(ui::tip(tip))
                .child(ui::icon(icon, 13., rgba(t.dim)))
        };
        let session_ref = || SessionRef {
            id: pane.id.clone(),
        };
        let actions = div()
            .flex()
            .flex_none()
            .gap(px(2.))
            .child(
                action("pause", "icons/pause.svg", "Pause session").on_click({
                    let r = session_ref();
                    cx.listener(move |this, _, _, cx| {
                        this.send::<method::SessionPause>(r.clone(), cx)
                    })
                }),
            )
            .child(action("stop", "icons/stop.svg", "Stop session").on_click({
                let r = session_ref();
                cx.listener(move |this, _, _, cx| this.send::<method::SessionStop>(r.clone(), cx))
            }))
            .child(
                action("open", "icons/open.svg", "Open the session deep dive").on_click({
                    let session = pane.id.clone();
                    cx.listener(move |_, _, _, cx| {
                        cx.emit(WarRoomEvent::OpenSession(session.clone()))
                    })
                }),
            );
        let line1 = div()
            .h(px(22.))
            .flex()
            .items_center()
            .gap(px(10.))
            .min_w(px(0.))
            .map(|d| {
                if show_check {
                    d.child(check)
                } else {
                    // A working dot is ink: on a dark console it takes the console text color.
                    // Working and waiting dots breathe; a waiting one faster, so it calls.
                    let dot = match pane.state {
                        SessionState::Working => {
                            ui::dot(rgba(t.text)).opacity(0.45 + 0.55 * self.breath(0.))
                        }
                        SessionState::NeedsYou => {
                            ui::state_dot(pane.state, 8.).opacity(0.35 + 0.65 * self.breath_fast())
                        }
                        _ => ui::state_dot(pane.state, 8.),
                    };
                    d.child(div().w(px(15.)).flex().justify_center().child(dot))
                }
            })
            .child(title)
            .map(|d| {
                if hovered && !micro {
                    d.child(actions)
                } else {
                    d.child(
                        div()
                            .flex_none()
                            .font_family(tk().mono)
                            .text_size(self.pts(T_CAPTION))
                            .text_color(rgba(t.dim))
                            .whitespace_nowrap()
                            .child(meta_text(pane, now)),
                    )
                }
            });
        let code = {
            let session = pane.id.clone();
            let files = self
                .diffs
                .get(&pane.id)
                .and_then(|(d, _)| d.as_ref())
                .map(|d| d.files.iter().filter(|f| !f.lines.is_empty()).count());
            div()
                .id(SharedString::from(format!("code-{}", pane.id)))
                .flex_none()
                .h(px(24.))
                .px(px(7.))
                .flex()
                .items_center()
                .gap(px(5.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.ghost_hover)))
                .tooltip(ui::tip("Show the changed files"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_focus(&session, super::FocusTab::Code, cx)
                }))
                .child(ui::icon("icons/code.svg", 13., rgba(t.dim)))
                .when_some(files, |d, n| d.child(n.to_string()))
        };
        let line2 = div()
            .h(px(22.))
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w(px(0.))
            .pl(px(25.))
            .font_family(tk().mono)
            .text_size(self.pts(T_CAPTION))
            .text_color(rgba(t.dim))
            .child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(rgba(t.text))
                    .child(format!("{} / {}", pane.codebase, pane.part)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(format!("{} {}", pane.provider, pane.model)),
            )
            .child(code)
            .when(!pane.subagents.is_empty(), |d| {
                d.child(self.subagent_switch(pane, tab, cx))
            });
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(if mini { 4. } else { 8. }))
            .pt(px(if mini { 10. } else { 16. }))
            .px(px(if mini { 12. } else { 20. }))
            .pb(px(6.))
            .child(line1)
            .when(!micro, |d| d.child(line2))
    }

    /// "Blocked by <session>: <reason>" with Override, when the mailbox holds an open block.
    fn block_banner(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<Div> {
        let block = self.blocks.get(&pane.id)?.0.first()?.clone();
        let t = self.geom.theme;
        let session = pane.id.clone();
        let key = block.key.clone();
        Some(crate::block::banner(
            &block,
            &pane.id,
            crate::block::Colors {
                bg: t.ask_bg,
                border: t.ask_border,
                text: t.ask_head,
                hover: t.ghost_hover,
            },
            self.geom.density != PaneDensity::Normal,
            Box::new(
                cx.listener(move |this, _, _, cx| this.override_block(&session, key.clone(), cx)),
            ),
        ))
    }

    pub(super) fn subagent_switch(&self, pane: &Pane, tab: usize, cx: &mut Context<Self>) -> Div {
        let t = self.geom.theme.header();
        let on = tab > 0;
        let label = match tab.checked_sub(1).and_then(|i| pane.subagents.get(i)) {
            Some(sub) => sub.name.clone(),
            None => pane.subagents.len().to_string(),
        };
        let open = self.open == Overlay::Subagents(pane.id.clone());
        let session = pane.id.clone();
        let mut wrap = div().relative().flex_none().child(
            div()
                .id(SharedString::from(format!("subs-{}", pane.id)))
                .h(px(24.))
                .px(px(7.))
                .flex()
                .items_center()
                .gap(px(5.))
                .rounded(px(6.))
                .font_family(tk().mono)
                .when(on, |d| {
                    d.bg(rgba(t.sub_on_bg)).text_color(rgba(t.sub_on_text))
                })
                .when(!on, |d| d.text_color(rgba(t.dim)))
                .whitespace_nowrap()
                .cursor_pointer()
                .tooltip(ui::tip("Subagents"))
                .hover(|s| s.bg(rgba(t.ghost_hover)))
                .child(ui::icon(
                    "icons/branch.svg",
                    13.,
                    rgba(if on { t.sub_on_text } else { t.dim }),
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle(Overlay::Subagents(session.clone()), cx)
                }))
                .child(label),
        );
        if open {
            let mut items = ui::menu(220.);
            let entries = std::iter::once(("main".to_string(), pane.state))
                .chain(pane.subagents.iter().map(|s| (s.name.clone(), s.state)));
            for (i, (name, state)) in entries.enumerate() {
                let session = pane.id.clone();
                items = items.child(ui::menu_item(
                    SharedString::from(format!("sub-{}-{i}", pane.id)),
                    i == tab,
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .font_family(tk().sans)
                        .child(ui::state_dot(state, 8.))
                        .child(name)
                        .into_any_element(),
                    Some(state.label().into()),
                    Box::new(cx.listener(move |this, _, _, cx| {
                        this.tabs.insert(session.clone(), i);
                        this.open = Overlay::None;
                        cx.notify();
                    })),
                ));
            }
            wrap =
                wrap.child(self.overlay(div().absolute().top(px(28.)).right_0().child(items), cx));
        }
        wrap
    }

    /// The console body: the transcript as a list of blocks, and the status line of a working
    /// pane under it.
    /// The geometry of the focus panel and the context popup: a white panel reads with the
    /// light console and the full pane layout.
    pub(super) fn panel_geom(&self, panel: bool) -> super::Geom {
        let mut g = self.geom;
        if panel {
            g.theme = if crate::theme::tk().look == aivory_protocol::Look::Bridge {
                PaneTheme::BRIDGE
            } else {
                PaneTheme::WHITE
            };
            g.compact = false;
            g.density = PaneDensity::Normal;
        }
        g
    }

    /// A transcript as a list of blocks that follows the newest line.
    pub(super) fn transcript(&self, key: &BodyKey, cx: &mut Context<Self>) -> Div {
        let Some(body) = self.bodies.get(key) else {
            return div().size_full();
        };
        div().size_full().child(
            list(body.list.list.clone(), {
                let key = key.clone();
                cx.processor(move |this, ix: usize, _, cx| this.render_line(&key, ix, cx))
            })
            .size_full(),
        )
    }

    fn pane_body(&self, key: &BodyKey, pane: &Pane, cx: &mut Context<Self>) -> Div {
        let t = self.geom.theme;
        if !self.bodies.contains_key(key) {
            return div().flex_1();
        }
        // The transcript follows the newest line; no live status line, so the pane stays calm.
        let console = div()
            .flex_1()
            .min_h(px(0.))
            .relative()
            .px(px(6.))
            .child(self.transcript(key, cx))
            // The top edge fades out, so a line cut by the scroll never shows half.
            .child(div().absolute().top_0().left_0().right_0().h(px(28.)).bg(
                gpui::linear_gradient(
                    180.,
                    gpui::linear_color_stop(rgba(t.bg), 0.),
                    gpui::linear_color_stop(rgba(t.bg & 0xFFFFFF00), 1.),
                ),
            ));
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .when(pane.state == SessionState::Idle, |d| d.opacity(0.6))
            .child(console)
    }

    /// The hooks of a transcript: fold, open a file, load older lines.
    pub(super) fn hooks(&self, key: &BodyKey, cx: &mut Context<Self>) -> Hooks {
        let weak = cx.entity().downgrade();
        let toggle = {
            let (weak, key) = (weak.clone(), key.clone());
            Rc::new(move |abs: u32, _: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| this.toggle_fold(&key, abs, cx));
            })
        };
        let open_file = {
            let (weak, session) = (weak.clone(), key.session.clone());
            Rc::new(move |path: String, _: &mut Window, cx: &mut App| {
                let session = session.clone();
                let _ = weak.update(cx, |_, cx| {
                    cx.emit(WarRoomEvent::OpenFile { session, path })
                });
            })
        };
        let load_older = {
            let key = key.clone();
            Rc::new(move |_: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| this.load_older(&key, cx));
            })
        };
        Hooks {
            toggle: Some(toggle),
            open_file: Some(open_file),
            load_older: Some(load_older),
        }
    }

    pub(super) fn render_line(
        &self,
        key: &BodyKey,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(body) = self.bodies.get(key) else {
            return div().into_any_element();
        };
        let style = Style::of(&self.panel_geom(key.focus));
        let hooks = self.hooks(key, cx);
        let id: SharedString = match &key.subagent {
            Some(sub) => format!("{}~{sub}", key.session).into(),
            None => key.session.clone().into(),
        };
        let frame = Frame {
            style: &style,
            hooks: &hooks,
            id: &id,
            tick: self.tick,
        };
        body.list.item(ix, &frame)
    }

    /// The input box in the Genie style: one rounded box, capture over attach on the left,
    /// the text field with the pickers and `ctx · cost` under it, send on the right. A normal
    /// pane always shows it; a mini pane opens it on hover or focus.
    pub(super) fn fold(
        &self,
        pane: &Pane,
        open: bool,
        focused: bool,
        input: gpui::Entity<gpui_base::input::TextareaState>,
        wide: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let g = self.panel_geom(wide);
        let t = g.theme;
        let cs = Style::of(&g);
        let mini = g.density != PaneDensity::Normal && !wide;
        let small = (mini || g.compact) && !wide;
        let full = open || !mini;
        // Micro panes have no room for a closed composer; hover opens the full one.
        if !full && (pane.ask.is_none() || g.density == PaneDensity::Micro) {
            return div().into_any_element();
        }
        let prefs = self.composer_prefs(pane);
        let tools = self.tools.get(&pane.id);
        let palette = compose::Palette::of(&t);
        let icon_button = |name: String, icon: &'static str, tip: &'static str| {
            div()
                .id(SharedString::from(name))
                .tooltip(ui::tip(tip))
                .w(px(if small { 30. } else { 36. }))
                .h(px(if small { 30. } else { 34. }))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.ghost_hover)))
                .child(ui::icon(icon, if small { 14. } else { 15. }, rgba(t.dim)))
        };
        let field = div()
            .min_h(px(if wide { 44. } else { 22. }))
            .mt(px(8.))
            .px(px(2.))
            .text_size(if wide {
                ui::ts(T_BODY)
            } else {
                self.pts(T_BODY)
            })
            .line_height(relative(1.45))
            .font_family(tk().sans)
            .text_color(rgba(t.text))
            .cursor_text()
            .child(input);
        let mut center = div().flex_1().min_w(px(0.)).flex().flex_col().pb(px(4.));
        if let Some(tools) = tools {
            let session = pane.id.clone();
            let chips = compose::chips(tools, &session, palette, |i| {
                let session = session.clone();
                Box::new(cx.listener(move |this, _, _, cx| {
                    this.tools_of(&session).remove(i);
                    cx.notify();
                }))
            });
            let status = compose::status(tools, &session, palette, |action| {
                let session = session.clone();
                Box::new(cx.listener(move |this, _, window, cx| {
                    this.tool_action(&session, action, window, cx)
                }))
            });
            center = center.children(chips).children(status);
        }
        center = center.child(field);
        if full {
            // The pickers shrink and clip; `ctx · cost` keeps its room on the right.
            let mut pickers = div()
                .flex_1()
                .flex()
                .items_center()
                .gap(px(2.))
                .min_w(px(0.))
                .overflow_hidden();
            for (menu, label) in [
                (ComposerMenu::Mode, format!("{} ▾", mode_label(prefs.mode))),
                (ComposerMenu::Provider, format!("{} ▾", prefs.provider)),
                (ComposerMenu::Model, format!("{} ▾", prefs.model)),
                (
                    ComposerMenu::Effort,
                    format!("{} ▾", effort_label(prefs.effort)),
                ),
            ] {
                // The grid shows mode and model; the focus panel shows every picker.
                if !wide && matches!(menu, ComposerMenu::Effort | ComposerMenu::Provider) {
                    continue;
                }
                let session = pane.id.clone();
                let on = self.open == Overlay::Composer(pane.id.clone(), menu);
                pickers = pickers.child(
                    div()
                        .id(SharedString::from(format!("cm-{}-{menu:?}", pane.id)))
                        .h(px(24.))
                        .px(px(6.))
                        .flex()
                        .flex_none()
                        .items_center()
                        .rounded(px(6.))
                        .text_size(self.pts(T_SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .whitespace_nowrap()
                        .text_color(rgba(if on { t.text } else { t.dim }))
                        .when(on, |d| d.bg(rgba(t.ghost_hover)))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(t.ghost_hover)).text_color(rgba(t.text)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle(Overlay::Composer(session.clone(), menu), cx)
                        }))
                        .child(label),
                );
            }
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .pt(px(4.))
                .ml(px(-4.))
                .min_w(px(0.))
                .child(pickers);
            // `ctx · cost` shows in the focus panel only; the grid composer stays short.
            if wide {
                let pct = pane.context_pct.min(100);
                row = row.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(8.))
                        .pr(px(2.))
                        .text_size(self.pts(T_SMALL))
                        .whitespace_nowrap()
                        .text_color(rgba(t.dim))
                        .child(format!("ctx {pct} %"))
                        .child(
                            div()
                                .w(px(40.))
                                .h(px(4.))
                                .rounded(px(2.))
                                .bg(rgba(t.line))
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(pct as f32 / 100.))
                                        .rounded(px(2.))
                                        .bg(rgba(t.dim)),
                                ),
                        )
                        .child(format!("${:.2}", pane.cost)),
                );
            }
            center = center.child(row);
        }
        let composer = div()
            .relative()
            .flex()
            .rounded(px(14.))
            .bg(rgba(if focused { t.bg } else { t.field_bg }))
            .when(focused, |d| d.shadow(ui::ring(tk().ink, 2.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .child(
                        icon_button(
                            format!("cap-{}", pane.id),
                            "icons/capture.svg",
                            "Capture a screen area",
                        )
                        .rounded_tl(px(14.))
                        .on_click({
                            let id = pane.id.clone();
                            cx.listener(move |this, _, window, cx| this.capture(&id, window, cx))
                        }),
                    )
                    .when(full, |d| {
                        d.child(
                            icon_button(
                                format!("att-{}", pane.id),
                                "icons/attach.svg",
                                "Attach files",
                            )
                            .on_click({
                                let id = pane.id.clone();
                                cx.listener(move |this, _, window, cx| this.attach(&id, window, cx))
                            }),
                        )
                    }),
            )
            .child(center)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .when(full && compose::VOICE_AVAILABLE, |d| {
                        let listening = tools.is_some_and(|t| t.listening.is_some());
                        d.child(
                            icon_button(
                                format!("mic-{}", pane.id),
                                "icons/mic.svg",
                                if listening {
                                    "Stop listening"
                                } else {
                                    "Dictate"
                                },
                            )
                            .rounded_tr(px(14.))
                            .when(listening, |d| d.bg(rgba(t.err_head_bg)))
                            .on_click({
                                let id = pane.id.clone();
                                cx.listener(move |this, _, window, cx| {
                                    this.dictate(&id, window, cx)
                                })
                            }),
                        )
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("send-{}", pane.id)))
                            .tooltip(ui::tip("Send (Enter)"))
                            .mt_auto()
                            .w(px(if small { 34. } else { 44. }))
                            .h(px(if small { 32. } else { 40. }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_br(px(14.))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(t.ghost_hover)))
                            .on_click({
                                let id = pane.id.clone();
                                cx.listener(move |this, _, window, cx| this.submit(&id, window, cx))
                            })
                            .child(ui::icon(
                                "icons/send.svg",
                                if small { 15. } else { 16. },
                                rgba(t.text),
                            )),
                    ),
            )
            .when_some(self.composer_menu(pane, &prefs, cx), |d, m| d.child(m));
        let fold = div()
            .flex_none()
            .pt(px(if wide { 12. } else { 8. }))
            .px(px(if wide { 14. } else { (cs.pad_x - 4.).max(12.) }))
            .pb(px(if wide { 14. } else { 16. }))
            .child(composer);
        if !mini || !open {
            return fold.into_any_element();
        }
        fold.with_animation(
            SharedString::from(format!("fold-{}", pane.id)),
            Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
            |el, delta| el.max_h(px(220. * delta)).opacity(delta),
        )
        .into_any_element()
    }

    fn composer_menu(
        &self,
        pane: &Pane,
        prefs: &aivory_protocol::ComposerPrefs,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Overlay::Composer(session, menu) = &self.open else {
            return None;
        };
        if session != &pane.id {
            return None;
        }
        let menu = *menu;
        let items: Vec<(String, bool)> = match menu {
            ComposerMenu::Mode => [Mode::Plan, Mode::Edit, Mode::Auto]
                .iter()
                .map(|m| (mode_label(*m).to_string(), *m == prefs.mode))
                .collect(),
            ComposerMenu::Provider => self
                .provider_ids()
                .into_iter()
                .map(|p| {
                    let on = p == prefs.provider;
                    (p, on)
                })
                .collect(),
            ComposerMenu::Model => self
                .models(&prefs.provider)
                .into_iter()
                .map(|m| {
                    let on = m == prefs.model;
                    (m, on)
                })
                .collect(),
            ComposerMenu::Effort => [Effort::Low, Effort::Medium, Effort::High, Effort::Max]
                .iter()
                .map(|e| (effort_label(*e).to_string(), *e == prefs.effort))
                .collect(),
        };
        let left = match menu {
            ComposerMenu::Mode => 34.,
            ComposerMenu::Provider => 84.,
            ComposerMenu::Model => 160.,
            ComposerMenu::Effort => 250.,
        };
        let mut m = ui::menu(150.);
        for (i, (label, on)) in items.into_iter().enumerate() {
            let session = pane.id.clone();
            let value = label.clone();
            m = m.child(ui::menu_item(
                SharedString::from(format!("cmi-{}-{menu:?}-{i}", pane.id)),
                on,
                div().child(label).into_any_element(),
                None,
                Box::new(cx.listener(move |this, _, _, cx| {
                    this.pick_composer(&session, menu, &value, cx)
                })),
            ));
        }
        Some(
            self.overlay(
                div()
                    .absolute()
                    .bottom(relative(1.))
                    .mb(px(6.))
                    .left(px(left))
                    .child(m),
                cx,
            ),
        )
    }
}

/// `waits 6 min`, `idle 12 min`, `error 3 min` or `14 min`.
pub(super) fn meta_text(pane: &Pane, now: aivory_protocol::UnixMs) -> String {
    let minutes = pane.minutes(now);
    match pane.state {
        SessionState::Idle => format!("idle {minutes} min"),
        SessionState::NeedsYou => format!("waits {minutes} min"),
        SessionState::Error => format!("error {minutes} min"),
        _ => format!("{minutes} min"),
    }
}

pub fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Plan => "Plan",
        Mode::Edit => "Edit",
        Mode::Auto => "Auto",
    }
}

pub fn effort_label(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Max => "max",
    }
}

/// The model list per provider. Mock values of the design until providers report models.
pub fn models(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &["opus-5.5", "sonnet-5.5", "haiku-5.5"],
        "codex" => &["gpt-5.2-codex", "gpt-5.2"],
        "gemini" => &["3-pro", "3-flash"],
        "opencode" => &["kimi-k2", "qwen3-coder"],
        "goose" => &["goose-1.9"],
        _ => &[],
    }
}

/// The words of an option in the dialog. The three permission answers read like the
/// dialog of the agent CLIs; the answer that goes to the daemon stays the original text.
pub fn option_label(kind: AskKind, option: &str, command: &str) -> String {
    if kind != AskKind::Permission {
        return option.to_string();
    }
    match option {
        "Allow" => "Yes".to_string(),
        "Always allow" => {
            let word = command.split_whitespace().next().unwrap_or("this");
            format!("Yes, and don't ask again for {word}")
        }
        "Deny" => "No, tell the agent what to do".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_permission_dialog_reads_like_the_agent_cli() {
        let cmd = "sqlx migrate run --database-url $DB";
        assert_eq!(option_label(AskKind::Permission, "Allow", cmd), "Yes");
        assert_eq!(
            option_label(AskKind::Permission, "Always allow", cmd),
            "Yes, and don't ask again for sqlx"
        );
        assert_eq!(
            option_label(AskKind::Permission, "Deny", cmd),
            "No, tell the agent what to do"
        );
        assert_eq!(option_label(AskKind::Question, "Allow", cmd), "Allow");
        assert_eq!(option_label(AskKind::Permission, "Retry", cmd), "Retry");
    }
}
