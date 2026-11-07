//! The code editor on the right of the Focus view: file tabs, the File / Diff / Split views,
//! breadcrumbs, "after turn n", the CHANGES list, the explorer, the code with line numbers
//! and syntax colors, hunk headers tagged with the turn, and the live caret of the agent.

use std::rc::Rc;
use std::time::Duration;

use aivory_protocol::Pane;
use aivory_protocol::session::{DiffFile, DiffLineKind};
use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt, AnyElement, Context, Div, FontWeight, HighlightStyle, ListOffset,
    MouseButton, SharedString, StyledText, Window, div, list, px, rgb, rgba,
};

use super::chat::CLAUDE;
use super::highlight::{self, Span, Tok};
use super::items::{self, EditRow, Item};
use super::rows::{self, Hunk, Mark, Row, Side};
use super::{CodeRef, FocusEvent, FocusScreen, Jump, Rows, TextFile, View};
use crate::theme::*;
use crate::ui;

fn green() -> u32 {
    crate::theme::pick(0x1A7F37, tk().ok_text)
}
fn green_bg() -> u32 {
    crate::theme::pick(0xEAF7EE, tk().ok_bg)
}
const GREEN_BAR: u32 = 0x2DA44E;
fn red() -> u32 {
    crate::theme::pick(0xCF222E, tk().q_text)
}
fn red_bg() -> u32 {
    crate::theme::pick(0xFDE9EB, tk().q_bg)
}
fn ai_tint() -> u32 {
    crate::theme::pick(0xFFF6F1, tk().e_bg)
}
fn cur_bg() -> u32 {
    crate::theme::pick(0xFFF1E8, tk().e_bg)
}
fn sel_bg() -> u32 {
    crate::theme::pick(0xDDF4FF, tk().work_bg)
}
fn hover_bg() -> u32 {
    crate::theme::pick(0xF6F8FA, tk().menu_hover)
}
fn gutter() -> u32 {
    tk().quiet
}
fn bar_bg() -> u32 {
    crate::theme::pick(0xF6F7F9, tk().track)
}
fn tree_bg() -> u32 {
    crate::theme::pick(0xFBFBFC, tk().card)
}

/// A stamp of a file diff: a change of the file makes a new one.
pub(super) fn stamp(file: &DiffFile) -> u64 {
    (u64::from(file.added) << 40) | (u64::from(file.removed) << 20) | file.lines.len() as u64
}

/// `now`, `14m ago`, `2h ago`.
fn ago(at: aivory_protocol::UnixMs, now: aivory_protocol::UnixMs) -> String {
    let minutes = ((now - at).max(0)) / aivory_protocol::common::MINUTE_MS;
    match minutes {
        0 => "now".to_string(),
        1..=59 => format!("{minutes}m ago"),
        _ => format!("{}h ago", minutes / 60),
    }
}

/// `A` for a file the agent made, `M` for one it changed.
fn status_letter(file: &DiffFile) -> (&'static str, u32) {
    if file.removed == 0
        && file.lines.iter().all(|l| l.kind != DiffLineKind::Context)
        && file.lines.iter().filter_map(|l| l.new).min() == Some(1)
    {
        ("A", green())
    } else {
        ("M", 0x9A6700)
    }
}

fn block_pill(id: &'static str, label: &str, on: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(24.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded_full()
        .text_size(ui::ts(T_CAPTION))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .map(|d| {
            if on {
                d.bg(rgb(tk().card))
                    .text_color(rgb(tk().ink))
                    .shadow(ui::card_shadow())
            } else {
                d.text_color(rgb(tk().ink_2))
            }
        })
        .child(label.to_string())
}

impl FocusScreen {
    /// The file the editor shows: the picked one, else the one the agent writes, else the
    /// first changed file.
    fn current_path(&mut self, changed: &[DiffFile]) -> Option<String> {
        if self.file.is_none() && !changed.is_empty() {
            let live = self.live_edit().and_then(|e| {
                changed
                    .iter()
                    .find(|f| items::same_file(&e.path, &f.path))
                    .map(|f| f.path.clone())
            });
            let path = live.unwrap_or_else(|| changed[0].path.clone());
            self.file = Some(path);
            self.jump = Some(Jump::Hunk(0));
        }
        self.file.clone()
    }

    /// The edit row that the agent writes now.
    fn live_edit(&self) -> Option<EditRow> {
        match self.items.last() {
            Some(Item::Edit(e)) if e.live => Some(e.clone()),
            _ => None,
        }
    }

