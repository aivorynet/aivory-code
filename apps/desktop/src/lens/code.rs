//! The center of the Lens: the code view with the blame column, the gutter, the heat, and the
//! hover cards; and the diff mode of a timeline point.

use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};

use aivory_protocol::code::{
    BlameTurn, CodeFileParams, CodePosParams, CommitRef, LineRange, RevertParams,
};
use aivory_protocol::rpc::{ForkParams, method};
use gpui::prelude::*;
use gpui::{
    AnyElement, BoxShadow, ClickEvent, Context, Div, FontWeight, HighlightStyle, MouseMoveEvent,
    ScrollStrategy, SharedString, StyledText, UnderlineStyle, deferred, div, point, px, relative,
    rgb, rgba, uniform_list,
};

use super::logic::{
    self, BLAME_COLORS, BlameKind, DiffKind, LINE_H, PointKind, Sev, card_opens_up, doc_text,
    gutter_sev, heat_at, heat_hover, heat_tag, is_symbol_at, line_segments, marks_for_line,
    place_text, signature_segments, word_at,
};
use super::{
    ADDED, CARD_CLOSE, ClaimTone, DiffState, FileState, HEAT, HoverEntry, Hovered, KICK, LINE_NO,
    LensScreen, Loadable, Note, ROW_HOVER, SELECTED, claim_pill_el, error_text, gb_button,
    gb_primary, hover_title, is_invalid_state, lang_label,
};
use crate::theme::*;
use crate::ui;

/// The pointer rests this long on a symbol before the daemon is asked.
const HOVER_DELAY: std::time::Duration = std::time::Duration::from_millis(220);
const BLAME_CARD_W: f32 = 380.;
const DOC_CARD_W: f32 = 420.;
/// The height that a card needs, for the choice between below and above the line.
const BLAME_CARD_H: f32 = 150.;
const DOC_CARD_H: f32 = 190.;

