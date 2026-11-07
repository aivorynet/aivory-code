//! The Radar tab of Repos: the graph card and the hit list, and the data fetch that the Lens
//! shares.

use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::coord::{
    Claim, ClaimList, ClaimsListParams, PingKind, RadarHit, RadarListParams, RadarNode,
    RadarPingParams,
};
use aivory_protocol::rpc::{SessionRef, method};
use gpui::prelude::*;
use gpui::{
    Bounds, BoxShadow, ClickEvent, Context, Div, ElementId, Entity, EventEmitter, FontWeight,
    PathBuilder, Pixels, SharedString, Stateful, Task, Window, canvas, deferred, div, point, px,
    relative, rgb, rgba,
};

use super::{
    BOX_H, BOX_W, EdgeGeom, NODE_H, NODE_W, REFRESH_EVERY, at_risk, edge_geom, edge_width,
    graph_meta, hits_meta, is_low, key_lines, layout_nodes, legend, node_order, ping_text, risk,
    score_text, sort_hits, tier_text,
};
use crate::model::Model;
use crate::theme::*;
use crate::ui;

/// What the radar tells the shell.
#[derive(Debug, Clone)]
pub enum RadarEvent {
    /// "Open both": the deep dives of these sessions.
    OpenSessions(Vec<String>),
}

/// What the screens read from the coordination methods.
#[derive(Debug, Clone, Default)]
pub struct Coord {
    pub claims: Vec<Claim>,
    pub sessions: Vec<RadarNode>,
    /// Sorted by [`sort_hits`].
    pub hits: Vec<RadarHit>,
}

impl Coord {
    pub fn node(&self, id: &str) -> Option<&RadarNode> {
        self.sessions.iter().find(|n| n.id == id)
    }
}

/// Ask the daemon for the claims and the radar of a codebase. A daemon without the methods
/// gives an empty answer.
pub fn fetch(model: &Entity<Model>, codebase: &str, cx: &gpui::App) -> Task<Coord> {
    let m = model.read(cx);
    let claims = m.call::<method::ClaimsList>(
        ClaimsListParams {
            codebase: Some(codebase.to_string()),
            session: None,
        },
        cx,
    );
    let radar = m.call::<method::RadarListMethod>(
        RadarListParams {
            codebase: Some(codebase.to_string()),
        },
        cx,
    );
    cx.spawn(async move |_| {
        let ClaimList { claims } = claims.await.unwrap_or_else(|e| {
            eprintln!("aivory: claims/list failed: {e:#}");
            ClaimList { claims: Vec::new() }
        });
        let (sessions, mut hits) = match radar.await {
            Ok(r) => (r.sessions, r.hits),
            Err(e) => {
                eprintln!("aivory: radar/list failed: {e:#}");
                (Vec::new(), Vec::new())
            }
        };
        sort_hits(&mut hits);
        Coord {
            claims,
            sessions,
            hits,
        }
    })
}

pub struct RadarView {
    model: Entity<Model>,
    codebase: String,
    coord: Option<Rc<Coord>>,
    /// A clicked node: its edges stay, the rest dims.
    selected: Option<String>,
    /// The edge chip under the pointer (shows the routing key).
    hover: Option<usize>,
    /// The row whose "Pause one" menu is open.
    menu: Option<usize>,
    /// A menu closed by a click outside, and when: the click on its own button must not open
    /// it again.
    closed: Option<(usize, Instant)>,
    note: Option<SharedString>,
    loading: bool,
    last: Option<Instant>,
    _note_timer: Option<Task<()>>,
}

impl EventEmitter<RadarEvent> for RadarView {}

impl RadarView {
    pub fn new(model: Entity<Model>) -> Self {
        Self {
            model,
            codebase: String::new(),
            coord: None,
            selected: None,
            hover: None,
            menu: None,
            closed: None,
            note: None,
            loading: false,
            last: None,
            _note_timer: None,
        }
    }

    pub fn codebase(&self) -> &str {
        &self.codebase
    }

    pub fn coord(&self) -> Option<Rc<Coord>> {
        self.coord.clone()
    }

    /// The pairs at 0.7 and up: the count on the tab.
    pub fn at_risk(&self) -> usize {
        self.coord.as_ref().map_or(0, |c| at_risk(&c.hits))
    }

    pub fn set_codebase(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.codebase == id {
            return;
        }
        self.codebase = id.to_string();
        self.coord = None;
        self.selected = None;
        self.menu = None;
        self.note = None;
        self.last = None;
        self.refresh(cx);
        cx.notify();
    }

