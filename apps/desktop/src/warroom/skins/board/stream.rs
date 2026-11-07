//! The ribbons of the board: one line per live session, from its codebase module down to its
//! gate. The color is the state of the session. A working session draws a bright line, an
//! idle session a thin muted one. The module under the pointer brings its ribbons forward,
//! and the line under the pointer comes forward alone. With "Motion" on (the default), pulses
//! of light travel along the lines of working sessions; idle lines stay still.

use std::cell::Cell;
use std::rc::Rc;

use aivory_protocol::SessionState;
use gpui::prelude::*;
use gpui::{
    Bounds, Div, FontWeight, PathBuilder, Pixels, canvas, div, fill, point, px, relative, rgb, size,
};

use super::{Gates, gate_of, gate_spans, module_center, module_index, state_tone};
use crate::theme::*;
use crate::ui;

/// The height of the ribbon band.
pub(super) const HEIGHT: f32 = 112.;

/// One session on its way from a module to a gate.
pub(super) struct Ribbon {
    pub session: String,
    pub state: SessionState,
    /// The module (of `Gates::mods`) and the number of modules.
    module: usize,
    modules: usize,
    /// The place in the band of the module and the size of the band.
    rank: usize,
    band: usize,
    /// The gate, the place in it and the sessions in it.
    gate: usize,
    slot: usize,
    count: usize,
    /// The line under the pointer.
    selected: bool,
}

/// The distance between two ribbons that leave a module side by side.
const STEP: f32 = 8.;

impl Ribbon {
    /// The module of the ribbon (`modules` for a codebase without a module card).
    #[cfg(test)]
    pub fn module(&self) -> usize {
        self.module
    }

    /// Start and end x in a row of `width` px.
    pub fn xs(&self, width: f32) -> (f32, f32) {
        let (start, w) = gate_spans(width)[self.gate];
        let n = self.modules.max(1) as f32;
        let mod_w = (width - (n - 1.) * super::GAP) / n;
        let step = STEP.min((mod_w - 32.).max(8.) / self.band.max(1) as f32);
        let off = (self.rank as f32 - (self.band as f32 - 1.) / 2.) * step;
        let module = self.module.min(self.modules.saturating_sub(1));
        // The ends keep 14 px from the gate edges, so they stay over the gate card.
        let inner = (w - 28.).max(1.);
        (
            module_center(width, self.modules, module) + off,
            start + 14. + (self.slot as f32 + 0.5) * inner / self.count.max(1) as f32,
        )
    }
}

/// An event tag with its place (a fraction of the width) and its age.
pub(super) struct TagView {
    pub x: f32,
    pub text: String,
    pub state: SessionState,
    pub age: f32,
}

/// The ribbons of all sessions in the gates. In a gate the ribbons keep the gate order (module
/// order); in a module they leave sorted by gate and place, so no two ribbons of a band cross.
pub(super) fn ribbons(gates: &Gates<'_>, lit: Option<&str>) -> Vec<Ribbon> {
    let mut out = Vec::new();
    for list in &gates.lists {
        for (slot, s) in list.iter().enumerate() {
            out.push(Ribbon {
                session: s.id.clone(),
                state: s.state,
                module: module_index(&gates.mods, &s.codebase),
                modules: gates.mods.len(),
                rank: 0,
                band: 1,
                gate: gate_of(s.state),
                slot,
                count: list.len(),
                selected: lit == Some(s.id.as_str()),
            });
        }
    }
    // Rank the ribbons of each module by their target place.
    for m in 0..=gates.mods.len() {
        let mut idx: Vec<usize> = (0..out.len()).filter(|&i| out[i].module == m).collect();
        idx.sort_by_key(|&i| (out[i].gate, out[i].slot));
        let band = idx.len();
        for (rank, i) in idx.into_iter().enumerate() {
            out[i].rank = rank;
            out[i].band = band;
        }
    }
    out
}

/// How a ribbon draws: color, alpha, stroke width, and a soft glow.
struct Pen {
    color: u32,
    alpha: u32,
    width: f32,
    glow: bool,
}

fn pen(r: &Ribbon, hover: Option<usize>) -> Pen {
    let color = state_tone(r.state);
    let mut p = match r.state {
        SessionState::Working => Pen {
            color,
            alpha: 0xE6,
            width: 2.2,
            glow: true,
        },
        SessionState::NeedsYou | SessionState::Error => Pen {
            color,
            alpha: 0xD9,
            width: 1.8,
            glow: false,
        },
        _ => Pen {
            color,
            alpha: 0x73,
            width: 1.,
            glow: false,
        },
    };
    if r.selected {
        p.alpha = 0xFF;
        p.width += 1.2;
    }
    if let Some(h) = hover {
        if r.module == h {
            p.alpha = 0xFF;
            p.width += 0.8;
        } else {
            p.alpha = 0x24;
            p.glow = false;
        }
    }
    p
}