    /// Jump to the hunk of an edit row of the chat.
    pub(super) fn show_edit(&mut self, edit: &EditRow, cx: &mut Context<Self>) {
        let files: Vec<DiffFile> = self
            .diff
            .as_ref()
            .and_then(|(d, _)| d.as_ref())
            .map(|d| d.files.clone())
            .unwrap_or_default();
        let Some(file) = files.iter().find(|f| items::same_file(&edit.path, &f.path)) else {
            return;
        };
        let hunks = rows::hunks(file, &self.fallback_turns(&file.path));
        let ix = hunks
            .iter()
            .position(|h| h.turn == Some(edit.turn))
            .unwrap_or(0);
        let path = file.path.clone();
        self.open_file(&path, Jump::Hunk(ix), cx);
        self.cursor = ix;
    }

    /// Select the lines `a..=b` of a file; the composer gets a chip for them.
    fn select(&mut self, path: &str, a: u32, b: u32, cx: &mut Context<Self>) {
        let (from, to) = (a.min(b), a.max(b));
        self.sel = Some((path.to_string(), from, to));
        let chip = CodeRef {
            path: path.to_string(),
            from,
            to,
            live: true,
        };
        match self.refs.iter_mut().find(|r| r.live) {
            Some(r) => *r = chip,
            None => self.refs.push(chip),
        }
        cx.notify();
    }

