//! The right panel of the Lens (320 px): the tabs Outline, Problems, Timeline, and Radar.

use aivory_protocol::code::{CodePosParams, Finding, Place};
use aivory_protocol::coord::{ClaimState, PingKind, RadarHit, RadarPingParams};
use aivory_protocol::rpc::method;
use gpui::prelude::*;
use gpui::{
    AnyElement, BoxShadow, Context, Div, FontWeight, SharedString, Stateful, div, point, px, rgb,
    rgba,
};

use super::logic::{
    self, OutlineRow, PointKind, Sev, bare_name, claims_on, focus_symbol, hits_for_file,
    kind_glyph, lease_text, lines_text, ping_label, recent_pings, stage_rows, state_word,
};
use super::{
    CLAIM_PALE, ClaimTone, FileState, LensEvent, LensScreen, Loadable, Note, PanelTab, ROW_HOVER,
    SELECTED, SideRefs, TABS, claim_pill_el, error_text, gb_button, section,
};
use crate::radar::{self, score_pill};
use crate::theme::*;
use crate::ui;

const TAG_TEXT: u32 = 0x2C4FA8;
const RING_LINE: u32 = 0x0D0D0E1F;

impl LensScreen {
    pub(super) fn panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let in_file = self.file_findings();
        let hits = self.file_hits();
        let mut track = div()
            .flex()
            .gap(px(2.))
            .p(px(2.))
            .w_full()
            .bg(rgb(super::TAB_TRACK))
            .rounded(tk().r_button);
        for (tab, label) in TABS {
            let on = self.tab == tab;
            let badge: Option<(String, u32)> = match tab {
                PanelTab::Problems if !in_file.is_empty() => {
                    Some((in_file.len().to_string(), tk().e_text))
                }
                PanelTab::Radar if !hits.is_empty() => Some((hits.len().to_string(), 0x6D28D9)),
                _ => None,
            };
            track = track.child(
                div()
                    .id(SharedString::from(format!("lens-tab-{label}")))
                    .flex_1()
                    .h(px(30.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .whitespace_nowrap()
                    .text_color(rgb(if on { tk().ink } else { tk().ink_2 }))
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(rgb(tk().card))
                            .font_weight(FontWeight::MEDIUM)
                            .shadow(vec![BoxShadow {
                                color: ui::hsla(0x0D0D0E14),
                                offset: point(px(0.), px(1.)),
                                blur_radius: px(2.),
                                spread_radius: px(0.),
                                inset: false,
                            }])
                    })
                    .when(!on, |d| d.hover(|s| s.text_color(rgb(tk().ink))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = tab;
                        cx.notify();
                    }))
                    .child(label)
                    .when_some(badge, |d, (text, color)| {
                        d.child(
                            div()
                                .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(color))
                                .child(text),
                        )
                    }),
            );
        }
        let body = match self.tab {
            PanelTab::Outline => self.outline_tab(cx),
            PanelTab::Problems => self.problems_tab(cx),
            PanelTab::Timeline => self.timeline_tab(cx),
            PanelTab::Radar => self.radar_tab(cx),
        };
        div()
            .w(px(320.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(tk().card))
            .border_l_1()
            .border_color(rgba(tk().line))
            .child(
                div()
                    .flex_none()
                    .pt(px(12.))
                    .px(px(16.))
                    .pb(px(6.))
                    .child(track),
            )
            .child(
                div()
                    .id("lens-panel-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .pb(px(24.))
                    .child(body),
            )
            .into_any_element()
    }

    /// The problems of the open file.
    fn file_findings(&self) -> Vec<Finding> {
        self.findings().to_vec()
    }

    fn file_hits(&self) -> Vec<RadarHit> {
        self.coord.as_ref().map_or(Vec::new(), |c| {
            hits_for_file(&c.hits, &c.claims, &self.path)
                .into_iter()
                .cloned()
                .collect()
        })
    }

    fn note_text(&self) -> Option<Div> {
        let note = self.note.as_ref()?;
        Some(
            div()
                .flex_none()
                .px(px(16.))
                .py(px(6.))
                .text_size(crate::ui::ts(crate::theme::T_SMALL))
                .text_color(rgb(if note.error { tk().e_text } else { 0x15803D }))
                .child(note.text.clone()),
        )
    }

    // ---- Outline ----

    fn outline_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let FileState::Ready(f) = &self.file else {
            return muted("The outline comes with the file.");
        };
        let rows = f.outline.clone();
        if rows.is_empty() {
            return muted("This file has no outline.");
        }
        let focus = focus_symbol(&rows, self.cursor).cloned();
        let chain: Vec<u32> = self
            .cursor
            .map(|l| logic::chain_at(&rows, l).iter().map(|r| r.line).collect())
            .unwrap_or_default();
        self.ensure_side(focus.as_ref(), cx);
        let name = file_name_of(&self.path);
        let mut col = div().flex().flex_col().child(section("Structure", name));
        for (i, r) in rows.iter().enumerate() {
            col = col.child(self.outline_row(i, r, chain.contains(&r.line), cx));
        }
        if let (Some(focus), Some(side)) = (&focus, &self.side) {
            let short = bare_name(&focus.name).to_string();
            let (callers, tests): (Vec<&Place>, Vec<&Place>) = match &side.items {
                Loadable::Ready(items) => items
                    .iter()
                    .filter(|p| !(p.path == self.path && p.line == focus.line))
                    .partition(|p| !is_test_place(p)),
                _ => (Vec::new(), Vec::new()),
            };
            let ready = matches!(side.items, Loadable::Ready(_));
            col = col.child(section(
                &format!("Callers of {short}"),
                if ready {
                    format!("call hierarchy · {}", callers.len())
                } else {
                    "loads…".to_string()
                },
            ));
            for (i, p) in callers.iter().enumerate() {
                col = col.child(self.place_row(("lens-caller", i), "→", p, false, cx));
            }
            if ready && callers.is_empty() {
                col = col.child(muted("No callers found."));
            }
            col = col.child(section(
                &format!("Tests of {short}"),
                tests.len().to_string(),
            ));
            for (i, p) in tests.iter().enumerate() {
                col = col.child(self.place_row(("lens-test", i), "▸", p, true, cx));
            }
            if ready && tests.is_empty() {
                col = col.child(muted("No test names it."));
            }
        }
        col.into_any_element()
    }

    fn outline_row(
        &self,
        i: usize,
        r: &OutlineRow,
        on: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let claim = self
            .claims()
            .iter()
            .filter(|c| c.path.as_deref() == Some(self.path.as_str()))
            .filter(|c| c.state != ClaimState::Landed)
            .filter(|c| {
                c.range.is_some_and(|g| g.from <= r.end && r.start <= g.to)
                    && Some(c.session.as_str()) != self.from_session.as_deref()
            })
            .max_by_key(|c| c.state == ClaimState::Editing);
        let worst = self
            .findings()
            .iter()
            .filter(|f| r.start <= f.line && f.line <= r.end)
            .filter_map(|f| Sev::of(f.level))
            .max();
        let line = r.line;
        div()
            .id(("lens-outline", i))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(30.))
            .pl(px(16. + f32::from(r.depth) * 18.))
            .pr(px(16.))
            .font_family(tk().mono)
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .cursor_pointer()
            .when(on, |d| d.bg(rgb(SELECTED)))
            .when(!on, |d| d.hover(|s| s.bg(rgb(ROW_HOVER))))
            .on_click(cx.listener(move |this, _, _, cx| this.jump_to(line, cx)))
            .child(
                div()
                    .w(px(22.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().muted))
                    .child(kind_glyph(r.kind)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(r.name.clone()),
            )
            .when_some(claim, |d, c| {
                d.child(claim_pill_el(
                    &c.session,
                    if c.state == ClaimState::Editing {
                        ClaimTone::Plain
                    } else {
                        ClaimTone::Pale
                    },
                ))
            })
            .when_some(worst, |d, sev| d.child(severity_dot(sev)))
            .child(
                div()
                    .flex_none()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().muted))
                    .whitespace_nowrap()
                    .child(range_text(r.start, r.end)),
            )
    }

    /// A caller or test row: glyph, the enclosing declaration or file, `file:line`.
    fn place_row(
        &self,
        id: (&'static str, usize),
        glyph: &'static str,
        p: &Place,
        test: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label = match &p.container {
            Some(c) if !c.is_empty() => c.clone(),
            _ => logic::file_name(&p.path).to_string(),
        };
        let problem = self
            .problems
            .as_ref()
            .is_some_and(|d| d.rows.iter().any(|f| f.path == p.path && f.line == p.line));
        let (path, line) = (p.path.clone(), p.line);
        div()
            .id(id)
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(30.))
            .px(px(16.))
            .font_family(tk().mono)
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(ROW_HOVER)))
            .on_click(cx.listener(move |this, _, _, cx| this.open_path(&path, Some(line), cx)))
            .child(
                div()
                    .w(px(22.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().muted))
                    .child(glyph),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(label),
            )
            .when(problem, |d| d.child(severity_dot(Sev::Error)))
            // The state of a test (passes, fails) is not in the protocol yet: a hollow ring.
            .when(test, |d| d.child(severity_dot(Sev::Hint)))
            .child(
                div()
                    .flex_none()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().muted))
                    .whitespace_nowrap()
                    .child(logic::place_text(&p.path, p.line)),
            )
    }

    /// Load the references of the symbol at the cursor, once per symbol.
    fn ensure_side(&mut self, focus: Option<&OutlineRow>, cx: &mut Context<Self>) {
        let Some(focus) = focus else {
            return;
        };
        let key = (self.path.clone(), focus.line);
        if self.side.as_ref().is_some_and(|s| s.key == key) {
            return;
        }
        let FileState::Ready(f) = &self.file else {
            return;
        };
        let name = bare_name(&focus.name).to_string();
        let col = f
            .lines
            .get(focus.line.saturating_sub(1) as usize)
            .and_then(|l| l.find(&name))
            .unwrap_or(0) as u32;
        self.side = Some(SideRefs {
            key: key.clone(),
            items: Loadable::Loading,
        });
        let call = self.model.read(cx).call::<method::CodeReferences>(
            CodePosParams {
                codebase: self.codebase.clone(),
                path: self.path.clone(),
                line: focus.line,
                col,
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
                if let Some(side) = this.side.as_mut().filter(|s| s.key == key) {
                    side.items = match result {
                        Ok(page) => Loadable::Ready(page.items),
                        Err(e) => Loadable::Failed(error_text(&e)),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ---- Problems ----

    fn problems_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let findings = self.file_findings();
        let added: Vec<&str> = self
            .problems
            .as_ref()
            .map(|p| {
                p.added
                    .iter()
                    .filter(|f| !f.fingerprint.is_empty())
                    .map(|f| f.fingerprint.as_str())
                    .collect()
            })
            .unwrap_or_default();
        let turn = self.problems.as_ref().and_then(|p| p.turn);
        let new_count = findings
            .iter()
            .filter(|f| added.contains(&f.fingerprint.as_str()))
            .count();
        let mut meta = findings.len().to_string();
        if let (true, Some(t)) = (new_count > 0, turn) {
            meta.push_str(&format!(" · {new_count} added in turn {t}"));
        }
        let mut col = div().flex().flex_col().child(section("This file", meta));
        if findings.is_empty() {
            col = col.child(muted("No problems in this file."));
        }
        for (i, f) in findings.iter().enumerate() {
            let is_new = added.contains(&f.fingerprint.as_str());
            col = col.child(self.problem_row(
                ("lens-prob", i),
                f,
                false,
                is_new.then_some(turn).flatten(),
                cx,
            ));
        }
        let others: Vec<Finding> = self
            .problems
            .as_ref()
            .map(|p| {
                p.rows
                    .iter()
                    .filter(|r| r.path != self.path)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if !others.is_empty() {
            let meta = match turn {
                Some(t) => format!("{} · caused by turn {t}", others.len()),
                None => others.len().to_string(),
            };
            col = col.child(section("Other files", meta));
            for (i, f) in others.iter().enumerate() {
                col = col.child(self.problem_row(("lens-prob2", i), f, true, None, cx));
            }
        }
        let notes = self
            .problems
            .as_ref()
            .map(|p| p.notes.clone())
            .unwrap_or_default();
        let (stages, rest) = stage_rows(&notes);
        let run_meta = match (turn, &self.from_session) {
            (Some(t), Some(s)) => format!("after turn {s} · {t}"),
            (Some(t), None) => format!("after turn {t}"),
            _ => String::new(),
        };
        col = col.child(section("Diagnostics run", run_meta));
        if stages.is_empty() && rest.is_empty() {
            col = col.child(muted("No run yet."));
        }
        for s in stages {
            col = col.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .px(px(16.))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(if s.on { tk().ink_2 } else { tk().muted }))
                    .child(if s.on {
                        ui::dot(rgb(tk().ok)).into_any_element()
                    } else {
                        div()
                            .flex_none()
                            .size(px(8.))
                            .rounded_full()
                            .border(px(1.5))
                            .border_color(rgb(tk().idle_ring))
                            .into_any_element()
                    })
                    .child(div().flex_1().child(s.name))
                    .when(!s.on, |d| {
                        d.child(div().text_color(rgb(tk().muted)).child("off"))
                    }),
            );
        }
        for note in rest {
            col = col.child(
                div()
                    .flex_none()
                    .px(px(16.))
                    .py(px(4.))
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().muted))
                    .child(note),
            );
        }
        col.into_any_element()
    }

    fn problem_row(
        &self,
        id: (&'static str, usize),
        f: &Finding,
        other_file: bool,
        new_in: Option<u32>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let sev = Sev::of(f.level).unwrap_or(Sev::Hint);
        let source = match (f.tool.is_empty(), f.rule.is_empty()) {
            (false, false) => format!("{} · {}", f.tool, f.rule),
            (true, false) => f.rule.clone(),
            (false, true) => f.tool.clone(),
            (true, true) => String::new(),
        };
        let at = if other_file {
            format!("{} · L{}", logic::file_name(&f.path), f.line)
        } else {
            format!("L{}", f.line)
        };
        let claim = other_file
            .then(|| radar::claim_pill(self.claims(), &f.path))
            .flatten()
            .map(|(s, _)| format!("claimed by {s}"));
        let (path, line) = (f.path.clone(), f.line);
        let mut meta = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(8.))
            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
            .text_color(rgb(tk().ink_3))
            .child(source)
            .child(at);
        if let Some(t) = new_in {
            meta = meta.child(
                div()
                    .px(px(8.))
                    .py(px(1.))
                    .rounded_full()
                    .bg(rgb(SELECTED))
                    .text_color(rgb(TAG_TEXT))
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .child(format!("new in turn {t}")),
            );
        }
        if let Some(c) = claim {
            meta = meta.child(claim_pill_el(&c, ClaimTone::Plain));
        }
        div()
            .id(id)
            .flex_none()
            .flex()
            .gap(px(8.))
            .py(px(10.))
            .px(px(16.))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(ROW_HOVER)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if path == this.path {
                    this.jump_to(line, cx);
                } else {
                    this.open_path(&path, Some(line), cx);
                }
            }))
            .child(
                div()
                    .w(px(14.))
                    .flex_none()
                    .pt(px(5.))
                    .child(severity_dot(sev)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .line_height(gpui::relative(1.4))
                            .child(f.message.clone()),
                    )
                    .child(meta),
            )
    }

    // ---- Timeline ----

    fn timeline_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let now = self.now(cx);
        let mut col = div().flex().flex_col().child(section(
            super::file_name(&self.path),
            "click shows the diff",
        ));
        if let Some(n) = self.note_text() {
            col = col.child(n);
        }
        if self.timeline.is_empty() {
            return col
                .child(muted("The ledger has no turn or commit for this file."))
                .into_any_element();
        }
        let selected = self.diff.as_ref().map(|d| d.point.kind.clone());
        let points = self.timeline.clone();
        for (i, p) in points.iter().enumerate() {
            let on = selected.as_ref() == Some(&p.kind);
            let when = when_text(now, p.time, self.clock(p.time, cx));
            let point = p.clone();
            let dot = match (&p.kind, p.color) {
                (PointKind::Turn { .. }, Some(c)) => div().bg(rgb(c)),
                _ => div()
                    .bg(rgb(tk().card))
                    .border(px(1.5))
                    .border_color(rgb(tk().idle_ring)),
            };
            col = col.child(
                div()
                    .id(("lens-point", i))
                    .relative()
                    .flex_none()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .py(px(11.))
                    .pl(px(42.))
                    .pr(px(16.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(rgb(SELECTED)))
                    .when(!on, |d| d.hover(|s| s.bg(rgb(ROW_HOVER))))
                    .on_click(cx.listener(move |this, _, _, cx| this.show_point(point.clone(), cx)))
                    .child(
                        div()
                            .absolute()
                            .left(px(21.))
                            .top_0()
                            .bottom_0()
                            .w(px(1.))
                            .bg(rgba(0x0D0D0E1F)),
                    )
                    .child(
                        dot.absolute()
                            .left(px(16.))
                            .top(px(15.))
                            .size(px(11.))
                            .rounded_full(),
                    )
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(tk().ink))
                                    .child(p.who.clone()),
                            )
                            .child(when)
                            .child(div().flex_1())
                            .when(p.added.is_some(), |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .gap(px(4.))
                                        .whitespace_nowrap()
                                        .child(
                                            div()
                                                .text_color(rgb(0x15803D))
                                                .child(format!("+{}", p.added.unwrap_or(0))),
                                        )
                                        .child(
                                            div()
                                                .text_color(rgb(tk().e_text))
                                                .child(format!("−{}", p.removed.unwrap_or(0))),
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .line_height(gpui::relative(1.4))
                            .child(p.text.clone()),
                    )
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(p.detail.clone()),
                    ),
            );
        }
        col.into_any_element()
    }

    // ---- Radar ----

    fn radar_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(coord) = self.coord.clone() else {
            return muted("Loads the claims from aivoryd…");
        };
        let now = self.now(cx);
        let own = claims_on(&coord.claims, &self.path);
        let mut col = div().flex().flex_col();
        col = col.child(section(
            "Claims on this file",
            format!(
                "{} session{}",
                own.len(),
                if own.len() == 1 { "" } else { "s" }
            ),
        ));
        if own.is_empty() {
            col = col.child(muted("No live claim on this file."));
        }
        for c in own {
            let provider = coord
                .node(&c.session)
                .map(|n| n.provider.clone())
                .or_else(|| {
                    self.model
                        .read(cx)
                        .war
                        .as_ref()
                        .and_then(|w| w.session(&c.session).map(|s| s.provider.clone()))
                })
                .unwrap_or_default();
            let at = match c.range {
                Some(r) => format!("{} · {}", lines_text(r.from, r.to), lease_text(c.lease_s)),
                None => lease_text(c.lease_s),
            };
            col = col.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(6.))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .child(
                        div()
                            .flex_none()
                            .size(px(7.))
                            .rounded_full()
                            .bg(rgb(provider_color(&provider))),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(tk().ink))
                            .child(c.session.clone()),
                    )
                    .child(state_word(c.state))
                    .child(div().flex_1())
                    .child(div().text_color(rgb(tk().muted)).child(at)),
            );
        }
        let hits = self.file_hits();
        let meta = match &self.from_session {
            Some(s) if hits.iter().any(|h| &h.a == s || &h.b == s) => {
                format!("{} · session {s} is one side", hits.len())
            }
            _ => hits.len().to_string(),
        };
        col = col.child(section("Radar hits", meta));
        if hits.is_empty() {
            col = col.child(muted("No pair of sessions meets on this file."));
        }
        for (i, h) in hits.iter().enumerate() {
            col = col.child(self.hit_card(i, h, cx));
        }
        col = col.child(section("Pings", "last hour"));
        let pings = recent_pings(&self.mail, now);
        if pings.is_empty() {
            col = col.child(muted("No ping in the last hour."));
        }
        for p in pings {
            let who = format!("{} to {}", ping_label(p.kind), p.to);
            col = col.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(16.))
                    .py(px(6.))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(tk().ink))
                                    .child(who),
                            )
                            .child(self.clock(p.at, cx)),
                    )
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .line_height(gpui::relative(1.4))
                            .text_color(rgb(tk().ink_2))
                            .child(p.text),
                    ),
            );
        }
        if let Some(n) = self.note_text() {
            col = col.child(n);
        }
        col.into_any_element()
    }

    fn hit_card(&self, i: usize, h: &RadarHit, cx: &mut Context<Self>) -> Div {
        let (a, b) = (h.a.clone(), h.b.clone());
        let hit = h.clone();
        let key = h.keys.first().cloned().unwrap_or_default();
        div()
            .flex_none()
            .mx(px(16.))
            .mb(px(10.))
            .px(px(14.))
            .py(px(12.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .rounded(tk().r_card)
            .border_1()
            .border_color(rgba(tk().line))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .child(score_pill(h.score, false))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} ↔ {}", h.a, h.b)),
                    ),
            )
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .text_color(rgb(tk().ink_2))
                    .child(radar::tier_text(h)),
            )
            .child(
                div()
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(key),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .mt(px(2.))
                    .child(gb_button(("lens-hit-open", i), "Open both", true).on_click(
                        cx.listener(move |_, _, _, cx| {
                            cx.emit(LensEvent::OpenSessions(vec![a.clone(), b.clone()]))
                        }),
                    ))
                    .child(
                        gb_button(("lens-hit-ping", i), "Send ping", true)
                            .on_click(cx.listener(move |this, _, _, cx| this.ping_pair(&hit, cx))),
                    ),
            )
    }

    /// "Send ping": an FYI to both sessions of the pair.
    fn ping_pair(&mut self, hit: &RadarHit, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let calls: Vec<_> = [(&hit.a, &hit.b), (&hit.b, &hit.a)]
            .into_iter()
            .map(|(to, other)| {
                model.call::<method::RadarPing>(
                    RadarPingParams {
                        from: None,
                        to: to.clone(),
                        kind: PingKind::Fyi,
                        text: radar::ping_text(hit, other),
                        keys: hit.keys.clone(),
                    },
                    cx,
                )
            })
            .collect();
        let (a, b) = (hit.a.clone(), hit.b.clone());
        cx.spawn(async move |this, cx| {
            let mut failed = None;
            for call in calls {
                if let Err(e) = call.await {
                    failed = Some(error_text(&e));
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.note = Some(match failed {
                    None => Note {
                        text: format!("Pinged {a} and {b}.").into(),
                        error: false,
                        open: None,
                    },
                    Some(e) => Note {
                        text: format!("Cannot send the ping: {e}").into(),
                        error: true,
                        open: None,
                    },
                });
                this.refresh_side(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

/// A quiet line in a tab.
fn muted(text: &'static str) -> AnyElement {
    div()
        .flex_none()
        .px(px(16.))
        .py(px(8.))
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .text_color(rgb(tk().muted))
        .child(text)
        .into_any_element()
}

/// The 8 px dot of a severity: filled, or a ring for a hint.
fn severity_dot(sev: Sev) -> Div {
    let d = div().flex_none().size(px(8.)).rounded_full();
    if sev == Sev::Hint {
        d.border(px(1.5)).border_color(rgb(tk().idle_ring))
    } else {
        d.bg(rgb(sev.color()))
    }
}

fn file_name_of(path: &str) -> String {
    logic::file_name(path).to_string()
}

/// `17–24`, or `8` for one line.
fn range_text(from: u32, to: u32) -> String {
    if from == to {
        from.to_string()
    } else {
        format!("{from}–{to}")
    }
}

/// A test is named in a file or a module that has `test` in its name.
fn is_test_place(p: &Place) -> bool {
    let path = p.path.to_lowercase();
    let in_tests = path.contains("/tests/")
        || path.contains("/test/")
        || path.contains("_test.")
        || path.contains(".test.")
        || path.contains("/tests.")
        || path
            .rsplit('/')
            .next()
            .is_some_and(|f| f.starts_with("test_"));
    in_tests
        || p.container
            .as_deref()
            .is_some_and(|c| c.to_lowercase().contains("test"))
}

/// `14:36` today, `yesterday`, `3 days ago`.
pub(super) fn when_text(now: i64, at: i64, clock: String) -> String {
    let age = now - at;
    if age < 24 * 3_600_000 {
        clock
    } else if age < 48 * 3_600_000 {
        "yesterday".to_string()
    } else {
        format!("{} days ago", age / (24 * 3_600_000))
    }
}

#[allow(dead_code, reason = "the ring color of an open point")]
const _RING: u32 = RING_LINE;
#[allow(dead_code, reason = "the pale tone of an intent claim")]
const _PALE: u32 = CLAIM_PALE;

#[cfg(test)]
mod tests {
    use super::*;

    fn place(path: &str, container: Option<&str>) -> Place {
        Place {
            path: path.into(),
            line: 1,
            col: 0,
            len: None,
            preview: String::new(),
            name: None,
            kind: None,
            container: container.map(str::to_string),
        }
    }

    #[test]
    fn test_places_are_found_by_path_or_module() {
        assert!(is_test_place(&place("crates/orch/tests/budget.rs", None)));
        assert!(is_test_place(&place("src/budget_test.go", None)));
        assert!(is_test_place(&place("src/a.rs", Some("tests"))));
        assert!(!is_test_place(&place("src/a.rs", Some("Invoice"))));
        assert!(!is_test_place(&place("src/contest.rs", None)));
    }

    #[test]
    fn old_points_read_as_days() {
        let hour = 3_600_000;
        assert_eq!(when_text(100 * hour, 99 * hour, "14:36".into()), "14:36");
        assert_eq!(
            when_text(100 * hour, 70 * hour + 1, "x".into()),
            "yesterday"
        );
        assert_eq!(when_text(100 * hour, 10 * hour, "x".into()), "3 days ago");
        assert_eq!(range_text(17, 24), "17–24");
        assert_eq!(range_text(8, 8), "8");
    }
}