impl LensScreen {
    /// The center column: the code of the file, or the diff of a timeline point.
    pub(super) fn center(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.heat = logic::heat_ranges(self.claims(), &self.path, self.from_session.as_deref());
        let inner = if self.diff.is_some() {
            self.diff_view(cx)
        } else {
            self.file_view(cx)
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .relative()
            .bg(rgb(tk().card))
            .child(inner)
            .into_any_element()
    }

    // ---- File ----

    fn file_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (total, language, truncated, symbols) = match &self.file {
            FileState::Loading => return placeholder("Loads the file from aivoryd…"),
            FileState::Failed(e) => {
                return placeholder(&format!("Cannot open {}: {e}", self.path));
            }
            FileState::Ready(f) => (
                f.lines.len(),
                f.language.clone(),
                f.truncated,
                f.outline.clone(),
            ),
        };
        if let Some(line) = self.pending_jump.take() {
            self.scroll
                .scroll_to_item(line.saturating_sub(1) as usize, ScrollStrategy::Center);
        }
        let scrolled: f32 = self.scroll.0.borrow().base_handle.offset().y.into();
        let top = ((-scrolled).max(0.) / LINE_H) as u32 + 1;
        let at = self.cursor.unwrap_or(top);
        let path = logic::symbol_path(&symbols, at);
        let mut head = div()
            .flex_none()
            .h(px(40.))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .border_b_1()
            .border_color(rgba(tk().line))
            .font_family(tk().mono)
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .whitespace_nowrap()
            .overflow_hidden();
        for (i, name) in path.split(" › ").filter(|n| !n.is_empty()).enumerate() {
            if i > 0 {
                head = head.child(div().text_color(rgb(LINE_NO)).child("›"));
            }
            head = head.child(name.to_string());
        }
        let engine = match self.engine {
            Some(aivory_protocol::code::CodeSource::Lsp) => " · language server ready",
            Some(aivory_protocol::code::CodeSource::Index) => " · symbol index",
            _ => "",
        };
        let more = if truncated { " (cut at 1 MiB)" } else { "" };
        head = head
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(format!(
                        "{} · {} lines{more}{engine}",
                        lang_label(&language),
                        total
                    )),
            )
            .child(div().flex_1())
            .child(legend());
        let list = uniform_list(
            "lens-lines",
            total,
            cx.processor(|this, range: Range<usize>, _, cx| {
                this.ensure_lines(range.clone(), cx);
                range.map(|ix| this.code_line(ix, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full()
        .on_scroll_wheel(cx.listener(|this, _, _, cx| {
            if this.hover.is_some() && !this.card_hover {
                this.hover = None;
                cx.notify();
            }
        }));
        let card = self.card_overlay(cx);
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .relative()
                    .pt(px(6.))
                    .overflow_hidden()
                    .child(list)
                    .when_some(card, |d, c| d.child(c)),
            )
            .into_any_element()
    }

    /// One line: blame column, number, gutter, code.
    fn code_line(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let FileState::Ready(f) = &self.file else {
            return div().into_any_element();
        };
        let Some(text) = f.lines.get(ix).cloned() else {
            return div().into_any_element();
        };
        let n = ix as u32 + 1;
        let tokens = f.tokens.get(ix).cloned().unwrap_or_default();
        let marks = marks_for_line(&f.findings, n, &text);
        let sev = gutter_sev(&f.findings, n);
        let sym_range = self
            .hover
            .as_ref()
            .filter(|h| h.line == n)
            .and_then(|h| h.sym.as_ref())
            .map(|(r, _)| r.clone());
        let highlights: Vec<(Range<usize>, HighlightStyle)> =
            line_segments(&text, &tokens, &marks, sym_range)
                .into_iter()
                .map(|s| {
                    (
                        s.range,
                        HighlightStyle {
                            color: s.color.map(|c| ui::hsla(c << 8 | 0xFF)),
                            background_color: s.bg.then(|| ui::hsla(SELECTED << 8 | 0xFF)),
                            underline: s.under.map(|sev| UnderlineStyle {
                                thickness: px(1.),
                                color: Some(ui::hsla(sev.color() << 8 | 0xFF)),
                                wavy: sev != Sev::Hint,
                            }),
                            ..Default::default()
                        },
                    )
                })
                .collect();
        let styled = StyledText::new(text.clone()).with_highlights(highlights);
        let layout = styled.layout().clone();
        let heat = heat_at(&self.heat, n).cloned();
        let first_of_heat = heat.as_ref().is_some_and(|h| h.from == n);
        let tint = heat.as_ref().map(|h| {
            if h.state == aivory_protocol::coord::ClaimState::Editing {
                0x4B5BDB0D
            } else {
                0x4B5BDB06
            }
        });
        let editing = heat
            .as_ref()
            .is_some_and(|h| h.state == aivory_protocol::coord::ClaimState::Editing);
        let blame = self.blame.color_at(n);
        let tooltip = (self.heat_hover == Some(n))
            .then(|| heat.as_ref().map(heat_hover))
            .flatten();
        let move_text = text.clone();
        div()
            .id(("lens-line", ix))
            .relative()
            .flex()
            .items_center()
            .w_full()
            .h(px(LINE_H))
            .flex_none()
            .font_family(tk().mono)
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .line_height(px(LINE_H))
            .when_some(tint, |d, t| d.bg(rgba(t)))
            .hover(|s| s.bg(rgb(ROW_HOVER)))
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                // The layout exists once the line is painted; the point must be on the text.
                let at = catch_unwind(AssertUnwindSafe(|| {
                    layout
                        .bounds()
                        .contains(&ev.position)
                        .then(|| layout.index_for_position(ev.position).ok())
                        .flatten()
                }))
                .ok()
                .flatten();
                let sym = at.and_then(|ix| {
                    let w = word_at(&move_text, ix)?;
                    is_symbol_at(&tokens, ix).then(|| (w.clone(), move_text[w].to_string()))
                });
                this.set_hover(n, sym, cx);
            }))
            .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                if !*over {
                    this.leave_line(n, cx);
                }
            }))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.click_line(n, ev.modifiers().secondary(), cx)
            }))
            .child(div().flex_none().w(px(4.)).h(px(LINE_H)).bg(rgb(blame)))
            .child(
                div()
                    .flex_none()
                    .w(px(38.))
                    .flex()
                    .justify_end()
                    .pr(px(8.))
                    .text_color(rgb(LINE_NO))
                    .child(n.to_string()),
            )
            .child(
                div()
                    .id(("lens-gutter", ix))
                    .relative()
                    .flex_none()
                    .w(px(24.))
                    .h(px(LINE_H))
                    .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                        let next = over.then_some(n);
                        if this.heat_hover != next {
                            this.heat_hover = next;
                            cx.notify();
                        }
                    }))
                    .when_some(sev, |d, sev| {
                        let dot = div()
                            .absolute()
                            .left(px(3.))
                            .top(px(7.))
                            .size(px(8.))
                            .rounded_full();
                        d.child(if sev == Sev::Hint {
                            dot.border(px(1.5)).border_color(rgb(tk().idle_ring))
                        } else {
                            dot.bg(rgb(sev.color()))
                        })
                    })
                    .when(heat.is_some(), |d| {
                        d.child(
                            div()
                                .absolute()
                                .right(px(2.))
                                .top_0()
                                .w(px(3.))
                                .h(px(LINE_H))
                                .bg(rgb(HEAT))
                                .when(!editing, |b| b.opacity(0.35)),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .pr(px(12.))
                    .whitespace_nowrap()
                    .text_color(rgb(tk().ink))
                    .child(styled),
            )
            .when(first_of_heat, |d| {
                let h = heat.as_ref().expect("heat");
                let tone = if editing {
                    ClaimTone::Plain
                } else {
                    ClaimTone::Pale
                };
                d.child(
                    div()
                        .absolute()
                        .right(px(14.))
                        .top(px(3.))
                        .h(px(16.))
                        .line_height(px(16.))
                        .child(
                            claim_pill_el(&heat_tag(h), tone)
                                .py(px(0.))
                                .h(px(16.))
                                .line_height(px(16.)),
                        ),
                )
            })
            .when_some(tooltip, |d, t| {
                d.child(
                    div()
                        .absolute()
                        .left(px(66.))
                        .top(px(-26.))
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(4.))
                        .bg(rgb(tk().dark))
                        .text_color(rgb(tk().card))
                        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                        .line_height(px(14.))
                        .whitespace_nowrap()
                        .child(t),
                )
            })
            .into_any_element()
    }

    // ---- Hover ----

    fn set_hover(
        &mut self,
        line: u32,
        sym: Option<(Range<usize>, String)>,
        cx: &mut Context<Self>,
    ) {
        // Any move cancels a pending close.
        self.hover_gen += 1;
        let next = Hovered { line, sym };
        if self.hover.as_ref() == Some(&next) {
            return;
        }
        if self.hover.as_ref().is_some_and(|h| h.line != line) {
            self.note = None;
            self.confirm_revert = false;
        }
        if let Some((range, _)) = &next.sym {
            self.ensure_hover(line, range.start, cx);
        }
        self.hover = Some(next);
        cx.notify();
    }

    /// The pointer left a line: close the card after a short delay, unless the pointer is on
    /// the card, a result stays on it, or another line took over.
    fn leave_line(&mut self, line: u32, cx: &mut Context<Self>) {
        let stamp = self.hover_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CARD_CLOSE).await;
            let _ = this.update(cx, |this, cx| {
                let same = this.hover.as_ref().is_some_and(|h| h.line == line);
                if this.hover_gen == stamp && !this.card_hover && this.note.is_none() && same {
                    this.hover = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn card_hovered(&mut self, over: bool, cx: &mut Context<Self>) {
        self.card_hover = over;
        if !over && let Some(h) = &self.hover {
            self.leave_line(h.line, cx);
        }
    }

    fn click_line(&mut self, line: u32, secondary: bool, cx: &mut Context<Self>) {
        self.cursor = Some(line);
        if secondary
            && let Some(h) = self.hover.clone()
            && h.line == line
            && let Some((range, _)) = h.sym
        {
            self.go_to_definition(line, range.start, cx);
        }
        cx.notify();
    }

    /// Ask for the hover of a symbol once the pointer rests on it.
    fn ensure_hover(&mut self, line: u32, col: usize, cx: &mut Context<Self>) {
        if self.hovers.contains_key(&(line, col)) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                let rests = this.hover.as_ref().is_some_and(|h| {
                    h.line == line && h.sym.as_ref().is_some_and(|(r, _)| r.start == col)
                });
                if rests {
                    this.fetch_hover(line, col, cx);
                }
            });
        })
        .detach();
    }

    /// `code/hover` first, then the usages for the counts of the doc card.
    fn fetch_hover(&mut self, line: u32, col: usize, cx: &mut Context<Self>) {
        let key = (line, col);
        if self.hovers.contains_key(&key) {
            return;
        }
        self.hovers.insert(key, HoverEntry::Loading);
        let params = CodePosParams {
            codebase: self.codebase.clone(),
            path: self.path.clone(),
            line,
            col: col as u32,
            session: self.session_arg(),
            page: None,
            per_page: None,
        };
        let hover = self
            .model
            .read(cx)
            .call::<method::CodeHover>(params.clone(), cx);
        let stamp = self.generation;
        cx.spawn(async move |this, cx| {
            let hover = hover.await;
            let refs = this
                .update(cx, |this, cx| {
                    if stamp != this.generation {
                        return None;
                    }
                    match hover {
                        Ok(h) if !h.name.is_empty() => {
                            this.engine = Some(h.source);
                            this.hovers.insert(
                                key,
                                HoverEntry::Ready {
                                    hover: Box::new(h),
                                    usages: None,
                                    sessions: None,
                                },
                            );
                            cx.notify();
                            Some(this.model.read(cx).call::<method::CodeReferences>(
                                CodePosParams {
                                    per_page: Some(200),
                                    ..params
                                },
                                cx,
                            ))
                        }
                        Ok(_) => {
                            this.hovers.insert(key, HoverEntry::Empty);
                            None
                        }
                        Err(e) => {
                            eprintln!("aivory: code/hover failed: {e:#}");
                            this.hovers.insert(key, HoverEntry::Empty);
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            let Some(refs) = refs else { return };
            let refs = refs.await;
            let _ = this.update(cx, |this, cx| {
                let Ok(page) = refs else { return };
                let mut ids: Vec<String> = Vec::new();
                for p in &page.items {
                    for c in logic::claims_on(this.claims(), &p.path) {
                        if !ids.contains(&c.session) {
                            ids.push(c.session.clone());
                        }
                    }
                }
                if let Some(s) = &this.from_session
                    && !ids.contains(s)
                {
                    ids.push(s.clone());
                }
                let sessions = ids.len() as u32;
                if let Some(HoverEntry::Ready {
                    usages,
                    sessions: n,
                    ..
                }) = this.hovers.get_mut(&key)
                {
                    *usages = Some(page.total);
                    *n = (sessions > 0).then_some(sessions);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Jump to the definition of the symbol at `line`, `col` (hover or Cmd-click).
    pub(super) fn go_to_definition(&mut self, line: u32, col: usize, cx: &mut Context<Self>) {
        let known = match self.hovers.get(&(line, col)) {
            Some(HoverEntry::Ready { hover, .. }) => hover.definitions.first().cloned(),
            _ => None,
        };
        if let Some(def) = known {
            self.hover = None;
            self.open_path(&def.path.clone(), Some(def.line), cx);
            return;
        }
        let call = self.model.read(cx).call::<method::CodeDefinition>(
            CodePosParams {
                codebase: self.codebase.clone(),
                path: self.path.clone(),
                line,
                col: col as u32,
                session: self.session_arg(),
                page: None,
                per_page: Some(5),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(page) => {
                    this.engine = Some(page.source);
                    match page.items.first() {
                        Some(def) => {
                            this.hover = None;
                            this.open_path(&def.path.clone(), Some(def.line), cx);
                        }
                        None => {
                            this.note = Some(Note {
                                text: "No definition found.".into(),
                                error: true,
                                open: None,
                            });
                            cx.notify();
                        }
                    }
                }
                Err(e) => {
                    this.note = Some(Note {
                        text: error_text(&e).into(),
                        error: true,
                        open: None,
                    });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ---- Cards ----

    /// The blame card or the doc card of the hovered line, placed under the line (above it when
    /// it does not fit).
    fn card_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let hover = self.hover.clone()?;
        let FileState::Ready(_) = &self.file else {
            return None;
        };
        let doc = match &hover.sym {
            Some((range, _)) => match self.hovers.get(&(hover.line, range.start)) {
                Some(HoverEntry::Ready { .. }) => Some(range.start),
                _ => return None,
            },
            None => None,
        };
        let blame_kind = match self.blame.kind_at(hover.line) {
            BlameKind::Turn(t) => Some(CardKind::Turn(t.clone())),
            BlameKind::Commit(c) => Some(CardKind::Commit(c.clone())),
            BlameKind::Unknown => None,
        };
        let (card, width, height, left) = match (doc, blame_kind) {
            (Some(col), _) => (
                self.doc_card(hover.line, col, cx)?,
                DOC_CARD_W,
                DOC_CARD_H,
                150.,
            ),
            (None, Some(kind)) => (self.blame_card(kind, cx), BLAME_CARD_W, BLAME_CARD_H, 112.),
            (None, None) => return None,
        };
        let _ = width;
        let bounds = self.scroll.0.borrow().base_handle.bounds();
        let offset = self.scroll.0.borrow().base_handle.offset();
        let view_h: f32 = bounds.size.height.into();
        if view_h <= 0. {
            return None;
        }
        let line_top = 6. + f32::from(offset.y) + (hover.line as f32 - 1.) * LINE_H;
        let up = card_opens_up(line_top, view_h + 6., height);
        let shell = div().absolute().left(px(left));
        let shell = if up {
            shell.bottom(px(view_h + 6. - (line_top - 2.)))
        } else {
            shell.top(px(line_top + LINE_H + 2.))
        };
        Some(
            deferred(shell.child(card))
                .with_priority(1)
                .into_any_element(),
        )
    }

    fn card_shell(&self, width: f32, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        div()
            .id("lens-card")
            .w(px(width))
            .py(px(14.))
            .px(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .bg(rgb(tk().card))
            .rounded(tk().r_card)
            .border_1()
            .border_color(rgba(tk().line))
            .shadow(vec![
                BoxShadow {
                    color: ui::hsla(0x0D0D0E0A),
                    offset: point(px(0.), px(1.)),
                    blur_radius: px(2.),
                    spread_radius: px(0.),
                    inset: false,
                },
                BoxShadow {
                    color: ui::hsla(0x0D0D0E24),
                    offset: point(px(0.), px(12.)),
                    blur_radius: px(32.),
                    spread_radius: px(0.),
                    inset: false,
                },
            ])
            .font_family(tk().sans)
            .text_size(TEXT)
            .line_height(relative(1.45))
            .text_color(rgb(tk().ink))
            .occlude()
            .on_hover(cx.listener(|this, over: &bool, _, cx| this.card_hovered(*over, cx)))
    }

    fn note_line(&self, cx: &mut Context<Self>) -> Option<Div> {
        let note = self.note.clone()?;
        let open = note.open.clone();
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(crate::ui::ts(crate::theme::T_SMALL))
                .text_color(rgb(if note.error { tk().e_text } else { ADDED }))
                .child(div().min_w(px(0.)).child(note.text))
                .when_some(open, |d, id| {
                    d.child(
                        div()
                            .id("note-open")
                            .flex_none()
                            .text_color(rgb(tk().link))
                            .cursor_pointer()
                            .child("Open")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(super::LensEvent::OpenSession(id.clone()))
                            })),
                    )
                }),
        )
    }

    fn blame_card(&mut self, kind: CardKind, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        match kind {
            CardKind::Turn(turn) => self.turn_card(&turn, cx),
            CardKind::Commit(commit) => self.commit_card(&commit, cx),
        }
    }

    fn turn_card(&mut self, turn: &BlameTurn, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let kicker = format!("Blame by prompt · turn {} · {}", turn.session, turn.n);
        let mut meta = Vec::new();
        if !turn.model.is_empty() {
            meta.push(turn.model.clone());
        }
        meta.push(format!("session {}", turn.session));
        meta.push(self.clock(turn.time, cx));
        if let Some(c) = turn.cost {
            meta.push(logic::cost_text(c));
        }
        let (session, n) = (turn.session.clone(), turn.n);
        let (fork_session, fork_n) = (session.clone(), n);
        let confirm = self.confirm_revert;
        let revert = gb_button(
            "card-revert",
            if confirm {
                "Confirm revert"
            } else {
                "Revert turn"
            },
            false,
        )
        .text_color(rgb(tk().e_text))
        .on_click(cx.listener(move |this, _, _, cx| this.revert_clicked(&session, n, cx)));
        let fork = gb_button("card-fork", "Fork from this turn", false)
            .on_click(cx.listener(move |this, _, _, cx| this.fork_turn(&fork_session, fork_n, cx)));
        let note = self.note_line(cx);
        self.card_shell(BLAME_CARD_W, cx)
            .child(kicker_text(kicker))
            .child(format!("“{}”", turn.prompt))
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_2))
                    .child(meta.join(" · ")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .mt(px(2.))
                    .child(revert)
                    .child(fork),
            )
            .when_some(note, |d, n| d.child(n))
    }

    fn commit_card(&mut self, commit: &CommitRef, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let id: String = commit.id.chars().take(7).collect();
        let kicker = format!("Blame · commit {id}");
        let when = self.clock(commit.time, cx);
        let target = commit.id.clone();
        // The author comes from the timeline, which lists the commits of the file.
        let author = self
            .timeline
            .iter()
            .find(|p| matches!(&p.kind, PointKind::Commit { id } if same_commit(id, &target)))
            .and_then(|p| p.detail.strip_suffix(" · committed"))
            .map(str::to_string);
        let show = gb_button("card-timeline", "Show in timeline", false).on_click(cx.listener(
            move |this, _, _, cx| {
                this.tab = super::PanelTab::Timeline;
                if let Some(p) = this
                    .timeline
                    .iter()
                    .find(
                        |p| matches!(&p.kind, PointKind::Commit { id } if same_commit(id, &target)),
                    )
                    .cloned()
                {
                    this.show_point(p, cx);
                }
                this.hover = None;
                cx.notify();
            },
        ));
        self.card_shell(BLAME_CARD_W, cx)
            .child(kicker_text(kicker))
            .child(format!("“{}”", commit.summary))
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_2))
                    .child(match author {
                        Some(a) => format!("{a} · {when} · not from an AIVory turn"),
                        None => format!("{when} · not from an AIVory turn"),
                    }),
            )
            .child(div().flex().gap(px(8.)).mt(px(2.)).child(show))
    }

    fn doc_card(
        &mut self,
        line: u32,
        col: usize,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let (title, sig, text, place, usages, sessions) = match self.hovers.get(&(line, col)) {
            Some(HoverEntry::Ready {
                hover,
                usages,
                sessions,
            }) => (
                hover_title(hover),
                hover.signature.clone(),
                doc_text(&hover.text),
                hover
                    .definitions
                    .first()
                    .map(|d| place_text(&d.path, d.line)),
                *usages,
                *sessions,
            ),
            _ => return None,
        };
        let mut facts = Vec::new();
        facts.extend(place);
        if let Some(n) = usages {
            facts.push(format!("{n} usage{}", if n == 1 { "" } else { "s" }));
        }
        if let Some(n) = sessions {
            facts.push(format!("{n} session{}", if n == 1 { "" } else { "s" }));
        }
        let name = self
            .hover
            .as_ref()
            .and_then(|h| h.sym.as_ref())
            .map(|(_, n)| n.clone())
            .unwrap_or_default();
        let note = self.note_line(cx);
        let signature = sig.map(|sig| {
            let spans: Vec<(Range<usize>, HighlightStyle)> = signature_segments(&sig)
                .into_iter()
                .map(|(r, c)| {
                    (
                        r,
                        HighlightStyle {
                            color: Some(ui::hsla(c << 8 | 0xFF)),
                            ..Default::default()
                        },
                    )
                })
                .collect();
            div()
                .font_family(tk().mono)
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .line_height(relative(1.5))
                .child(StyledText::new(SharedString::from(sig)).with_highlights(spans))
        });
        Some(
            self.card_shell(DOC_CARD_W, cx)
                .child(kicker_text(title))
                .when_some(signature, |d, s| d.child(s))
                .when(!text.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .text_color(rgb(tk().ink_2))
                            .child(text),
                    )
                })
                .child(
                    div()
                        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                        .text_color(rgb(tk().ink_2))
                        .child(facts.join(" · ")),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .mt(px(2.))
                        .child(gb_primary("card-def", "Go to definition").on_click(
                            cx.listener(move |this, _, _, cx| this.go_to_definition(line, col, cx)),
                        ))
                        .child(
                            gb_button("card-refs", "Find usages", false).on_click(cx.listener(
                                move |this, _, _, cx| this.open_refs(name.clone(), line, col, cx),
                            )),
                        ),
                )
                .when_some(note, |d, n| d.child(n)),
        )
    }

    // ---- Revert and fork ----

    /// The first click asks for a confirmation; the second reverts.
    pub(super) fn revert_clicked(&mut self, session: &str, n: u32, cx: &mut Context<Self>) {
        if !self.confirm_revert {
            self.confirm_revert = true;
            self.note = None;
            cx.notify();
            return;
        }
        self.confirm_revert = false;
        let call = self.model.read(cx).call::<method::LedgerRevert>(
            RevertParams {
                session: session.to_string(),
                turn: n,
            },
            cx,
        );
        let session = session.to_string();
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(r) => {
                    let note = Note {
                        text: format!(
                            "Reverted turn {n} of {session}: the worktree has turn {} ({}).",
                            r.restored, r.snapshot
                        )
                        .into(),
                        error: false,
                        open: None,
                    };
                    let cursor = this.cursor;
                    this.load_file(cursor, cx);
                    this.tab = super::PanelTab::Timeline;
                    this.note = Some(note);
                    cx.notify();
                }
                Err(e) => {
                    let text = if is_invalid_state(&e) {
                        error_text(&e)
                    } else {
                        format!("Cannot revert: {}", error_text(&e))
                    };
                    this.note = Some(Note {
                        text: text.into(),
                        error: true,
                        open: None,
                    });
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Fork the session at the end of a turn.
    pub(super) fn fork_turn(&mut self, session: &str, n: u32, cx: &mut Context<Self>) {
        let call = self.model.read(cx).call::<method::SessionFork>(
            ForkParams {
                id: session.to_string(),
                at: Some(n.to_string()),
                provider: None,
                prompt: None,
            },
            cx,
        );
        self.note = Some(Note {
            text: "Forks the session…".into(),
            error: false,
            open: None,
        });
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                this.note = Some(match result {
                    Ok(created) => Note {
                        text: format!("Forked as {}.", created.id).into(),
                        error: false,
                        open: Some(created.id),
                    },
                    Err(e) => Note {
                        text: format!("Cannot fork: {}", error_text(&e)).into(),
                        error: true,
                        open: None,
                    },
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ---- Diff mode ----

    /// Show the diff of a timeline point: the file at the end of the turn against the file at
    /// the end of the turn before (or the commit against its parent).
    pub(super) fn show_point(&mut self, point: logic::Point, cx: &mut Context<Self>) {
        let (old_rev, new_rev) = logic::revs_of(&point.kind);
        let session = match &point.kind {
            PointKind::Turn { session, .. } => Some(session.clone()),
            PointKind::Commit { .. } => None,
        };
        let make = |rev: String| CodeFileParams {
            codebase: self.codebase.clone(),
            path: self.path.clone(),
            session: session.clone(),
            rev: Some(rev),
            lines: Some(LineRange { from: 1, to: 1 }),
        };
        let model = self.model.read(cx);
        let old = model.call::<method::CodeFileMethod>(make(old_rev), cx);
        let new = model.call::<method::CodeFileMethod>(make(new_rev), cx);
        let kind = point.kind.clone();
        self.diff = Some(DiffState {
            point,
            rows: Loadable::Loading,
            approx: false,
        });
        self.hover = None;
        cx.spawn(async move |this, cx| {
            let old = old.await;
            let new = new.await;
            let _ = this.update(cx, |this, cx| {
                let Some(state) = this.diff.as_mut().filter(|d| d.point.kind == kind) else {
                    return;
                };
                match new {
                    Ok(new) => {
                        let old_lines =
                            old.map(|f| logic::split_lines(&f.text)).unwrap_or_default();
                        let d = logic::diff_lines(&old_lines, &logic::split_lines(&new.text), 3);
                        state.rows = Loadable::Ready((d.rows, d.added, d.removed));
                    }
                    Err(e) => {
                        // Without a snapshot, show the lines that the turn wrote.
                        let rows = match (&kind, &this.file) {
                            (PointKind::Turn { session, n }, FileState::Ready(f)) => {
                                let lines: Vec<String> =
                                    f.lines.iter().map(|l| l.to_string()).collect();
                                let d = logic::turn_lines(&lines, &this.blame, session, *n, 3);
                                (!d.rows.is_empty()).then_some(d)
                            }
                            _ => None,
                        };
                        match rows {
                            Some(d) => {
                                state.rows = Loadable::Ready((d.rows, d.added, d.removed));
                                state.approx = true;
                            }
                            None => state.rows = Loadable::Failed(error_text(&e)),
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn diff_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = &self.diff else {
            return div().into_any_element();
        };
        let point = state.point.clone();
        let time = self.clock(point.time, cx);
        let (added, removed) = match &state.rows {
            Loadable::Ready((_, a, r)) if !state.approx => (Some(*a), Some(*r)),
            _ => (point.added, point.removed),
        };
        let title = match &point.kind {
            PointKind::Turn { .. } => point.who.clone(),
            PointKind::Commit { .. } => format!("commit {}", point.who),
        };
        let approx = state.approx;
        let mut meta = Vec::new();
        match &point.kind {
            PointKind::Turn { .. } => {
                if !point.model.is_empty() {
                    meta.push(point.model.clone());
                }
                meta.push(time);
                if let Some(c) = point.cost {
                    meta.push(logic::cost_text(c));
                }
                if approx {
                    meta.push("no snapshot: the lines of this turn".to_string());
                }
            }
            PointKind::Commit { .. } => {
                meta.push(point.detail.clone());
                meta.push(time);
            }
        }
        let head = div()
            .flex_none()
            .h(px(40.))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .border_b_1()
            .border_color(rgba(tk().line))
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .whitespace_nowrap()
            .overflow_hidden()
            .child(title)
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(meta.join(" · ")),
            )
            .child(div().flex_1())
            .when_some(added, |d, a| {
                d.child(
                    div()
                        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                        .text_color(rgb(ADDED))
                        .child(format!("+{a}")),
                )
            })
            .when_some(removed, |d, r| {
                d.child(
                    div()
                        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                        .text_color(rgb(tk().e_text))
                        .child(format!("−{r}")),
                )
            });
        let turn = match &point.kind {
            PointKind::Turn { session, n } => Some((session.clone(), *n)),
            PointKind::Commit { .. } => None,
        };
        let confirm = self.confirm_revert;
        let note = self.note_line(cx);
        let mut bar = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(16.))
            .py(px(10.))
            .border_b_1()
            .border_color(rgba(tk().line))
            .bg(rgb(tk().page))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .line_height(relative(1.4))
                    .child(format!("“{}”", point.text)),
            )
            .when_some(note, |d, n| d.child(n));
        if let Some((session, n)) = turn {
            let fork_session = session.clone();
            bar = bar
                .child(
                    gb_button(
                        "diff-revert",
                        if confirm {
                            "Confirm revert"
                        } else {
                            "Revert turn"
                        },
                        true,
                    )
                    .text_color(rgb(tk().e_text))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.revert_clicked(&session, n, cx)),
                    ),
                )
                .child(
                    gb_button("diff-fork", "Fork from this turn", true).on_click(
                        cx.listener(move |this, _, _, cx| this.fork_turn(&fork_session, n, cx)),
                    ),
                );
        }
        bar = bar.child(
            gb_button("diff-back", "Back to file", true).on_click(cx.listener(|this, _, _, cx| {
                this.diff = None;
                this.note = None;
                this.confirm_revert = false;
                cx.notify();
            })),
        );
        let body: AnyElement = match &state.rows {
            Loadable::Loading => placeholder("Loads the two versions of the file…"),
            Loadable::Failed(e) => placeholder(&format!("Cannot build the diff: {e}")),
            Loadable::Ready((rows, _, _)) if rows.is_empty() => {
                placeholder("This point left the lines of the file as they were.")
            }
            Loadable::Ready((rows, _, _)) => {
                let count = rows.len();
                uniform_list(
                    "lens-diff",
                    count,
                    cx.processor(|this, range: Range<usize>, _, _| {
                        range.map(|i| this.diff_row(i)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.diff_scroll)
                .size_full()
                .into_any_element()
            }
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(head)
            .child(bar)
            .child(div().flex_1().min_h(px(0.)).pt(px(6.)).child(body))
            .into_any_element()
    }

    fn diff_row(&self, i: usize) -> AnyElement {
        let Some(DiffState {
            rows: Loadable::Ready((rows, _, _)),
            ..
        }) = &self.diff
        else {
            return div().into_any_element();
        };
        let Some(row) = rows.get(i) else {
            return div().into_any_element();
        };
        let (bg, fg, mark) = match row.kind {
            DiffKind::Hunk => (Some(0xF3F5F8), tk().ink_3, ""),
            DiffKind::Added => (Some(0xF0FAF3), ADDED, "+"),
            DiffKind::Removed => (Some(0xFEF3F3), tk().e_text, "−"),
            DiffKind::Context => (None, tk().ink, ""),
        };
        let number = |n: Option<u32>| {
            div()
                .flex_none()
                .w(px(40.))
                .flex()
                .justify_end()
                .pr(px(8.))
                .text_color(rgb(LINE_NO))
                .child(n.map(|n| n.to_string()).unwrap_or_default())
        };
        div()
            .flex()
            .items_center()
            .w_full()
            .h(px(LINE_H))
            .flex_none()
            .font_family(tk().mono)
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .line_height(px(LINE_H))
            .when_some(bg, |d, c| d.bg(rgb(c)))
            .child(number(row.old))
            .child(number(row.new))
            .child(
                div()
                    .flex_none()
                    .w(px(18.))
                    .flex()
                    .justify_center()
                    .text_color(rgb(if row.kind == DiffKind::Context {
                        tk().muted
                    } else {
                        fg
                    }))
                    .child(mark),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .pr(px(12.))
                    .whitespace_nowrap()
                    .text_color(rgb(fg))
                    .child(row.text.clone()),
            )
            .into_any_element()
    }
}

/// Two commit ids name one commit when one is the start of the other (12 or 40 characters).
fn same_commit(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

enum CardKind {
    Turn(BlameTurn),
    Commit(CommitRef),
}

/// The kicker of a card (`.kick`): mono 9.5 px, bold, caps, blue.
fn kicker_text(text: String) -> Div {
    div()
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(KICK))
        .child(text.to_uppercase())
}

/// The blame legend: five swatches, new to old.
fn legend() -> Div {
    let mut swatches = div().flex().gap(px(2.));
    for c in BLAME_COLORS {
        swatches = swatches.child(div().w(px(12.)).h(px(6.)).rounded(px(2.)).bg(rgb(c)));
    }
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .text_color(rgb(tk().ink_3))
        .child("turn blame")
        .child(swatches)
        .child("new → old")
}

fn placeholder(text: &str) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .px(px(24.))
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .text_color(rgb(tk().muted))
        .child(text.to_string())
        .into_any_element()
}