    /// Keep the chip of the selection and let the user type about it.
    fn keep_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for r in &mut self.refs {
            r.live = false;
        }
        self.sel = None;
        self.input.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.refs.retain(|r| !r.live);
        self.sel = None;
        cx.notify();
    }

    /// Scroll the chat to the edit row of a turn.
    fn show_in_chat(&mut self, path: &str, turn: u32, cx: &mut Context<Self>) {
        let ix = self.items.iter().position(
            |i| matches!(i, Item::Edit(e) if e.turn == turn && items::same_file(&e.path, path)),
        );
        if let Some(ix) = ix.or_else(|| {
            self.items
                .iter()
                .position(|i| matches!(i, Item::Edit(e) if e.turn == turn))
        }) {
            self.list.scroll_to(ListOffset {
                item_ix: ix.saturating_sub(1),
                offset_in_item: px(0.),
            });
            cx.notify();
        }
    }

    pub(super) fn editor(
        &mut self,
        pane: &Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let all: Vec<DiffFile> = self
            .diff
            .as_ref()
            .and_then(|(d, _)| d.as_ref())
            .map(|d| d.files.clone())
            .unwrap_or_default();
        let changed = self.changed();
        let path = self.current_path(&changed);
        let live = self.live_edit();
        let frame = div()
            .flex_1()
            .flex_grow(1.7)
            .min_w(px(560.))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(tk().card));
        let Some(path) = path else {
            let text = if self.diff.is_none() {
                "Reads the changes of the session…"
            } else {
                "The agent has not changed a file yet."
            };
            return frame.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(ui::ts(T_BODY))
                    .text_color(rgb(tk().ink_3))
                    .child(text),
            );
        };
        let file = all.iter().find(|f| f.path == path).cloned();
        let st = file.as_ref().map(stamp).unwrap_or(0);
        let turn = self.turn;
        self.ensure_text(&path, turn, st, cx);
        self.build_rows(&path, file.as_ref(), st, live.as_ref(), cx);
        let hunks: Vec<Hunk> = self
            .rows
            .as_ref()
            .map(|r| r.hunks.clone())
            .unwrap_or_default();
        let live_here = live
            .as_ref()
            .filter(|e| items::same_file(&e.path, &path))
            .cloned();
        let caret = live_here.as_ref().and_then(|_| live_line(&hunks));
        let status = self.status_bar(pane, &path, live_here.as_ref(), caret, cx);
        frame
            .child(self.tabs(&changed, &path, cx))
            .child(self.crumbs(&path, &hunks, cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(self.side(&all, &changed, &path, live.as_ref(), cx))
                    .child(self.code(&path, caret, window, cx)),
            )
            .child(status)
    }

    // ---- the data -------------------------------------------------------------------------

    /// Make the rows of the open file in the open view, when something changed.
    fn build_rows(
        &mut self,
        path: &str,
        file: Option<&DiffFile>,
        st: u64,
        _live: Option<&EditRow>,
        cx: &mut Context<Self>,
    ) {
        let text_entry = self.texts.get(&(path.to_string(), self.turn));
        let text = text_entry.and_then(|t| t.file.clone());
        let key = (path.to_string(), self.view, self.turn, st, text.is_some());
        let unchanged = self.rows.as_ref().is_some_and(|r| r.key == key);
        if !unchanged {
            let hunks = file
                .map(|f| rows::hunks(f, &self.fallback_turns(path)))
                .unwrap_or_default();
            let rows_vec = match (self.view, self.turn) {
                (View::Diff, _) => rows::diff_rows(&hunks),
                (View::Split, _) => Vec::new(),
                (View::File, None) => match &text {
                    // A text that does not hold the hunks is older or newer than the diff.
                    Some(t) if hunks.iter().all(|h| h.new_end() as usize <= t.spans.len()) => {
                        rows::file_rows(Some(&t.text), &hunks)
                    }
                    _ if !hunks.is_empty() => rows::diff_rows(&hunks),
                    _ => Vec::new(),
                },
                (View::File, Some(_)) => text
                    .as_ref()
                    .map(|t| rows::plain_rows(&t.text))
                    .unwrap_or_default(),
            };
            let split = if self.view == View::Split {
                rows::split_rows(&hunks)
            } else {
                Vec::new()
            };
            let same_view = self
                .rows
                .as_ref()
                .is_some_and(|r| r.key.0 == key.0 && r.key.1 == key.1 && r.key.2 == key.2);
            let old_n = self.rows.as_ref().map_or(0, |r| r.rows.len());
            if same_view {
                self.code.splice(0..old_n, rows_vec.len());
            } else {
                self.code.reset(rows_vec.len());
            }
            self.rows = Some(Rows {
                key,
                hunks,
                rows: rows_vec,
                split,
                text,
            });
        }
        // Scroll once the rows are there.
        if let Some(jump) = self.jump {
            let Some(r) = self.rows.as_ref() else { return };
            let loading = text_entry.is_some_and(|t| t.busy) && r.rows.is_empty();
            if loading {
                return;
            }
            let ix = match jump {
                Jump::Top => Some(0),
                Jump::Hunk(h) => rows::row_of_hunk(&r.rows, h),
            };
            if let Some(ix) = ix {
                self.code.scroll_to(ListOffset {
                    item_ix: ix.saturating_sub(2),
                    offset_in_item: px(0.),
                });
            }
            self.jump = None;
            cx.notify();
        }
    }

    // ---- the chrome -----------------------------------------------------------------------

    fn tabs(&mut self, changed: &[DiffFile], path: &str, cx: &mut Context<Self>) -> Div {
        let mut bar = div()
            .flex_none()
            .h(px(40.))
            .flex()
            .items_stretch()
            .bg(rgb(bar_bg()))
            .border_b_1()
            .border_color(rgba(tk().line));
        let mut tabs = div()
            .id("focus-tabs")
            .flex_1()
            .min_w(px(0.))
            .flex()
            .overflow_x_scroll();
        let mut names: Vec<&str> = changed.iter().map(|f| f.path.as_str()).collect();
        if !names.contains(&path) {
            names.push(path);
        }
        for (i, p) in names.iter().enumerate() {
            let on = *p == path;
            let target = p.to_string();
            let name = p.rsplit('/').next().unwrap_or(p).to_string();
            let agent = changed.iter().any(|f| f.path == *p);
            tabs =
                tabs.child(
                    div()
                        .id(("focus-tab", i))
                        .flex_none()
                        .px(px(14.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(ui::ts(T_SMALL))
                        .border_r_1()
                        .border_color(rgba(tk().line))
                        .cursor_pointer()
                        .tooltip(ui::tip(p.to_string()))
                        .map(|d| {
                            if on {
                                d.bg(rgb(tk().card))
                                    .text_color(rgb(tk().ink))
                                    .border_t_2()
                                    .border_color(rgb(tk().ink))
                            } else {
                                d.text_color(rgb(tk().ink_3))
                                    .hover(|s| s.text_color(rgb(tk().ink)))
                            }
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_file(&target, Jump::Hunk(0), cx)
                        }))
                        .child(
                            div()
                                .font_family(tk().mono)
                                .text_size(ui::ts(12.5))
                                .child(name),
                        )
                        .when(agent, |d| {
                            d.child(
                                div()
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(rgb(CLAUDE))
                                    .into_any_element(),
                            )
                        }),
                );
        }
        bar = bar.child(tabs);
        let mut seg = div()
            .flex_none()
            .my(px(5.))
            .p(px(3.))
            .flex()
            .gap(px(2.))
            .rounded_full()
            .bg(rgb(tk().track_2));
        for (id, label, view) in [
            ("focus-v-file", "File", View::File),
            ("focus-v-diff", "Diff", View::Diff),
            ("focus-v-split", "Split", View::Split),
        ] {
            seg = seg.child(
                block_pill(id, label, self.view == view).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.view = view;
                        this.jump = Some(Jump::Hunk(this.cursor));
                        cx.notify();
                    },
                )),
            );
        }
        let (session, p) = (self.session.clone(), path.to_string());
        bar.child(seg).child(
            div().flex_none().mx(px(10.)).my(px(5.)).child(
                ui::button_sm("focus-ide", "Open in IDE", false)
                    .tooltip(ui::tip("Open the file in the code lens"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(FocusEvent::OpenFile {
                            session: session.clone(),
                            path: p.clone(),
                        })
                    })),
            ),
        )
    }

    fn crumbs(&mut self, path: &str, hunks: &[Hunk], cx: &mut Context<Self>) -> Div {
        let parts: Vec<&str> = path.split('/').collect();
        let mut row = div()
            .flex_none()
            .h(px(32.))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(ui::ts(T_CAPTION))
            .text_color(rgb(tk().ink_3))
            .border_b_1()
            .border_color(rgba(tk().line));
        let mut crumbs = div()
            .min_w(px(0.))
            .flex()
            .items_center()
            .gap(px(6.))
            .overflow_hidden();
        for (i, p) in parts.iter().enumerate() {
            if i + 1 == parts.len() {
                crumbs = crumbs.child(
                    div()
                        .flex_none()
                        .text_color(rgb(tk().ink))
                        .font_weight(FontWeight::MEDIUM)
                        .child(p.to_string()),
                );
            } else {
                crumbs = crumbs
                    .child(div().flex_none().child(p.to_string()))
                    .child(div().flex_none().child("›"));
            }
        }
        row = row.child(crumbs).child(div().flex_1());
        let turns = rows::turns_of(hunks);
        if !turns.is_empty() {
            let latest = *turns.last().expect("a turn");
            let shown = self.turn.unwrap_or(latest);
            let mut group = div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(3.))
                .child(div().mr(px(4.)).child("after turn"));
            for n in turns {
                let on = n == shown;
                group = group.child(
                    div()
                        .id(("focus-turn", n as usize))
                        .w(px(24.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .font_family(tk().mono)
                        .text_size(ui::ts(11.))
                        .cursor_pointer()
                        .map(|d| {
                            if on {
                                d.bg(rgb(CLAUDE)).text_color(rgb(0xFFFFFF))
                            } else if n < shown {
                                d.text_color(rgb(tk().ink)).hover(|s| s.bg(rgb(tk().track)))
                            } else {
                                d.hover(|s| s.bg(rgb(tk().track)))
                            }
                        })
                        .tooltip(ui::tip(if n == latest {
                            "The file now".to_string()
                        } else {
                            format!("The file after turn {n}")
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.turn = if n == latest { None } else { Some(n) };
                            this.jump = Some(Jump::Top);
                            cx.notify();
                        }))
                        .child(n.to_string()),
                );
            }
            row = row.child(group);
        }
        row
    }

    /// The left column: CHANGES and the explorer.
    fn side(
        &mut self,
        all: &[DiffFile],
        changed: &[DiffFile],
        path: &str,
        live: Option<&EditRow>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let (add, del) = changed
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.added, d + f.removed));
        let head = |label: &'static str| {
            div()
                .px(px(8.))
                .py(px(4.))
                .flex()
                .items_center()
                .justify_between()
                .text_size(ui::ts(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(tk().ink_3))
                .child(label)
        };
        let mut col = div()
            .id("focus-tree")
            .flex_none()
            .w(px(212.))
            .h_full()
            .overflow_y_scroll()
            .px(px(6.))
            .py(px(10.))
            .bg(rgb(tree_bg()))
            .border_r_1()
            .border_color(rgba(tk().line))
            .text_size(ui::ts(12.5))
            .child(
                head("CHANGES").child(
                    div()
                        .font_family(tk().mono)
                        .flex()
                        .gap(px(6.))
                        .child(div().text_color(rgb(green())).child(format!("+{add}")))
                        .child(div().text_color(rgb(red())).child(format!("−{del}"))),
                ),
            );
        for (i, f) in changed.iter().enumerate() {
            let on = f.path == path;
            let (letter, letter_color) = status_letter(f);
            let target = f.path.clone();
            let is_live = live.is_some_and(|e| items::same_file(&e.path, &f.path));
            col =
                col.child(
                    tree_row(("focus-ch", i), on)
                        .tooltip(ui::tip(f.path.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_file(&target, Jump::Hunk(0), cx)
                        }))
                        .child(
                            div()
                                .flex_none()
                                .w(px(10.))
                                .font_family(tk().mono)
                                .text_size(ui::ts(11.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(letter_color))
                                .child(letter),
                        )
                        .child(name_cell(f.path.rsplit('/').next().unwrap_or(&f.path)))
                        .when(is_live, |d| d.child(live_dot(("focus-live", i))))
                        .child(
                            div()
                                .flex_none()
                                .font_family(tk().mono)
                                .text_size(ui::ts(11.))
                                .text_color(rgb(tk().ink_3))
                                .child(format!("+{} −{}", f.added, f.removed)),
                        ),
                );
        }
        col = col.child(head("EXPLORER").mt(px(12.)));
        let paths: Vec<String> = all.iter().map(|f| f.path.clone()).collect();
        for (i, t) in rows::tree(&paths).into_iter().enumerate() {
            let indent = px(8. + 12. * t.depth as f32);
            match t.path {
                None => {
                    col = col.child(
                        div()
                            .h(px(26.))
                            .pl(indent)
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_color(rgb(tk().ink_2))
                            .child(div().flex_none().text_color(rgb(tk().ink_3)).child("▾"))
                            .child(name_cell(&t.name).font_family(tk().sans)),
                    );
                }
                Some(p) => {
                    let on = p == path;
                    let diff = all.iter().find(|f| f.path == p);
                    let touched = diff.is_some_and(|f| !f.lines.is_empty());
                    let is_live = live.is_some_and(|e| items::same_file(&e.path, &p));
                    let target = p.clone();
                    col = col.child(
                        tree_row(("focus-ex", i), on)
                            .pl(indent)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_file(&target, Jump::Hunk(0), cx)
                            }))
                            .child(name_cell(&t.name))
                            .when(is_live, |d| d.child(live_dot(("focus-exlive", i))))
                            .when(touched, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .font_family(tk().mono)
                                        .text_size(ui::ts(11.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(0x9A6700))
                                        .child("M"),
                                )
                            }),
                    );
                }
            }
        }
        col
    }

    fn status_bar(
        &self,
        pane: &Pane,
        path: &str,
        live: Option<&EditRow>,
        caret: Option<u32>,
        cx: &gpui::App,
    ) -> Div {
        let lang = highlight::lang_of(path);
        let branch = self
            .war(cx)
            .and_then(|w| w.codebase(&pane.codebase))
            .map(|c| c.branch.clone())
            .unwrap_or_default();
        let line = self.sel.as_ref().map(|(_, a, _)| *a).or(caret).unwrap_or(1);
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        div()
            .flex_none()
            .h(px(26.))
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(16.))
            .bg(rgb(bar_bg()))
            .border_t_1()
            .border_color(rgba(tk().line))
            .font_family(tk().mono)
            .text_size(ui::ts(T_CAPTION))
            .text_color(rgb(tk().ink_3))
            .whitespace_nowrap()
            .overflow_hidden()
            .when_some(live, |d, _| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_color(rgb(0x9A3412))
                        .child(live_dot(("focus-status-live", 0)))
                        .child(format!(
                            "{} edits {name}{}",
                            pane.provider,
                            caret.map(|l| format!(":{l}")).unwrap_or_default()
                        )),
                )
            })
            .child(format!("Ln {line}"))
            .child("UTF-8")
            .child("LF")
            .child(lang.name())
            .child(div().flex_1())
            .child(branch)
    }

    // ---- the code -------------------------------------------------------------------------

    fn code(
        &mut self,
        path: &str,
        caret: Option<u32>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let wrap = div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .relative()
            .bg(rgb(tk().card));
        let Some(rows) = self.rows.as_ref() else {
            return wrap;
        };
        let empty = if self.view == View::Split {
            rows.split.is_empty()
        } else {
            rows.rows.is_empty()
        };
        if empty {
            let busy = self
                .texts
                .get(&(path.to_string(), self.turn))
                .is_some_and(|t| t.busy || t.file.is_none() && !t.failed);
            return wrap.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child(if busy {
                        "Reads the file…"
                    } else {
                        "No lines to show."
                    }),
            );
        }
        let body: AnyElement = if self.view == View::Split {
            self.split_view(path, cx)
        } else {
            let path = path.to_string();
            list(
                self.code.clone(),
                cx.processor(move |this, ix: usize, _, cx| this.code_row(&path, ix, caret, cx)),
            )
            .size_full()
            .into_any_element()
        };
        let mut wrap = wrap
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .child(div().size_full().pt(px(6.)).child(body));
        if let Some((_, a, b)) = self.sel.clone() {
            let text = if a == b {
                format!("Line {a} selected")
            } else {
                format!("Lines {a}–{b} selected")
            };
            wrap = wrap.child(
                div()
                    .absolute()
                    .top(px(8.))
                    .left(px(64.))
                    .pl(px(12.))
                    .pr(px(6.))
                    .py(px(5.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded_full()
                    .bg(rgb(tk().dark))
                    .text_color(rgb(0xFFFFFF))
                    .text_size(ui::ts(T_CAPTION))
                    .shadow(ui::popover_shadow())
                    .child(text)
                    .child(
                        div()
                            .id("focus-sel-ask")
                            .h(px(24.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .bg(rgb(crate::theme::pick(0xFFFFFF, tk().track)))
                            .text_color(rgb(tk().ink))
                            .font_weight(FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.keep_selection(window, cx)),
                            )
                            .child("Ask agent about this"),
                    )
                    .child(
                        div()
                            .id("focus-sel-clear")
                            .h(px(24.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .bg(rgba(0xFFFFFF26))
                            .font_weight(FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.clear_selection(cx)))
                            .child("Clear"),
                    ),
            );
        }
        wrap
    }

    /// The runs of colors of a line of code.
    fn runs(
        &self,
        text: &str,
        no: Option<u32>,
        mark: Mark,
        file: Option<&Rc<TextFile>>,
        lang: highlight::Lang,
    ) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
        let precomputed = match (no, file) {
            (Some(n), Some(f)) if mark != Mark::Removed => {
                f.spans.get((n as usize).wrapping_sub(1))
            }
            _ => None,
        };
        let owned;
        let spans: &[Span] = match precomputed {
            Some(s) => s,
            None => {
                owned = highlight::lex(lang, text)
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                &owned
            }
        };
        spans
            .iter()
            .filter(|s| {
                s.end <= text.len()
                    && text.is_char_boundary(s.start)
                    && text.is_char_boundary(s.end)
            })
            .map(|s| {
                (
                    s.start..s.end,
                    HighlightStyle {
                        color: Some(ui::hsla((highlight::color(s.tok) << 8) | 0xFF)),
                        font_weight: matches!(s.tok, Tok::Macro | Tok::Heading)
                            .then_some(FontWeight::MEDIUM),
                        font_style: (s.tok == Tok::Comment).then_some(gpui::FontStyle::Italic),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    fn code_row(
        &mut self,
        path: &str,
        ix: usize,
        caret: Option<u32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(rows) = self.rows.as_ref() else {
            return div().into_any_element();
        };
        let Some(row) = rows.rows.get(ix).cloned() else {
            return div().h(px(40.)).into_any_element();
        };
        let lang = highlight::lang_of(path);
        let line_h = px(20.);
        match row {
            Row::Fold(n) => div()
                .w_full()
                .h(ui::ts(24.))
                .my(px(2.))
                .pl(px(64.))
                .flex()
                .items_center()
                .gap(px(8.))
                .bg(rgb(hover_bg()))
                .text_size(ui::ts(T_CAPTION))
                .text_color(rgb(tk().ink_3))
                .child("⋯")
                .child(format!(
                    "{n} unchanged line{}",
                    if n == 1 { "" } else { "s" }
                ))
                .into_any_element(),
            Row::Hunk(h) => {
                let Some(hunk) = rows.hunks.get(h).cloned() else {
                    return div().into_any_element();
                };
                self.hunk_row(path, ix, h, &hunk, cx)
            }
            Row::Line {
                no,
                text,
                mark,
                hunk,
            } => {
                let file = rows.text.clone();
                let runs = self.runs(&text, no, mark, file.as_ref(), lang);
                let selected = no.is_some_and(|n| {
                    self.sel
                        .as_ref()
                        .is_some_and(|(p, a, b)| p == path && n >= *a && n <= *b)
                });
                let is_caret = no.is_some() && no == caret;
                let reviewed = hunk.is_some_and(|h| self.reviewed.contains(&(path.to_string(), h)));
                let (bg, bar) = match mark {
                    Mark::Added => (
                        if self.view == View::Diff {
                            green_bg()
                        } else {
                            ai_tint()
                        },
                        Some(GREEN_BAR),
                    ),
                    Mark::Removed => (red_bg(), Some(red())),
                    Mark::Context => (tk().card, None),
                };
                let bg = if selected {
                    sel_bg()
                } else if is_caret {
                    cur_bg()
                } else if reviewed && mark == Mark::Added {
                    tk().card
                } else {
                    bg
                };
                let target = path.to_string();
                let text_el = if text.is_empty() {
                    div().child(" ").into_any_element()
                } else {
                    StyledText::new(text.clone())
                        .with_highlights(runs)
                        .into_any_element()
                };
                let mut line = div()
                    .id(("focus-line", ix))
                    .w_full()
                    .h(line_h)
                    .flex()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .bg(rgb(bg))
                    .font_family(tk().mono)
                    .text_size(px(13.))
                    .line_height(line_h)
                    .when(bg == tk().card, |d| d.hover(|s| s.bg(rgb(hover_bg()))))
                    .when(mark == Mark::Removed, |d| {
                        d.text_color(rgb(0x82071E)).opacity(0.85)
                    });
                if let Some(n) = no {
                    let t2 = target.clone();
                    line = line.on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.dragging
                            && let Some((p, a, _)) = this.sel.clone()
                            && p == t2
                        {
                            this.select(&t2, a, n, cx);
                        }
                    }));
                }
                let no_cell = div()
                    .flex_none()
                    .w(px(48.))
                    .pr(px(6.))
                    .text_color(rgb(gutter()))
                    .flex()
                    .justify_end()
                    .child(no.map(|n| n.to_string()).unwrap_or_default());
                let no_cell = match no {
                    Some(n) => no_cell
                        .id(("focus-no", ix))
                        .cursor_pointer()
                        .hover(|s| s.text_color(rgb(tk().ink)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                let anchor = match &this.sel {
                                    Some((p, a, _)) if ev.modifiers.shift && p == &target => *a,
                                    _ => n,
                                };
                                this.dragging = true;
                                this.select(&target, anchor, n, cx);
                            }),
                        )
                        .into_any_element(),
                    None => no_cell.into_any_element(),
                };
                line = line
                    .child(no_cell)
                    .child(
                        div()
                            .flex_none()
                            .w(px(4.))
                            .mr(px(12.))
                            .when_some(bar, |d, c| d.bg(rgb(c))),
                    )
                    .child(div().pr(px(24.)).child(text_el));
                if is_caret {
                    line = line.child(caret_el());
                }
                line.into_any_element()
            }
        }
    }

    fn hunk_row(
        &mut self,
        path: &str,
        ix: usize,
        h: usize,
        hunk: &Hunk,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let reviewed = self.reviewed.contains(&(path.to_string(), h));
        let now = self.war(cx).map(|w| w.now);
        let mut label = match hunk.turn {
            Some(t) => format!("turn {t}"),
            None => "agent change".to_string(),
        };
        if let (Some(at), Some(now)) = (hunk.at, now) {
            label.push_str(&format!(" · {}", ago(at, now)));
        }
        label.push_str(&format!(" · +{} −{}", hunk.added, hunk.removed));
        if reviewed {
            label.push_str(" · reviewed");
        }
        let (p1, p2, p3) = (path.to_string(), path.to_string(), path.to_string());
        let (from, to) = (hunk.new_start, hunk.new_end());
        let turn = hunk.turn;
        let group: SharedString = format!("hk-{ix}").into();
        let btn = |id: &'static str, label: String, dark: bool| {
            div()
                .id((id, ix))
                .h(px(20.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded_full()
                .text_size(ui::ts(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .map(|d| {
                    if dark {
                        d.bg(rgb(tk().dark))
                            .text_color(rgb(0xFFFFFF))
                            .hover(|s| s.bg(rgb(tk().dark_hover)))
                    } else {
                        d.bg(rgb(tk().track)).hover(|s| s.bg(rgb(tk().track_2)))
                    }
                })
                .child(label)
        };
        let mut actions = div()
            .flex()
            .gap(px(4.))
            .opacity(0.4)
            .group_hover(group.clone(), |s| s.opacity(1.))
            .child(
                btn(
                    "hk-accept",
                    if reviewed {
                        "Undo review".into()
                    } else {
                        "Accept".into()
                    },
                    !reviewed,
                )
                .tooltip(ui::tip("Mark the hunk as reviewed"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let key = (p1.clone(), h);
                    if !this.reviewed.remove(&key) {
                        this.reviewed.insert(key);
                    }
                    cx.notify();
                })),
            )
            .child(
                btn("hk-ask", "Ask agent".into(), false)
                    .tooltip(ui::tip("Put the lines of the hunk in the composer"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.refs.retain(|r| r.live);
                        this.refs.push(CodeRef {
                            path: p2.clone(),
                            from,
                            to,
                            live: false,
                        });
                        this.input.update(cx, |s, cx| s.focus(window, cx));
                        cx.notify();
                    })),
            );
        if let Some(t) = turn {
            actions =
                actions
                    .child(btn("hk-chat", format!("Turn {t} in chat"), false).on_click(
                        cx.listener(move |this, _, _, cx| this.show_in_chat(&p3, t, cx)),
                    ));
        }
        div()
            .group(group)
            .w_full()
            .h(ui::ts(26.))
            .pl(px(64.))
            .pr(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(ui::ts(T_CAPTION))
            .text_color(rgb(tk().ink_3))
            .child(
                div()
                    .h(px(20.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded_full()
                    .font_weight(FontWeight::MEDIUM)
                    .map(|d| {
                        if reviewed {
                            d.bg(rgb(tk().track)).text_color(rgb(tk().ink_3))
                        } else {
                            d.bg(rgb(cur_bg())).text_color(rgb(0x9A3412))
                        }
                    })
                    .child(div().size(px(6.)).rounded_full().bg(rgb(if reviewed {
                        tk().ink_3
                    } else {
                        CLAUDE
                    })))
                    .child(label),
            )
            .child(actions)
            .into_any_element()
    }

    /// The old and the new side of the hunks.
    fn split_view(&mut self, path: &str, _cx: &mut Context<Self>) -> AnyElement {
        let Some(rows) = self.rows.as_ref() else {
            return div().into_any_element();
        };
        let lang = highlight::lang_of(path);
        let turn = rows.hunks.iter().filter_map(|h| h.turn).max();
        let side_el = |(no, text, side): &(Option<u32>, String, Side)| {
            let bg = match side {
                Side::Added => Some(green_bg()),
                Side::Removed => Some(red_bg()),
                Side::Blank => Some(bar_bg()),
                Side::Context => None,
            };
            let runs: Vec<_> = highlight::lex(lang, text)
                .into_iter()
                .next()
                .unwrap_or_default()
                .into_iter()
                .filter(|s| s.end <= text.len())
                .map(|s| {
                    (
                        s.start..s.end,
                        HighlightStyle {
                            color: Some(ui::hsla((highlight::color(s.tok) << 8) | 0xFF)),
                            ..Default::default()
                        },
                    )
                })
                .collect();
            div()
                .flex_1()
                .min_w(px(0.))
                .h(px(20.))
                .flex()
                .whitespace_nowrap()
                .overflow_hidden()
                .when_some(bg, |d, c| d.bg(rgb(c)))
                .child(
                    div()
                        .flex_none()
                        .w(px(40.))
                        .pr(px(10.))
                        .flex()
                        .justify_end()
                        .text_color(rgb(gutter()))
                        .child(no.map(|n| n.to_string()).unwrap_or_default()),
                )
                .child(
                    StyledText::new(if text.is_empty() {
                        " ".to_string()
                    } else {
                        text.clone()
                    })
                    .with_highlights(runs),
                )
        };
        let head = |label: String| {
            div()
                .flex_1()
                .pl(px(50.))
                .pb(px(4.))
                .text_size(ui::ts(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(tk().ink_3))
                .child(label)
        };
        let mut col = div()
            .id("focus-split")
            .size_full()
            .overflow_y_scroll()
            .font_family(tk().mono)
            .text_size(px(12.5))
            .pb(px(40.))
            .child(
                div()
                    .flex()
                    .child(head("BEFORE".into()))
                    .child(head(match turn {
                        Some(t) => format!("AFTER TURN {t}"),
                        None => "AFTER".into(),
                    })),
            );
        for r in &rows.split {
            col = col.child(
                div()
                    .flex()
                    .child(side_el(&r.left))
                    .child(div().flex_none().w(px(1.)).bg(rgba(tk().line)))
                    .child(side_el(&r.right)),
            );
        }
        col.into_any_element()
    }
}

/// The line the agent writes: the last added line of the last hunk.
fn live_line(hunks: &[Hunk]) -> Option<u32> {
    hunks
        .last()?
        .lines
        .iter()
        .rev()
        .find(|l| l.kind == DiffLineKind::Added)
        .and_then(|l| l.new)
}

/// The blinking caret and the flag of the agent at the end of a line.
fn caret_el() -> Div {
    div()
        .flex()
        .items_center()
        .child(div().w(px(2.)).h(px(16.)).bg(rgb(CLAUDE)).with_animation(
            "focus-caret",
            Animation::new(Duration::from_millis(1000)).repeat(),
            |el, t| el.opacity(if t < 0.5 { 1. } else { 0. }),
        ))
        .child(
            div()
                .ml(px(-2.))
                .mt(px(-11.))
                .px(px(5.))
                .rounded_tl(px(4.))
                .rounded_tr(px(4.))
                .rounded_br(px(4.))
                .bg(rgb(CLAUDE))
                .text_color(rgb(0xFFFFFF))
                .font_family(tk().sans)
                .text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("claude"),
        )
}

/// The pulsing dot of the file the agent edits now.
fn live_dot(id: (&'static str, usize)) -> AnyElement {
    div()
        .id(id)
        .flex_none()
        .size(px(7.))
        .rounded_full()
        .bg(rgb(CLAUDE))
        .tooltip(ui::tip("The agent edits this file now"))
        .with_animation(
            id,
            Animation::new(Duration::from_millis(1400)).repeat(),
            |el, t| el.opacity(1. - 0.65 * (t * std::f32::consts::TAU / 2.).sin().abs()),
        )
        .into_any_element()
}

/// A row of the file lists (`.tf`).
fn tree_row(id: (&'static str, usize), on: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .h(px(26.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(6.))
        .cursor_pointer()
        .whitespace_nowrap()
        .bg(rgb(if on { tk().track_2 } else { tree_bg() }))
        .hover(|s| s.bg(rgb(if on { tk().track_2 } else { tk().track })))
}

/// A file name that clips.
fn name_cell(name: &str) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .overflow_hidden()
        .text_ellipsis()
        .font_family(tk().mono)
        .child(name.to_string())
}
