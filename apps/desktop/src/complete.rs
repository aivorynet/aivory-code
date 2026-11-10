//! Completion in the chat fields of every look: a word that starts with `/` (at the start of
//! the message) lists the slash commands of the agent of the session; a word that starts with
//! `@` lists the files of the codebase and the subagents. The list comes from the daemon
//! (`compose/complete`).
//!
//! A view calls [`attach`] once for its chat field. The popup is one global: the shell draws
//! it over the window ([`popup`]), and one key interceptor ([`init`]) moves the selection
//! (Up, Down), takes an item (Tab, Enter) and closes the list (Esc). A key interceptor of a
//! view that also uses these keys checks [`is_open`] first.

use std::ops::Range;
use std::rc::Rc;

use aivory_protocol::complete::{
    CompleteItem, CompleteItemKind, CompleteList, CompleteParams, word_at,
};
use aivory_protocol::rpc::method;
use gpui::prelude::*;
use gpui::{
    Anchor, AnyElement, App, Bounds, Entity, EntityId, MouseButton, Pixels, Window, anchored,
    deferred, div, point, px, rgb,
};
use gpui_base::input::{InputBaseState, InputEvent, InputModeKind};

use crate::model::Model;
use crate::theme::{T_CAPTION, tk};
use crate::ui;

/// The most rows on view; the list scrolls for more.
const ROWS: usize = 8;
const ROW_H: f32 = 28.;

type Apply = Rc<dyn Fn(&CompleteItem, &mut Window, &mut App)>;
/// The agent and the folder of a chat field without a session.
type Fallback = dyn Fn(&App) -> (Option<String>, Option<String>);

struct Popup {
    owner: EntityId,
    items: Vec<CompleteItem>,
    selected: usize,
    /// The bounds of the chat field: the list opens above it.
    anchor: Bounds<Pixels>,
    apply: Apply,
}

#[derive(Default)]
struct Completion {
    popup: Option<Popup>,
    /// The number of the newest request: an older answer is dropped.
    seq: u64,
}

impl gpui::Global for Completion {}

/// `true` while the list is on view (a view leaves Up, Down, Tab, Enter and Esc to it).
pub fn is_open(cx: &App) -> bool {
    cx.try_global::<Completion>()
        .is_some_and(|c| c.popup.is_some())
}

fn close_for(owner: Option<EntityId>, cx: &mut App) {
    let Some(c) = cx.try_global::<Completion>() else {
        return;
    };
    let mine = c
        .popup
        .as_ref()
        .is_some_and(|p| owner.is_none_or(|o| o == p.owner));
    if mine {
        cx.global_mut::<Completion>().popup = None;
        cx.refresh_windows();
    }
}

/// Close the list.
pub fn close(cx: &mut App) {
    close_for(None, cx);
}

/// Complete in the chat field `input`. `session` gives the session of the field (its agent
/// and its folder); `fallback` gives the agent and the folder when there is no session yet
/// (a launcher). The subscription lives as long as the field.
pub fn attach<M: InputModeKind + 'static>(
    input: &Entity<InputBaseState<M>>,
    model: Entity<Model>,
    session: impl Fn(&App) -> Option<String> + 'static,
    fallback: impl Fn(&App) -> (Option<String>, Option<String>) + 'static,
    cx: &mut App,
) {
    let session = Rc::new(session);
    let fallback = Rc::new(fallback);
    cx.subscribe(input, move |input, event: &InputEvent, cx| match event {
        InputEvent::Change | InputEvent::Focus => {
            changed(&input, &model, session.as_ref(), fallback.as_ref(), cx);
        }
        InputEvent::Blur => close_for(Some(input.entity_id()), cx),
        InputEvent::PressEnter { .. } => {}
    })
    .detach();
}

fn changed<M: InputModeKind + 'static>(
    input: &Entity<InputBaseState<M>>,
    model: &Entity<Model>,
    session: &dyn Fn(&App) -> Option<String>,
    fallback: &Fallback,
    cx: &mut App,
) {
    let owner = input.entity_id();
    let (text, cursor, anchor) = {
        let s = input.read(cx);
        (s.value().to_string(), s.cursor(), s.input_bounds())
    };
    let Some((kind, _, query)) = word_at(&text, cursor) else {
        close_for(Some(owner), cx);
        return;
    };
    if !cx.has_global::<Completion>() {
        cx.set_global(Completion::default());
    }
    let seq = {
        let c = cx.global_mut::<Completion>();
        c.seq += 1;
        c.seq
    };
    let (provider, cwd) = fallback(cx);
    let params = CompleteParams {
        kind,
        query,
        session: session(cx),
        provider,
        cwd,
    };
    let task = model.read(cx).call::<method::ComposeComplete>(params, cx);
    let weak = input.downgrade();
    cx.spawn(async move |cx| {
        let list: CompleteList = task.await.unwrap_or_default();
        cx.update(|cx| {
            if cx.try_global::<Completion>().is_none_or(|c| c.seq != seq) {
                return;
            }
            if list.items.is_empty() {
                close_for(Some(owner), cx);
                return;
            }
            let apply_to = weak.clone();
            let apply: Apply = Rc::new(move |item, window, cx| {
                let _ = apply_to.update(cx, |s, cx| {
                    let text = s.value().to_string();
                    if let Some((_, range, _)) = word_at(&text, s.cursor()) {
                        replace(s, range, &item.insert, window, cx);
                    }
                });
            });
            let keep = cx
                .global::<Completion>()
                .popup
                .as_ref()
                .filter(|p| p.owner == owner)
                .and_then(|p| p.items.get(p.selected))
                .and_then(|old| list.items.iter().position(|i| i.insert == old.insert))
                .unwrap_or(0);
            cx.global_mut::<Completion>().popup = Some(Popup {
                owner,
                items: list.items,
                selected: keep,
                anchor,
                apply,
            });
            cx.refresh_windows();
        });
    })
    .detach();
}