/// A point of a cubic curve.
fn bezier(p: [(f32, f32); 4], t: f32) -> (f32, f32) {
    let u = 1. - t;
    let w = [u * u * u, 3. * u * u * t, 3. * u * t * t, t * t * t];
    (
        p[0].0 * w[0] + p[1].0 * w[1] + p[2].0 * w[2] + p[3].0 * w[3],
        p[0].1 * w[0] + p[1].1 * w[1] + p[2].1 * w[2] + p[3].1 * w[3],
    )
}

/// The four points of a ribbon: a smooth S from the module to the gate.
fn curve(r: &Ribbon, width: f32) -> [(f32, f32); 4] {
    let (a, b) = r.xs(width);
    let mid = HEIGHT * 0.5;
    [(a, 0.), (a, mid), (b, mid), (b, HEIGHT)]
}

/// Pulses of light per working line.
const PULSES: u32 = 1;
/// The length of a band of light as a part of the curve, and its number of segments.
const BAND: f32 = 0.55;
const BAND_STEPS: u32 = 16;

/// A color between `rgb` and white: `f` 0 keeps it, 1 is white. The core of a light.
fn lighten(rgb: u32, f: f32) -> u32 {
    let ch = |shift: u32| {
        let c = ((rgb >> shift) & 0xFF) as f32;
        ((c + (255. - c) * f).round() as u32).min(255) << shift
    };
    ch(16) | ch(8) | ch(0)
}

/// One band of light centered at `head` (0 = module, 1 = gate): the line lights up softly
/// where the band passes and fades out to both sides, as light moving inside a fiber. No
/// shapes, only the line itself and a faint wide glow.
fn paint_pulse(
    window: &mut gpui::Window,
    at: &dyn Fn((f32, f32)) -> gpui::Point<Pixels>,
    c: [(f32, f32); 4],
    head: f32,
    p: &Pen,
) {
    let lit = lighten(p.color, 0.6);
    // The alpha of the line under the hover dim scales the light too.
    let dim = p.alpha as f32 / 255.;
    for k in 0..BAND_STEPS {
        let f0 = k as f32 / BAND_STEPS as f32;
        let f1 = (k + 1) as f32 / BAND_STEPS as f32;
        let t0 = head - BAND / 2. + BAND * f0;
        let t1 = head - BAND / 2. + BAND * f1;
        if t1 <= 0. || t0 >= 1. {
            continue;
        }
        let (t0, t1) = (t0.max(0.), t1.min(1.));
        // A smooth bell: 0 at both ends of the band, 1 in its middle.
        let bell = (std::f32::consts::PI * (f0 + f1) / 2.).sin().powi(2) * dim;
        let segment = |w: f32| {
            let mut path = PathBuilder::stroke(px(w));
            path.move_to(at(bezier(c, t0)));
            path.line_to(at(bezier(c, (t0 + t1) / 2.)));
            path.line_to(at(bezier(c, t1)));
            path.build().ok()
        };
        // Neon: a wide haze, a tighter glow, and a hot core in a lighter tone.
        for (w, color, a) in [
            (p.width + 14., p.color, 46.),
            (p.width + 6., p.color, 120.),
            (p.width + 1.5, lit, 255.),
        ] {
            if let Some(path) = segment(w) {
                window.paint_path(path, ui::hsla(color << 8 | (bell * a) as u32));
            }
        }
    }
}

/// The draw order: idle first, the highlighted ribbons last.
fn layer(r: &Ribbon, hover: Option<usize>) -> u8 {
    let lit = hover == Some(r.module) || r.selected;
    let base = match r.state {
        SessionState::Idle | SessionState::Parked | SessionState::Done => 0,
        SessionState::Working => 1,
        _ => 2,
    };
    base + if lit { 3 } else { 0 }
}

/// The line under the point `(x, y)` of a band `width` px wide: the nearest line within 6 px.
pub(super) fn hit(ribbons: &[Ribbon], width: f32, x: f32, y: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, r) in ribbons.iter().enumerate() {
        let c = curve(r, width);
        for k in 0..=32 {
            let (px_, py_) = bezier(c, k as f32 / 32.);
            let d = ((px_ - x).powi(2) + (py_ - y).powi(2)).sqrt();
            if d <= 6. && best.is_none_or(|b| d < b.1) {
                best = Some((i, d));
            }
        }
    }
    best.map(|b| b.0)
}

