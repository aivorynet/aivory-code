//! The New session popover: codebase, part, prompt, provider and model → `session/new`.

use aivory_protocol::rpc::{NewSessionParams, method};
use aivory_protocol::{ComposerPrefs, Effort, Mode};
use gpui::prelude::*;
use gpui::{Context, Div, Entity, FontWeight, SharedString, Window, div, px, relative, rgb, rgba};
use gpui_base::input::{InputEvent, TextareaState};

use super::{Overlay, WarRoomScreen};
use crate::theme::*;
use crate::ui;

#[derive(Default)]
pub(super) struct NewSession {
    pub codebase: Option<String>,
    pub part: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub input: Option<Entity<TextareaState>>,
    pub error: Option<String>,
    pub busy: bool,
    pub _events: Option<gpui::Subscription>,
}

impl WarRoomScreen {
    pub(super) fn new_session_button(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.open == Overlay::NewSession;
        let mut wrap = div().relative().child(
            ui::dark_button("new-session", "New session")
                .on_click(cx.listener(|this, _, _, cx| this.toggle(Overlay::NewSession, cx))),
        );
        if open {
            let popover = self.new_session_popover(window, cx);
            wrap = wrap
                .child(self.overlay(div().absolute().top(px(42.)).right_0().child(popover), cx));
        }
        wrap
    }