fn replace<M: InputModeKind + 'static>(
    s: &mut InputBaseState<M>,
    range: Range<usize>,
    text: &str,
    window: &mut Window,
    cx: &mut gpui::Context<InputBaseState<M>>,
) {
    s.set_selected_range(range, cx);
    s.replace(text.to_string(), window, cx);
}

/// Take the item at `index`: it replaces the typed word.
fn take(index: usize, window: &mut Window, cx: &mut App) {
    let Some((item, apply)) = cx.try_global::<Completion>().and_then(|c| {
        let p = c.popup.as_ref()?;
        Some((p.items.get(index)?.clone(), p.apply.clone()))
    }) else {
        return;
    };
    cx.global_mut::<Completion>().popup = None;
    apply(&item, window, cx);
    cx.refresh_windows();
}

/// The key interceptor of the list. Call once at start.
pub fn init(cx: &mut App) {
    cx.set_global(Completion::default());
    cx.intercept_keystrokes(|event, window, cx| {
        if !is_open(cx) {
            return;
        }
        let k = &event.keystroke;
        if k.modifiers.modified() {
            return;
        }
        let key = k.key.as_str();
        if !matches!(key, "up" | "down" | "tab" | "enter" | "escape") {
            return;
        }
        cx.stop_propagation();
        // The change waits for the end of this key: the other interceptors of the same key
        // still see the list open and leave the key alone.
        let key = key.to_string();
        window.defer(cx, move |window, cx| {
            let Some(p) = cx.global_mut::<Completion>().popup.as_mut() else {
                return;
            };
            let n = p.items.len();
            match key.as_str() {
                "up" => p.selected = (p.selected + n - 1) % n,
                "down" => p.selected = (p.selected + 1) % n,
                "escape" => {
                    close(cx);
                    return;
                }
                _ => {
                    let index = p.selected;
                    take(index, window, cx);
                    return;
                }
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

fn kind_word(kind: CompleteItemKind) -> &'static str {
    match kind {
        CompleteItemKind::Command => "command",
        CompleteItemKind::Custom => "custom",
        CompleteItemKind::Skill => "skill",
        CompleteItemKind::File => "file",
        CompleteItemKind::Agent => "agent",
    }
}

/// The list over the window, above the chat field that asked for it.
pub fn popup(cx: &App) -> Option<AnyElement> {
    let p = cx.try_global::<Completion>()?.popup.as_ref()?;
    let t = tk();
    let width = f32::from(p.anchor.size.width).clamp(320., 560.);
    let shown = p.items.len().min(ROWS);
    let first = p.selected.saturating_sub(ROWS - 1);
    let rows: Vec<AnyElement> = p
        .items
        .iter()
        .enumerate()
        .skip(first)
        .take(shown)
        .map(|(i, item)| {
            let on = i == p.selected;
            div()
                .id(("complete-row", i))
                .h(px(ROW_H))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(10.))
                .rounded(px(4.))
                .when(on, |d| d.bg(rgb(t.track)))
                .hover(|h| h.bg(rgb(t.menu_hover)))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    take(i, window, cx);
                })
                .child(
                    div()
                        .flex_none()
                        .font_family(t.mono)
                        .text_color(rgb(t.ink))
                        .child(item.label.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(rgb(t.ink_3))
                        .child(item.detail.clone().unwrap_or_default()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(t.muted))
                        .child(kind_word(item.kind)),
                )
                .into_any_element()
        })
        .collect();
    let more = p.items.len().saturating_sub(shown);
    Some(
        deferred(
            anchored()
                .anchor(Anchor::BottomLeft)
                .position(point(p.anchor.left(), p.anchor.top() - px(6.)))
                .snap_to_window_with_margin(px(8.))
                .child(
                    div()
                        .id("complete-popup")
                        .occlude()
                        .w(px(width))
                        .p(px(4.))
                        .flex()
                        .flex_col()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(rgb(t.line))
                        .bg(rgb(t.card))
                        .text_size(ui::ts(T_CAPTION))
                        .font_family(t.sans)
                        .children(rows)
                        .child(div().px(px(10.)).pt(px(4.)).text_color(rgb(t.muted)).child(
                            if more > 0 {
                                format!("{more} more · ↑↓ move · Tab takes · Esc closes")
                            } else {
                                "↑↓ move · Tab takes · Esc closes".to_string()
                            },
                        )),
                ),
        )
        .with_priority(10)
        .into_any_element(),
    )
}
