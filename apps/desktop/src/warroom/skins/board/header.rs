//! The header of the Minimal skin: title, dispatch field, view switch, Autopilot and New
//! session. The look switch lives in the status bar (ADR 0022). In the Compact width class a
//! button opens the sidebar as an overlay.

use aivory_protocol::rpc::{NewSessionParams, method};
use aivory_protocol::{ComposerPrefs, Effort, Mode};
use gpui::prelude::*;
use gpui::{Context, Div, Entity, Focusable, FontWeight, Window, div, px, rgb};
use gpui_base::input::{InputEvent, InputState};

use super::{BoardView, WidthClass, parse_dispatch, width_class};
use crate::theme::*;
use crate::ui;
use crate::warroom::skins::SKINS;
use crate::warroom::{Overlay, WarRoomScreen};

impl WarRoomScreen {
    /// The dispatch field: `@codebase task [with provider]`, Enter starts a session.
    fn dispatch_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(input) = &self.board.dispatch {
            return input.clone();
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Dispatch: @codebase add refund tests with codex")
        });
        let events = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let text = input.read(cx).value().to_string();
                    if this.dispatch(&text, cx) {
                        input.update(cx, |s, cx| s.clean(window, cx));
                    }
                }
            },
        );
        self.board.dispatch = Some(input.clone());
        self.board.dispatch_events = Some(events);
        input
    }

    /// Start a session from a dispatch line. Returns true when the daemon got the call.
    fn dispatch(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(war) = self.war.clone() else {
            return false;
        };
        let codebases: Vec<String> = war.codebases.iter().map(|c| c.id.clone()).collect();
        let providers = self.provider_ids();
        let parsed = parse_dispatch(text, &codebases, &providers);
        if parsed.prompt.is_empty() {
            return false;
        }
        let codebase = parsed
            .codebase
            .or_else(|| self.board.module.clone())
            .or_else(|| codebases.first().cloned());
        let provider = parsed.provider.or_else(|| providers.first().cloned());
        let (Some(codebase), Some(provider)) = (codebase, provider) else {
            return false;
        };
        let model = self.models(&provider).first().cloned().unwrap_or_default();
        self.send::<method::SessionNew>(
            NewSessionParams {
                codebase,
                part: None,
                title: None,
                prompt: parsed.prompt,
                composer: ComposerPrefs {
                    mode: Mode::Edit,
                    provider,
                    model,
                    effort: Effort::High,
                },
                task: None,
            },
            cx,
        );
        true
    }

    /// The `Board | Panes` segment.
    pub(in crate::warroom) fn view_switch(&self, cx: &mut Context<Self>) -> Div {
        let view = self.board.view;
        ui::seg_track()
            .child(
                ui::seg_button("view-board", view == BoardView::Board, "Board", false)
                    .tooltip(ui::tip("The orchestration board"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.board.view = BoardView::Board;
                        cx.notify();
                    })),
            )
            .child(
                ui::seg_button("view-panes", view == BoardView::Panes, "Panes", false)
                    .tooltip(ui::tip("The grid of session panes"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.board.view = BoardView::Panes;
                        cx.notify();
                    })),
            )
    }

    /// The skin button and its menu.
    pub(in crate::warroom) fn skin_menu(&self, cx: &mut Context<Self>) -> Div {
        let open = self.open == Overlay::Skin;
        let current = SKINS
            .iter()
            .find(|(s, _)| *s == self.skin())
            .map_or("Skin", |(_, l)| *l);
        let mut wrap = div().relative().child(
            ui::button_lead(
                "skin",
                ui::icon("icons/panel.svg", 14., rgb(tk().ink_2)),
                current,
            )
            .when(open, |d| d.bg(rgb(tk().track_2)))
            .tooltip(ui::tip("Change the look of the war room"))
            .on_click(cx.listener(|this, _, _, cx| this.toggle(Overlay::Skin, cx))),
        );
        if open {
            let mut menu = ui::menu(160.);
            for (i, (skin, label)) in SKINS.iter().enumerate() {
                let skin = *skin;
                menu = menu.child(ui::menu_item(
                    ("skin-item", i),
                    self.skin() == skin,
                    div().child(*label).into_any_element(),
                    None,
                    Box::new(cx.listener(move |this, _, _, cx| {
                        this.open = Overlay::None;
                        this.set_skin(skin, cx);
                    })),
                ));
            }
            // The stream motion toggle lives here, so the header row stays short.
            menu = menu
                .child(div().h(px(1.)).my(px(4.)).bg(rgb(tk().track_2)))
                .child(ui::menu_item(
                    "skin-motion",
                    self.board.motion,
                    div().child("Live motion").into_any_element(),
                    None,
                    Box::new(cx.listener(|this, _, _, cx| {
                        this.open = Overlay::None;
                        this.board.motion = !this.board.motion;
                        cx.notify();
                    })),
                ));
            wrap =
                wrap.child(self.overlay(div().absolute().top(px(42.)).right_0().child(menu), cx));
        }
        wrap
    }

    /// The header of the board (`.hdr`).
    pub(in crate::warroom) fn render_board_header(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let dispatch = self.dispatch_input(window, cx);
        let focused = dispatch.read(cx).focus_handle(cx).is_focused(window);
        let autopilot = self.autopilot_button(cx);
        let new_session = self.new_session_button(window, cx);
        let compact = width_class(f32::from(self.width)) == WidthClass::Compact;
        let side_open = self.board.side_open;
        div()
            .flex_none()
            .h(px(64.))
            .flex()
            .items_center()
            .gap(px(if compact { 10. } else { 14. }))
            .pl(px(if compact { 16. } else { 24. }))
            .pr(px(if compact { 16. } else { 24. }))
            .bg(rgb(tk().board_bg))
            .when(!compact, |d| {
                d.child(
                    div()
                        .flex_none()
                        .font_family(tk().display)
                        .text_size(ui::ts(24.))
                        .font_weight(FontWeight::NORMAL)
                        .whitespace_nowrap()
                        .child("War room"),
                )
            })
            .child(
                div()
                    .flex_1()
                    .max_w(px(520.))
                    .min_w(px(120.))
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pl(px(16.))
                    .pr(px(8.))
                    .rounded_full()
                    .bg(rgb(tk().card))
                    .shadow(if focused {
                        ui::ring(tk().ink, 2.)
                    } else {
                        ui::card_shadow()
                    })
                    .text_size(ui::ts(T_BODY))
                    .child(ui::icon("icons/send.svg", 16., rgb(tk().ink_3)))
                    .child(div().flex_1().min_w(px(0.)).child(dispatch)),
            )
            .child(div().flex_none().ml_auto().child(self.view_switch(cx)))
            .child(div().flex_none().child(autopilot))
            .child(div().flex_none().child(new_session))
            .when(compact, |d| {
                d.child(
                    ui::button_lead(
                        "board-side",
                        ui::icon("icons/panel.svg", 14., rgb(tk().ink_2)),
                        "Overview",
                    )
                    .flex_none()
                    .when(side_open, |d| d.bg(rgb(tk().track_2)))
                    .tooltip(ui::tip("Show the Overview"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.board.side_open = !this.board.side_open;
                        cx.notify();
                    })),
                )
            })
    }
}