    pub(super) fn new_session_popover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let codebases: Vec<(String, Vec<String>)> = self
            .war
            .as_ref()
            .map(|w| {
                w.codebases
                    .iter()
                    .map(|c| (c.id.clone(), c.part_names().map(str::to_string).collect()))
                    .collect()
            })
            .unwrap_or_default();
        let providers = self.provider_ids();
        let ns = &mut self.new_session;
        if ns.codebase.is_none() {
            ns.codebase = codebases.first().map(|(id, _)| id.clone());
        }
        if ns.provider.is_none() {
            ns.provider = providers.first().cloned();
        }
        if ns.input.is_none() {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("What should the agent do?")
                    .submit_on_enter(true)
                    .auto_grow(3, 8)
            });
            let weak = cx.weak_entity();
            crate::complete::attach(
                &input,
                self.model.clone(),
                |_| None,
                move |cx| {
                    weak.upgrade().map_or((None, None), |v| {
                        let v = v.read(cx);
                        let path = v.war.as_ref().and_then(|w| {
                            let id = v.new_session.codebase.as_ref()?;
                            w.codebases
                                .iter()
                                .find(|c| &c.id == id)
                                .map(|c| c.path.clone())
                        });
                        (v.new_session.provider.clone(), path)
                    })
                },
                cx,
            );
            let events =
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { shift: false, .. } = event {
                        this.start_new_session(window, cx);
                    }
                });
            ns.input = Some(input);
            ns._events = Some(events);
        }
        let provider = ns.provider.clone().unwrap_or_default();
        let models = self.models(&provider);
        let ns = &mut self.new_session;
        if ns.model.as_ref().is_none_or(|m| !models.contains(m)) {
            ns.model = models.first().cloned();
        }
        let ns = &self.new_session;
        let parts = codebases
            .iter()
            .find(|(id, _)| Some(id) == ns.codebase.as_ref())
            .map(|(_, parts)| parts.clone())
            .unwrap_or_default();

        let mut cb_chips = div().flex().flex_wrap().gap(px(6.));
        for (i, (id, _)) in codebases.iter().enumerate() {
            let value = id.clone();
            cb_chips = cb_chips.child(
                chip(("ns-cb", i), id.clone(), ns.codebase.as_ref() == Some(id)).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.new_session.codebase = Some(value.clone());
                        this.new_session.part = None;
                        cx.notify();
                    }),
                ),
            );
        }
        let mut part_chips = div().flex().flex_wrap().gap(px(6.)).child(
            chip("ns-part-any", "any part", ns.part.is_none()).on_click(cx.listener(
                |this, _, _, cx| {
                    this.new_session.part = None;
                    cx.notify();
                },
            )),
        );
        for (i, part) in parts.iter().enumerate() {
            let value = part.clone();
            part_chips = part_chips.child(
                chip(("ns-part", i), part.clone(), ns.part.as_ref() == Some(part)).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.new_session.part = Some(value.clone());
                        cx.notify();
                    }),
                ),
            );
        }
        let mut provider_chips = div().flex().flex_wrap().gap(px(6.));
        for (i, p) in providers.iter().enumerate() {
            let value = p.clone();
            provider_chips = provider_chips.child(
                chip_with_dot(
                    ("ns-pv", i),
                    p.clone(),
                    ns.provider.as_ref() == Some(p),
                    provider_color(p),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.new_session.provider = Some(value.clone());
                    this.new_session.model = None;
                    cx.notify();
                })),
            );
        }
        let mut model_chips = div().flex().flex_wrap().gap(px(6.));
        for (i, m) in models.iter().enumerate() {
            let value = m.clone();
            model_chips = model_chips.child(
                chip(("ns-md", i), m.clone(), ns.model.as_ref() == Some(m)).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.new_session.model = Some(value.clone());
                        cx.notify();
                    },
                )),
            );
        }
        let field = |label: &'static str, control: Div| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(div().text_color(rgb(tk().ink_2)).child(label))
                .child(control)
        };
        let input = ns.input.clone().expect("input exists");
        let busy = ns.busy;
        ui::popover(420.)
            .child(div().font_weight(FontWeight::BOLD).child("New session"))
            .when(codebases.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().ink_3))
                        .child("No codebase yet. Start one agent in a repository, or add a repo."),
                )
            })
            .child(field("Codebase", cb_chips))
            .when(!parts.is_empty(), |d| d.child(field("Part", part_chips)))
            .child(field(
                "Prompt",
                div()
                    .min_h(px(64.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(tk().r_button)
                    .border_1()
                    .border_color(rgb(tk().field_line))
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .line_height(relative(1.45))
                    .child(input),
            ))
            .child(field("Provider", provider_chips))
            .when(!models.is_empty(), |d| d.child(field("Model", model_chips)))
            .when_some(ns.error.clone(), |d, e| {
                d.child(
                    div()
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().e_text))
                        .child(e),
                )
            })
            .child(
                div().flex().justify_end().child(
                    div()
                        .id("ns-start")
                        .h(px(32.))
                        .px(px(14.))
                        .flex()
                        .items_center()
                        .rounded(tk().r_button)
                        .bg(rgb(tk().primary))
                        .text_color(rgb(tk().card))
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .when(busy, |d| d.opacity(0.6))
                        .hover(|s| s.bg(rgb(tk().primary_hover)))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_new_session(window, cx)),
                        )
                        .child(if busy { "Starting…" } else { "Start" }),
                ),
            )
    }

    fn start_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ns = &mut self.new_session;
        let Some(input) = ns.input.clone() else {
            return;
        };
        let prompt = input.read(cx).value().trim().to_string();
        let (Some(codebase), Some(provider)) = (ns.codebase.clone(), ns.provider.clone()) else {
            ns.error = Some("Pick a codebase and a provider.".into());
            cx.notify();
            return;
        };
        if prompt.is_empty() || ns.busy {
            return;
        }
        ns.busy = true;
        ns.error = None;
        let params = NewSessionParams {
            codebase,
            part: ns.part.clone(),
            title: None,
            prompt,
            composer: ComposerPrefs {
                mode: Mode::Edit,
                provider,
                model: ns.model.clone().unwrap_or_default(),
                effort: Effort::High,
            },
            task: None,
            attachments: Vec::new(),
        };
        let call = self.model.read(cx).call::<method::SessionNew>(params, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.new_session.busy = false;
                match result {
                    Ok(_) => {
                        input.update(cx, |s, cx| s.clean(window, cx));
                        this.open = Overlay::None;
                    }
                    Err(e) => this.new_session.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// A chip with a provider dot before the label.
fn chip_with_dot(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    dot: u32,
) -> gpui::Stateful<Div> {
    chip_frame(id, on)
        .child(div().size(px(7.)).rounded_full().bg(rgb(dot)))
        .child(label.into())
}

/// A selectable chip (`.fch` of the Display popover).
fn chip(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    on: bool,
) -> gpui::Stateful<Div> {
    chip_frame(id, on).child(label.into())
}

fn chip_frame(id: impl Into<gpui::ElementId>, on: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(6.))
        .border_1()
        .border_color(if on {
            rgb(tk().primary)
        } else {
            rgba(tk().line_2)
        })
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(tk().muted)))
}