    /// Load the claims and the hits; at most one call each 3 s.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.codebase.is_empty() {
            return;
        }
        if self.last.is_some_and(|t| t.elapsed() < REFRESH_EVERY) {
            return;
        }
        self.loading = true;
        self.last = Some(Instant::now());
        let codebase = self.codebase.clone();
        let task = fetch(&self.model, &codebase, cx);
        cx.spawn(async move |this, cx| {
            let coord = task.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                if this.codebase == codebase {
                    this.coord = Some(Rc::new(coord));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn say(&mut self, text: String, cx: &mut Context<Self>) {
        self.note = Some(text.into());
        self._note_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(6)).await;
            let _ = this.update(cx, |this, cx| {
                this.note = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn hit(&self, ix: usize) -> Option<RadarHit> {
        self.coord.as_ref()?.hits.get(ix).cloned()
    }

    fn open_both(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(hit) = self.hit(ix) {
            cx.emit(RadarEvent::OpenSessions(vec![hit.a, hit.b]));
        }
    }

    /// Send an FYI to both sessions about the keys of the hit.
    fn send_ping(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(hit) = self.hit(ix) else { return };
        let model = self.model.read(cx);
        let calls: Vec<_> = [(&hit.a, &hit.b), (&hit.b, &hit.a)]
            .into_iter()
            .map(|(to, other)| {
                model.call::<method::RadarPing>(
                    RadarPingParams {
                        from: None,
                        to: to.clone(),
                        kind: PingKind::Fyi,
                        text: ping_text(&hit, other),
                        keys: hit.keys.clone(),
                    },
                    cx,
                )
            })
            .collect();
        cx.spawn(async move |this, cx| {
            let mut failed = None;
            for call in calls {
                if let Err(e) = call.await {
                    failed = Some(format!("{e:#}"));
                }
            }
            let text = match failed {
                None => format!("Pinged {} and {}.", hit.a, hit.b),
                Some(e) => format!("Cannot send the ping: {e}"),
            };
            let _ = this.update(cx, |this, cx| this.say(text, cx));
        })
        .detach();
    }

    fn pause(&mut self, session: String, cx: &mut Context<Self>) {
        self.menu = None;
        let call = self.model.read(cx).call::<method::SessionPause>(
            SessionRef {
                id: session.clone(),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let text = match call.await {
                Ok(_) => format!("Paused {session}."),
                Err(e) => format!("Cannot pause {session}: {e:#}"),
            };
            let _ = this.update(cx, |this, cx| this.say(text, cx));
        })
        .detach();
        cx.notify();
    }

    fn toggle_menu(&mut self, ix: usize, cx: &mut Context<Self>) {
        let just_closed = self
            .closed
            .is_some_and(|(i, t)| i == ix && t.elapsed() < Duration::from_millis(250));
        self.menu = if self.menu == Some(ix) || just_closed {
            None
        } else {
            Some(ix)
        };
        cx.notify();
    }

    fn select_node(&mut self, id: &str, cx: &mut Context<Self>) {
        self.selected = if self.selected.as_deref() == Some(id) {
            None
        } else {
            Some(id.to_string())
        };
        cx.notify();
    }

    // ---- Graph ----

    fn graph_card(&self, coord: &Coord, cx: &mut Context<Self>) -> Div {
        let order = node_order(&coord.hits);
        let nodes = layout_nodes(&order);
        let pos = |id: &str| nodes.iter().find(|n| n.id == id);
        let selected = self.selected.as_deref();
        let dimmed = |a: &str, b: &str| selected.is_some_and(|s| s != a && s != b);
        // The edges whose two ends are on the box.
        let edges: Vec<(usize, EdgeGeom, &RadarHit)> = coord
            .hits
            .iter()
            .enumerate()
            .filter_map(|(i, hit)| {
                let (a, b) = (pos(&hit.a)?, pos(&hit.b)?);
                Some((i, edge_geom(a, b), hit))
            })
            .collect();
        let paint: Vec<(EdgeGeom, u32, f32, u32)> = edges
            .iter()
            .map(|(_, geom, hit)| {
                let alpha = if dimmed(&hit.a, &hit.b) { 0x30 } else { 0xFF };
                (*geom, risk(hit.score).edge, edge_width(hit.score), alpha)
            })
            .collect();
        let mut gbox = div().relative().mt(px(6.)).h(px(BOX_H)).w_full().child(
            canvas(
                move |_, _, _| paint,
                move |bounds: Bounds<Pixels>, paint, window, _| {
                    let sx = bounds.size.width / px(BOX_W);
                    let at = |p: (f32, f32)| {
                        point(bounds.origin.x + px(p.0) * sx, bounds.origin.y + px(p.1))
                    };
                    for (geom, color, width, alpha) in &paint {
                        let mut path = PathBuilder::stroke(px(*width));
                        path.move_to(at(geom.from));
                        if geom.c1 == geom.from && geom.c2 == geom.to {
                            path.line_to(at(geom.to));
                        } else {
                            path.cubic_bezier_to(at(geom.to), at(geom.c1), at(geom.c2));
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, ui::hsla(color << 8 | alpha));
                        }
                    }
                },
            )
            .absolute()
            .inset_0(),
        );
        for node in &nodes {
            let info = coord.node(&node.id);
            let provider = info.map_or("", |n| n.provider.as_str());
            let sub = info.map_or(String::new(), |n| {
                format!("{} · {}", n.part, n.owner.as_deref().unwrap_or("you"))
            });
            let faded = selected.is_some_and(|s| {
                s != node.id
                    && !coord
                        .hits
                        .iter()
                        .any(|h| (h.a == s && h.b == node.id) || (h.b == s && h.a == node.id))
            });
            let id = node.id.clone();
            gbox = gbox.child(
                div()
                    .id(SharedString::from(format!("rnode-{}", node.id)))
                    .absolute()
                    .left(relative(node.x / BOX_W))
                    .ml(px(-NODE_W / 2.))
                    .top(px(node.y - NODE_H / 2.))
                    .w(px(NODE_W))
                    .h(px(NODE_H))
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .rounded(tk().r_card)
                    .bg(rgb(tk().card))
                    .shadow(node_shadow())
                    .cursor_pointer()
                    .when(faded, |d| d.opacity(0.35))
                    .on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.select_node(&id, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .text_size(crate::ui::ts(crate::theme::T_SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(rgb(provider_color(provider))),
                            )
                            .child(node.id.clone())
                            .child(div().flex_1())
                            .when_some(info, |d, n| d.child(ui::state_dot(n.state, 8.))),
                    )
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(sub),
                    ),
            );
        }
        for (i, geom, hit) in &edges {
            let i = *i;
            let faded = dimmed(&hit.a, &hit.b);
            let hovered = self.hover == Some(i);
            gbox = gbox.child(
                div()
                    .id(("rchip", i))
                    .absolute()
                    .left(relative(geom.mid.0 / BOX_W))
                    .ml(px(-19.))
                    .top(px(geom.mid.1 - 10.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .when(faded, |d| d.opacity(0.35))
                    .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                        let next = over.then_some(i);
                        if this.hover != next && (*over || this.hover == Some(i)) {
                            this.hover = next;
                            cx.notify();
                        }
                    }))
                    .child(score_pill(hit.score, true).w(px(38.)))
                    .when(hovered, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(24.))
                                .px(px(8.))
                                .py(px(3.))
                                .rounded(px(4.))
                                .bg(rgb(tk().dark))
                                .text_color(rgb(tk().card))
                                .font_family(tk().mono)
                                .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                                .whitespace_nowrap()
                                .child(hit.keys.first().cloned().unwrap_or_default()),
                        )
                    }),
            );
        }
        let mut legend_row = div().flex().items_center().gap(px(10.));
        for r in legend() {
            legend_row = legend_row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(
                        div()
                            .flex_none()
                            .w(px(16.))
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(rgb(r.edge)),
                    )
                    .child(r.legend),
            );
        }
        div()
            .flex_none()
            .w_full()
            .pt(px(14.))
            .px(px(18.))
            .pb(px(12.))
            .bg(rgb(tk().card))
            .rounded(tk().r_card)
            .shadow(ui::card_shadow())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(24.))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .child("sessions → shared keys"),
                    )
                    .child(
                        div()
                            .text_color(rgb(tk().ink_3))
                            .whitespace_nowrap()
                            .child(graph_meta(
                                order.len(),
                                coord.hits.len(),
                                at_risk(&coord.hits),
                            )),
                    )
                    .child(div().flex_1())
                    .child(legend_row),
            )
            .child(gbox)
    }

    // ---- Hits ----

    fn hits_card(&self, coord: &Coord, cx: &mut Context<Self>) -> Div {
        let mut card = div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .bg(rgb(tk().card))
            .rounded(tk().r_card)
            .shadow(ui::card_shadow())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(42.))
                    .px(px(18.))
                    .border_b_1()
                    .border_color(rgba(tk().line))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Hits"))
                    .child(
                        div()
                            .text_color(rgb(tk().ink_3))
                            .whitespace_nowrap()
                            .child(hits_meta(coord.hits.len())),
                    )
                    .child(div().flex_1())
                    .when_some(self.note.clone(), |d, n| {
                        d.child(
                            div()
                                .text_size(crate::ui::ts(crate::theme::T_SMALL))
                                .text_color(rgb(tk().ink_2))
                                .whitespace_nowrap()
                                .child(n),
                        )
                    }),
            );
        let count = coord.hits.len();
        for (i, hit) in coord.hits.iter().enumerate() {
            card = card.child(self.hit_row(coord, i, hit, i + 1 < count, cx));
        }
        card
    }

    fn hit_row(
        &self,
        coord: &Coord,
        ix: usize,
        hit: &RadarHit,
        line: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let color = |id: &str| provider_color(coord.node(id).map_or("", |n| n.provider.as_str()));
        let (k1, k2) = key_lines(hit);
        let key_line = |text: String| {
            div()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(text)
        };
        let dot = |c: u32| div().flex_none().size(px(7.)).rounded_full().bg(rgb(c));
        let mut pause = div()
            .relative()
            .child(small_button(("rpause", ix), "Pause one ▾").on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_menu(ix, cx)),
            ));
        if self.menu == Some(ix) {
            let mut menu = ui::menu(150.);
            for (n, id) in [hit.a.clone(), hit.b.clone()].into_iter().enumerate() {
                let target = id.clone();
                menu = menu.child(ui::menu_item(
                    ("rpause-item", ix * 2 + n),
                    false,
                    format!("Pause {id}").into_any_element(),
                    None,
                    Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.pause(target.clone(), cx)
                    })),
                ));
            }
            pause = pause.child(
                deferred(
                    div().absolute().top(px(30.)).right_0().child(
                        menu.occlude()
                            .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                this.closed = Some((ix, Instant::now()));
                                cx.notify();
                            })),
                    ),
                )
                .with_priority(1),
            );
        }
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(14.))
            .h(px(62.))
            .px(px(18.))
            .when(line, |d| d.border_b_1().border_color(rgba(0x0D0D0E0F)))
            .when(is_low(hit.score), |d| d.opacity(0.6))
            .child(score_pill(hit.score, false).flex_none())
            .child(
                div()
                    .w(px(290.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .text_size(crate::ui::ts(crate::theme::T_SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(dot(color(&hit.a)))
                            .child(hit.a.clone())
                            .child(
                                div()
                                    .text_color(rgb(tk().muted))
                                    .font_weight(FontWeight::NORMAL)
                                    .child("↔"),
                            )
                            .child(dot(color(&hit.b)))
                            .child(hit.b.clone()),
                    )
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_SMALL))
                            .text_color(rgb(tk().ink_2))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(tier_text(hit)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(key_line(k1))
                    .child(key_line(k2)),
            )
            .child(
                div()
                    .w(px(262.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(small_button(("ropen", ix), "Open both").on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.open_both(ix, cx)),
                    ))
                    .child(small_button(("rping", ix), "Send ping").on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.send_ping(ix, cx)),
                    ))
                    .child(pause),
            )
    }
}

