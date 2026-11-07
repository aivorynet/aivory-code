//! The overlays of the Lens: the usages list under the header (frame 4) and the "Go to symbol"
//! palette.

use aivory_protocol::code::{CodePosParams, SymbolKind, SymbolsParams};
use aivory_protocol::rpc::method;
use gpui::prelude::*;
use gpui::{
    AnyElement, BoxShadow, Context, Div, FontWeight, HighlightStyle, ScrollHandle, SharedString,
    StyledText, Window, deferred, div, point, px, rgb, rgba,
};
use gpui_base::input::{InputEvent, InputState};

use super::logic::{
    self, OutlineRow, group_places, kind_glyph, name_range, part_of, step, usages_meta,
};
use super::{
    ClaimTone, FileState, LensScreen, Loadable, Palette, ROW_HOVER, RefsState, SELECTED,
    claim_pill_el, error_text, hover_title, part_tag,
};
use crate::radar;
use crate::theme::*;
use crate::ui;

const LINE_NO: u32 = 0xA0A7B5;

/// A row of the palette.
struct Item {
    glyph: &'static str,
    name: String,
    detail: String,
    path: String,
    line: u32,
}

impl LensScreen {
    // ---- Usages ----

    /// "Find usages": the list under the header.
    pub(super) fn open_refs(
        &mut self,
        name: String,
        line: u32,
        col: usize,
        cx: &mut Context<Self>,
    ) {
        let title = match self.hovers.get(&(line, col)) {
            Some(super::HoverEntry::Ready { hover, .. }) => hover_title(hover),
            _ => name,
        };
        self.hover = None;
        self.card_hover = false;
        self.refs = Some(RefsState {
            name: title,
            page: Loadable::Loading,
            selected: 0,
            scroll: ScrollHandle::new(),
            reveal: false,
        });
        let call = self.model.read(cx).call::<method::CodeReferences>(
            CodePosParams {
                codebase: self.codebase.clone(),
                path: self.path.clone(),
                line,
                col: col as u32,
                session: self.session_arg(),
                page: None,
                per_page: Some(200),
            },
            cx,
        );
        let stamp = self.generation;
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                if stamp != this.generation {
                    return;
                }
                if let Some(refs) = &mut this.refs {
                    refs.page = match result {
                        Ok(page) => {
                            this.engine = Some(page.source);
                            Loadable::Ready(page)
                        }
                        Err(e) => Loadable::Failed(error_text(&e)),
                    };
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The places in the order of the list: grouped by file.
    fn refs_flat(&self) -> Vec<usize> {
        match self.refs.as_ref().map(|r| &r.page) {
            Some(Loadable::Ready(page)) => group_places(&page.items)
                .into_iter()
                .flat_map(|(_, ix)| ix)
                .collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn move_refs(&mut self, delta: i32) {
        let len = self.refs_flat().len();
        if let Some(refs) = &mut self.refs {
            refs.selected = step(refs.selected, len, delta);
            refs.reveal = true;
        }
    }

    pub(super) fn open_selected_ref(&mut self, cx: &mut Context<Self>) {
        let flat = self.refs_flat();
        let place = self.refs.as_ref().and_then(|r| {
            let Loadable::Ready(page) = &r.page else {
                return None;
            };
            page.items.get(*flat.get(r.selected)?).cloned()
        });
        if let Some(p) = place {
            self.refs = None;
            self.open_path(&p.path, Some(p.line), cx);
        }
    }

    pub(super) fn refs_overlay(
        &mut self,
        show_tree: bool,
        show_panel: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let flat = self.refs_flat();
        let Some(refs) = &mut self.refs else {
            return div().into_any_element();
        };
        let (title, selected, scroll, reveal) = (
            refs.name.clone(),
            refs.selected,
            refs.scroll.clone(),
            std::mem::take(&mut refs.reveal),
        );
        let mut meta = String::new();
        let mut list = div().flex().flex_col();
        match &refs.page {
            Loadable::Loading => list = list.child(refs_note("Looks for usages…")),
            Loadable::Failed(e) => {
                list = list.child(refs_note(&format!("Cannot find usages: {e}")))
            }
            Loadable::Ready(page) => {
                let groups = group_places(&page.items);
                meta = usages_meta(page.total, groups.len());
                if page.items.is_empty() {
                    list = list.child(refs_note("No usage found."));
                }
                let mut child = 0usize;
                let mut pos = 0usize;
                let mut reveal_child = None;
                for (path, ixs) in &groups {
                    let part = part_of(
                        self.detail.as_ref().map_or(&[], |d| d.files.as_slice()),
                        path,
                    )
                    .map(str::to_string);
                    let claim = radar::claim_pill(
                        self.coord.as_ref().map_or(&[], |c| c.claims.as_slice()),
                        path,
                    );
                    list = list.child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(30.))
                            .px(px(16.))
                            .bg(rgb(tk().page))
                            .font_family(tk().mono)
                            .text_size(crate::ui::ts(crate::theme::T_SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(path.clone())
                            .when_some(part, |d, p| d.child(part_tag(p)))
                            .when_some(claim, |d, (s, state)| {
                                d.child(claim_pill_el(
                                    &format!("claimed by {s}"),
                                    if state == aivory_protocol::coord::ClaimState::Editing {
                                        ClaimTone::Plain
                                    } else {
                                        ClaimTone::Pale
                                    },
                                ))
                            }),
                    );
                    child += 1;
                    for &i in ixs {
                        let p = &page.items[i];
                        let on = pos == selected;
                        if on {
                            reveal_child = Some(child);
                        }
                        let problem = self.problems.as_ref().is_some_and(|d| {
                            d.rows.iter().any(|f| f.path == p.path && f.line == p.line)
                        });
                        let name = p.name.clone().unwrap_or_default();
                        let preview = SharedString::from(p.preview.clone());
                        let spans = name_range(&preview, &name)
                            .map(|r| {
                                vec![(
                                    r,
                                    HighlightStyle {
                                        background_color: Some(ui::hsla(SELECTED << 8 | 0xFF)),
                                        font_weight: Some(FontWeight::MEDIUM),
                                        ..Default::default()
                                    },
                                )]
                            })
                            .unwrap_or_default();
                        let (path, line, at) = (p.path.clone(), p.line, pos);
                        list = list.child(
                            div()
                                .id(("lens-ref", pos))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .h(px(28.))
                                .px(px(16.))
                                .font_family(tk().mono)
                                .text_size(crate::ui::ts(crate::theme::T_SMALL))
                                .cursor_pointer()
                                .when(on, |d| d.bg(rgb(SELECTED)))
                                .when(!on, |d| d.hover(|s| s.bg(rgb(ROW_HOVER))))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(r) = &mut this.refs {
                                        r.selected = at;
                                    }
                                    this.refs = None;
                                    this.open_path(&path, Some(line), cx);
                                }))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(30.))
                                        .flex()
                                        .justify_end()
                                        .text_color(rgb(LINE_NO))
                                        .child(p.line.to_string()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(StyledText::new(preview).with_highlights(spans)),
                                )
                                .when(problem, |d| d.child(ui::dot(rgb(tk().e)))),
                        );
                        child += 1;
                        pos += 1;
                    }
                }
                if reveal && let Some(c) = reveal_child {
                    scroll.scroll_to_item(c);
                }
            }
        }
        let left = if show_tree { 300. } else { 0. };
        let right = if show_panel { 320. } else { 0. };
        let _ = flat;
        div()
            .absolute()
            .top_0()
            .left(px(left))
            .right(px(right))
            .child(deferred(
                div()
                    .flex()
                    .flex_col()
                    .bg(rgb(tk().card))
                    .rounded_bl(tk().r_card)
                    .rounded_br(tk().r_card)
                    .border_1()
                    .border_t_0()
                    .border_color(rgba(tk().line))
                    .shadow(vec![
                        BoxShadow {
                            color: ui::hsla(0x0D0D0E0F),
                            offset: point(px(0.), px(1.)),
                            blur_radius: px(2.),
                            spread_radius: px(0.),
                            inset: false,
                        },
                        BoxShadow {
                            color: ui::hsla(0x0D0D0E2E),
                            offset: point(px(0.), px(16.)),
                            blur_radius: px(40.),
                            spread_radius: px(0.),
                            inset: false,
                        },
                    ])
                    .occlude()
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .h(px(44.))
                            .px(px(16.))
                            .border_b_1()
                            .border_color(rgba(tk().line))
                            .child(
                                div()
                                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("Usages of {title}")),
                            )
                            .child(
                                div()
                                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                                    .text_color(rgb(tk().ink_3))
                                    .child(meta),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id("refs-close")
                                    .size(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .text_size(crate::ui::ts(crate::theme::T_TITLE))
                                    .text_color(rgb(tk().ink_3))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(tk().track)))
                                    .tooltip(ui::tip("Close the references"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refs = None;
                                        cx.notify();
                                    }))
                                    .child("×"),
                            ),
                    )
                    .child(
                        div()
                            .id("refs-list")
                            .max_h(px(520.))
                            .overflow_y_scroll()
                            .track_scroll(&scroll)
                            .child(list),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .h(px(34.))
                            .px(px(16.))
                            .border_t_1()
                            .border_color(rgba(tk().line))
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().muted))
                            .child("↑ ↓ move")
                            .child("Enter opens")
                            .child("Esc closes"),
                    ),
            ))
            .into_any_element()
    }

    // ---- Go to symbol ----

    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Go to symbol"));
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => this.palette_changed(cx),
                    InputEvent::PressEnter { .. } => this.open_palette_selected(cx),
                    _ => {}
                },
            );
        input.update(cx, |s, cx| s.focus(window, cx));
        self.palette = Some(Palette {
            input,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            stamp: 0,
            _events: events,
        });
        self.hover = None;
        cx.notify();
    }

    fn palette_changed(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &mut self.palette else {
            return;
        };
        let query = p.input.read(cx).value().trim().to_string();
        if query == p.query {
            return;
        }
        p.query = query.clone();
        p.stamp += 1;
        p.selected = 0;
        let stamp = p.stamp;
        if query.is_empty() {
            p.results.clear();
            cx.notify();
            return;
        }
        let call = self.model.read(cx).call::<method::CodeSymbols>(
            SymbolsParams {
                codebase: self.codebase.clone(),
                query,
                kind: None,
                session: self.session_arg(),
                page: None,
                per_page: Some(super::SYMBOL_ROWS),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(p) = this.palette.as_mut().filter(|p| p.stamp == stamp) {
                    match result {
                        Ok(page) => p.results = page.items,
                        Err(e) => {
                            eprintln!("aivory: code/symbols failed: {e:#}");
                            p.results.clear();
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// The rows: the symbols found by name, or the outline of the file while the field is empty.
    fn palette_items(&self) -> Vec<Item> {
        let Some(p) = &self.palette else {
            return Vec::new();
        };
        if p.query.is_empty() {
            let FileState::Ready(f) = &self.file else {
                return Vec::new();
            };
            return f
                .outline
                .iter()
                .map(|r: &OutlineRow| Item {
                    glyph: kind_glyph(r.kind),
                    name: r.name.clone(),
                    detail: String::new(),
                    path: self.path.clone(),
                    line: r.line,
                })
                .collect();
        }
        p.results
            .iter()
            .map(|s| Item {
                glyph: symbol_glyph(s.kind),
                name: match &s.container {
                    Some(c) if !c.is_empty() => format!("{c}::{}", s.name),
                    _ => s.name.clone(),
                },
                detail: s.signature.clone(),
                path: s.path.clone(),
                line: s.line,
            })
            .collect()
    }

    pub(super) fn move_palette(&mut self, delta: i32) {
        let len = self.palette_items().len();
        if let Some(p) = &mut self.palette {
            p.selected = step(p.selected, len, delta);
        }
    }

    fn open_palette_selected(&mut self, cx: &mut Context<Self>) {
        let items = self.palette_items();
        let at = self.palette.as_ref().map_or(0, |p| p.selected);
        if let Some(item) = items.get(at) {
            let (path, line) = (item.path.clone(), item.line);
            self.palette = None;
            self.open_path(&path, Some(line), cx);
        }
    }

    pub(super) fn palette_overlay(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let items = self.palette_items();
        let Some(p) = &self.palette else {
            return div().into_any_element();
        };
        let selected = p.selected;
        let input = p.input.clone();
        let empty_query = p.query.is_empty();
        let mut list = div().flex().flex_col().gap(px(2.));
        if items.is_empty() {
            list = list.child(
                div()
                    .py(px(10.))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child(if empty_query {
                        "Type a name."
                    } else {
                        "No symbol matches."
                    }),
            );
        }
        for (i, item) in items.into_iter().take(12).enumerate() {
            let (path, line) = (item.path.clone(), item.line);
            let on = i == selected;
            list = list.child(
                div()
                    .id(("lens-sym", i))
                    .h(px(34.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .rounded(tk().r_button)
                    .cursor_pointer()
                    .when(on, |d| d.bg(rgb(tk().menu_hover)))
                    .when(!on, |d| d.hover(|s| s.bg(rgb(tk().menu_hover))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.palette = None;
                        this.open_path(&path, Some(line), cx);
                    }))
                    .child(
                        div()
                            .w(px(22.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().muted))
                            .child(item.glyph),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(tk().mono)
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .child(item.name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_family(tk().mono)
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(item.detail),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(tk().mono)
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().muted))
                            .child(logic::place_text(&item.path, item.line)),
                    ),
            );
        }
        let popover: Div = ui::popover(560.)
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
        div()
            .absolute()
            .top(px(12.))
            .left(gpui::relative(0.5))
            .ml(px(-280.))
            .child(deferred(popover.occlude().on_mouse_down_out(cx.listener(
                |this, _, _, cx| {
                    this.palette = None;
                    cx.notify();
                },
            ))))
            .into_any_element()
    }
}

fn refs_note(text: &str) -> Div {
    div()
        .flex_none()
        .px(px(16.))
        .py(px(12.))
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .text_color(rgb(tk().muted))
        .child(text.to_string())
}

fn symbol_glyph(kind: SymbolKind) -> &'static str {
    kind_glyph(kind)
}
