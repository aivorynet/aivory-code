//! Search (Ctrl K): find a session by title, codebase, part or agent and open its deep dive.

use aivory_protocol::{Pane, SessionState};
use gpui::prelude::*;
use gpui::{
    Context, Div, Entity, FontWeight, KeyBinding, SharedString, Window, actions, div, px, rgb,
};
use gpui_base::input::{InputEvent, InputState};

use super::{Overlay, WarRoomEvent, WarRoomScreen};
use crate::theme::*;
use crate::ui;

actions!(warroom, [ToggleSearch, CloseOverlay]);

const MAX_RESULTS: usize = 12;

/// Register the keys of the war room.
pub(super) fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-k", ToggleSearch, None),
        KeyBinding::new("cmd-k", ToggleSearch, None),
        KeyBinding::new("escape", CloseOverlay, None),
    ]);
}

#[derive(Default)]
pub(super) struct Search {
    pub input: Option<Entity<InputState>>,
    pub _events: Option<gpui::Subscription>,
    /// The picked hit: the arrow keys move it, Enter opens it.
    pub selected: usize,
}

/// The hit after an arrow key: it stops at the first and the last hit.
pub(super) fn move_selection(selected: usize, hits: usize, delta: isize) -> usize {
    if hits == 0 {
        return 0;
    }
    selected.saturating_add_signed(delta).min(hits - 1)
}

/// Sessions whose title, codebase, part or agent contains every word of the query: the ones
/// that need you first, then by state, then newest.
pub(super) fn matches<'a>(sessions: &'a [Pane], query: &str) -> Vec<&'a Pane> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    let mut found: Vec<&Pane> = sessions
        .iter()
        .filter(|s| {
            let hay = format!(
                "{} {} {} {} {}",
                s.title, s.codebase, s.part, s.provider, s.model
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect();
    found.sort_by_key(|s| (rank(s.state), std::cmp::Reverse(s.since)));
    found.truncate(MAX_RESULTS);
    found
}

fn rank(state: SessionState) -> u8 {
    match state {
        SessionState::NeedsYou => 0,
        SessionState::Error => 1,
        SessionState::Working => 2,
        SessionState::Idle => 3,
        SessionState::Parked => 4,
        SessionState::Done => 5,
    }
}

impl WarRoomScreen {
    pub(super) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open == Overlay::Search {
            self.open = Overlay::None;
        } else {
            self.open = Overlay::Search;
            let input = self.search_input(window, cx);
            input.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    fn search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(input) = &self.search.input {
            return input.clone();
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search sessions, codebases, parts, agents")
        });
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, input, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        let query = input.read(cx).value().to_string();
                        let picked = this.war.as_ref().and_then(|w| {
                            let hits = matches(&w.sessions, &query);
                            let at = this.search.selected.min(hits.len().saturating_sub(1));
                            hits.get(at).map(|s| s.id.clone())
                        });
                        if let Some(id) = picked {
                            this.open = Overlay::None;
                            cx.emit(WarRoomEvent::OpenSession(id));
                        }
                    }
                    InputEvent::Change => {
                        this.search.selected = 0;
                        cx.notify()
                    }
                    _ => {}
                },
            );
        self.search.input = Some(input.clone());
        self.search._events = Some(events);
        input
    }

    /// The Search button and, when open, the result popover under it.
    pub(super) fn search_button(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let open = self.open == Overlay::Search;
        // A focused session narrows the header: the search shrinks to its icon.
        let narrow = self.focus_session.is_some();
        let mut wrap = div().relative().child(
            div()
                .id("search")
                .h(px(36.))
                .when(narrow, |d| d.w(px(36.)).justify_center())
                .when(!narrow, |d| d.w(px(200.)).px(px(14.)))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded_full()
                .bg(rgb(tk().track))
                .when(open, |d| d.shadow(ui::ring(tk().ink, 2.)))
                .text_size(crate::ui::ts(crate::theme::T_SMALL))
                .text_color(rgb(tk().ink_3))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(tk().track_2)))
                .tooltip(ui::tip(format!("Search ({SEARCH_KEY})")))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_search(window, cx)))
                .child(ui::icon("icons/search.svg", 14., rgb(tk().ink_3)))
                .when(!narrow, |d| {
                    d.child("Search").child(
                        div()
                            .ml_auto()
                            .font_family(tk().mono)
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .child(SEARCH_KEY),
                    )
                }),
        );
        if open {
            let input = self.search_input(window, cx);
            let query = input.read(cx).value().to_string();
            let results: Vec<Pane> = self
                .war
                .as_ref()
                .map(|w| matches(&w.sessions, &query).into_iter().cloned().collect())
                .unwrap_or_default();
            let mut list = div().flex().flex_col().gap(px(2.));
            if results.is_empty() {
                list = list.child(
                    div()
                        .py(px(10.))
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().ink_3))
                        .child("No session matches."),
                );
            }
            let selected = self.search.selected.min(results.len().saturating_sub(1));
            for (i, s) in results.iter().enumerate() {
                let id = s.id.clone();
                list = list.child(
                    div()
                        .id(("search-hit", i))
                        .h(px(40.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .rounded(tk().r_button)
                        .cursor_pointer()
                        .when(i == selected, |d| d.bg(rgb(tk().menu_hover)))
                        .hover(|st| st.bg(rgb(tk().menu_hover)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open = Overlay::None;
                            cx.emit(WarRoomEvent::OpenSession(id.clone()));
                            cx.notify();
                        }))
                        .child(ui::state_dot(s.state, 8.))
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
                                        .text_size(crate::ui::ts(crate::theme::T_BODY))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(s.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                                        .text_color(rgb(tk().ink_3))
                                        .whitespace_nowrap()
                                        .child(format!(
                                            "{} / {} · {} {}",
                                            s.codebase, s.part, s.provider, s.model
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                                .text_color(rgb(tk().ink_3))
                                .child(SharedString::from(s.state.label())),
                        ),
                );
            }
            let popover = ui::popover(520.)
                .gap(px(10.))
                .child(
                    div()
                        .h(px(36.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .rounded(tk().r_button)
                        .border_1()
                        .border_color(rgb(tk().primary))
                        .text_size(crate::ui::ts(crate::theme::T_BODY))
                        .child(input),
                )
                .child(list);
            wrap = wrap
                .child(self.overlay(div().absolute().top(px(42.)).right_0().child(popover), cx));
        }
        wrap
    }
}

#[cfg(target_os = "macos")]
const SEARCH_KEY: &str = "⌘K";
#[cfg(not(target_os = "macos"))]
const SEARCH_KEY: &str = "Ctrl K";

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::demo::Demo;

    #[test]
    fn the_arrow_keys_stop_at_both_ends() {
        assert_eq!(move_selection(0, 5, -1), 0);
        assert_eq!(move_selection(0, 5, 1), 1);
        assert_eq!(move_selection(4, 5, 1), 4);
        assert_eq!(move_selection(3, 0, 1), 0);
    }

    #[test]
    fn every_word_must_match_and_needs_you_comes_first() {
        let war = Demo::new().war_room;
        let all = matches(&war.sessions, "");
        assert!(all.len() <= MAX_RESULTS);
        assert_eq!(all[0].state, SessionState::NeedsYou);
        let hits = matches(&war.sessions, "billing PAYMENT");
        assert!(!hits.is_empty());
        assert!(
            hits.iter()
                .all(|s| s.codebase == "billing" && s.part == "payment")
        );
        assert!(matches(&war.sessions, "no-such-session-word").is_empty());
    }
}