impl Render for RadarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(coord) = self.coord.clone() else {
            return empty_card("Loads the radar from aivoryd…").into_any_element();
        };
        if coord.hits.is_empty() {
            return empty_card("No pair of sessions changes the same code.").into_any_element();
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(self.graph_card(&coord, cx))
            .child(self.hits_card(&coord, cx))
            .into_any_element()
    }
}

fn empty_card(text: &'static str) -> Div {
    div()
        .flex_none()
        .w_full()
        .px(px(18.))
        .py(px(16.))
        .bg(rgb(tk().card))
        .rounded(tk().r_card)
        .shadow(ui::card_shadow())
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .text_color(rgb(tk().muted))
        .child(text)
}

/// The score chip (`.sc`); `outlined` is the chip on an edge (white, 1.5 px risk border).
pub fn score_pill(score: f32, outlined: bool) -> Div {
    let r = risk(score);
    let chip = div()
        .min_w(px(34.))
        .h(px(20.))
        .px(px(8.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(r.text))
        .child(score_text(score));
    if outlined {
        chip.bg(rgb(tk().card))
            .border(px(1.5))
            .border_color(rgb(r.text))
    } else {
        chip.bg(rgb(r.bg))
    }
}

/// The small button of a row (`.gb.sm`).
pub fn small_button(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(26.))
        .px(px(10.))
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(0xC9CCD6))
        .bg(rgb(tk().card))
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(0x8A93A6)))
        .child(label)
}

fn node_shadow() -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: ui::hsla(0x0D0D0E0F),
            offset: point(px(0.), px(1.)),
            blur_radius: px(2.),
            spread_radius: px(0.),
            inset: false,
        },
        BoxShadow {
            color: ui::hsla(0x0D0D0E14),
            offset: point(px(0.), px(6.)),
            blur_radius: px(18.),
            spread_radius: px(0.),
            inset: false,
        },
    ]
}