/// The ribbon band with its tags. The paint stores the band bounds in `bounds`.
pub(super) fn render(
    ribbons: Vec<Ribbon>,
    tags: Vec<TagView>,
    hover: Option<usize>,
    motion: bool,
    time: f32,
    bounds_out: Rc<Cell<Option<Bounds<Pixels>>>>,
) -> Div {
    let mut host = div().relative().flex_none().w_full().h(px(HEIGHT)).child(
        canvas(
            move |_, _, _| ribbons,
            move |bounds: Bounds<Pixels>, ribbons, window, _| {
                bounds_out.set(Some(bounds));
                let width = f32::from(bounds.size.width);
                let at = |p: (f32, f32)| {
                    point(
                        bounds.origin.x + px(p.0),
                        bounds.origin.y + px(p.1 * f32::from(bounds.size.height) / HEIGHT),
                    )
                };
                let mut order: Vec<usize> = (0..ribbons.len()).collect();
                order.sort_by_key(|&i| layer(&ribbons[i], hover));
                for i in order {
                    let r = &ribbons[i];
                    let p = pen(r, hover);
                    let c = curve(r, width);
                    let strokes: &[(f32, u32)] = if p.glow {
                        &[(7., 0x14), (0., 0)]
                    } else {
                        &[(0., 0)]
                    };
                    for &(w, a) in strokes {
                        let (w, a) = if w == 0. { (p.width, p.alpha) } else { (w, a) };
                        let mut path = PathBuilder::stroke(px(w));
                        path.move_to(at(c[0]));
                        path.cubic_bezier_to(at(c[3]), at(c[1]), at(c[2]));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, ui::hsla(p.color << 8 | a));
                        }
                    }
                    // The end dot sits on the gate.
                    let end = at(c[3]);
                    let d = (p.width + 2.5).min(6.);
                    let half = px(d / 2.);
                    window.paint_quad(
                        fill(
                            Bounds {
                                origin: point(end.x - half, end.y - px(d)),
                                size: size(px(d), px(d)),
                            },
                            ui::hsla(p.color << 8 | p.alpha),
                        )
                        .corner_radii(half),
                    );
                    if motion && r.state == SessionState::Working {
                        // One soft band of light per line, 3-4 s per trip, lines out of phase.
                        let period = 3. + (i % 5) as f32 * 0.25;
                        for k in 0..PULSES {
                            let head =
                                (time / period + k as f32 / PULSES as f32 + i as f32 * 0.137)
                                    .fract();
                            // The band enters above the card and leaves below the gate: no jump.
                            let head = head * (1. + BAND) - BAND / 2.;
                            paint_pulse(window, &at, c, head, &p);
                        }
                    }
                }
            },
        )
        .absolute()
        .inset_0(),
    );
    if motion {
        for tag in tags {
            let fade_in = (tag.age / 0.3).min(1.);
            let fade_out = (1. - (tag.age - 2.0).max(0.) / 0.6).clamp(0., 1.);
            host = host.child(
                div()
                    .absolute()
                    .left(relative(tag.x))
                    .top(px(HEIGHT * 0.42 - tag.age * 8.))
                    .w(px(0.))
                    .flex()
                    .justify_center()
                    .opacity(fade_in * fade_out)
                    .child(
                        div()
                            .flex_none()
                            .px(px(7.))
                            .rounded_full()
                            .bg(rgb(state_tone(tag.state)))
                            .text_color(rgb(tk().card))
                            .text_size(ui::ts(11.))
                            .line_height(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .child(tag.text),
                    ),
            );
        }
    }
    host
}

/// One legend item: a line sample in the state color and its label.
pub(super) fn legend_item(state: SessionState, label: &'static str) -> Div {
    let (w, h, alpha) = match state {
        SessionState::Working => (18., 3., 0xFF),
        SessionState::Idle => (18., 1., 0x99),
        _ => (18., 2., 0xFF),
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .w(px(w))
                .h(px(h))
                .rounded_full()
                .bg(ui::hsla(state_tone(state) << 8 | alpha)),
        )
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_moves_a_color_toward_white() {
        assert_eq!(lighten(0x000000, 1.), 0xFFFFFF);
        assert_eq!(lighten(0x3574F0, 0.), 0x3574F0);
        assert_eq!(lighten(0x000000, 0.5), 0x808080);
    }

    #[test]
    fn a_curve_runs_from_its_first_to_its_last_point() {
        let p = [(0., 0.), (0., 70.), (100., 80.), (100., 150.)];
        assert_eq!(bezier(p, 0.), (0., 0.));
        assert_eq!(bezier(p, 1.), (100., 150.));
        let mid = bezier(p, 0.5);
        assert!((mid.0 - 50.).abs() < 0.01);
    }

    #[test]
    fn a_point_on_a_line_hits_it_and_a_far_point_hits_nothing() {
        use aivory_protocol::demo::Demo;
        let war = Demo::new().war_room;
        let gates = super::super::split(&war, None);
        let ribbons = ribbons(&gates, None);
        let c = curve(&ribbons[0], 1200.);
        let (x, y) = bezier(c, 0.5);
        let i = hit(&ribbons, 1200., x, y).expect("a line under the point");
        let (hx, hy) = bezier(curve(&ribbons[i], 1200.), 0.5);
        assert!(hit(&ribbons, 1200., hx, hy).is_some());
        assert_eq!(hit(&ribbons, 1200., -100., -100.), None);
    }
}
