//! The open effect of the Holo look, after the owner's "open" pen
//! (`docs/design/refs/holo/klippo.txt`, `@keyframes materialize`, 1.05 s): a new screen
//! unfolds from a bright slit to full size and burns from white to its own colors.
//!
//! GPUI cannot scale a live element tree, so the effect scales a picture: one frame after a
//! screen change, the renderer draws that frame again into a CPU image (`render_to_image`), the
//! content area is cut out, and for 1.05 s a canvas paints the picture with the pen's scale,
//! brightness and opacity over the live screen. Then the live screen shows again. Where the
//! platform cannot make the picture (Linux), the screen shows at once.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, Context, Pixels, RenderImage, Window, canvas, div, fill, point,
    prelude::*, px, size,
};

use crate::holo_kit::{PA, PAGE, PK, ink};

/// The length of the effect.
pub(super) const OPEN_TIME: f32 = 1.05;

#[derive(Default)]
enum State {
    #[default]
    Idle,
    /// A screen change: the picture comes at the next frame.
    Capture,
    Play {
        image: Arc<RenderImage>,
        start: Instant,
    },
}

/// The open effect of one window.
#[derive(Default)]
pub(super) struct OpenFx {
    /// The screen of the last frame.
    key: Option<String>,
    state: State,
    /// The content area of the last frame, in window pixels.
    area: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// The pen's `--ease`: cubic-bezier(.18, .8, .22, 1), per keyframe step as CSS does.
pub(super) fn ease(x: f32) -> f32 {
    let (x1, y1, x2, y2) = (0.18, 0.8, 0.22, 1.);
    let bez = |a: f32, b: f32, t: f32| {
        3. * a * t * (1. - t).powi(2) + 3. * b * t * t * (1. - t) + t.powi(3)
    };
    let (mut lo, mut hi) = (0., 1.);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.;
        if bez(x1, x2, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y1, y2, (lo + hi) / 2.)
}

/// Scale x, scale y, opacity and brightness at `age` seconds. Keyframes: 0 % (sx .7, sy .03,
/// glow 4), 20 % (sx 1, sy .05, glow 5), 45 % (sx .98, sy 1.04), 70 % (sx 1, sy .98),
/// 100 % (1, 1, glow 1).
pub(super) fn keyframe(age: f32) -> (f32, f32, f32, f32) {
    let f = (age / OPEN_TIME).clamp(0., 1.);
    let steps: [(f32, f32, f32); 5] = [
        (0., 0.7, 0.03),
        (0.2, 1., 0.05),
        (0.45, 0.98, 1.04),
        (0.7, 1., 0.98),
        (1., 1., 1.),
    ];
    let k = steps.iter().rposition(|s| s.0 <= f).unwrap_or(0).min(3);
    let (a, b) = (steps[k], steps[k + 1]);
    let e = ease((f - a.0) / (b.0 - a.0));
    let sx = a.1 + (b.1 - a.1) * e;
    let sy = a.2 + (b.2 - a.2) * e;
    let opacity = ease((f / 0.2).min(1.));
    let glow = if f < 0.2 {
        4. + ease(f / 0.2)
    } else {
        5. - 4. * ease((f - 0.2) / 0.8)
    };
    (sx, sy, opacity, glow)
}

/// Cut `area` (window pixels) out of the frame picture and turn it into an image GPUI draws.
fn picture(window: &mut Window, area: Bounds<Pixels>) -> Option<Arc<RenderImage>> {
    let full = window.render_to_image().ok()?;
    let k = window.scale_factor();
    let x = (f32::from(area.origin.x) * k).max(0.) as u32;
    let y = (f32::from(area.origin.y) * k).max(0.) as u32;
    let w = ((f32::from(area.size.width) * k) as u32).min(full.width().saturating_sub(x));
    let h = ((f32::from(area.size.height) * k) as u32).min(full.height().saturating_sub(y));
    if w == 0 || h == 0 {
        return None;
    }
    let mut cut = image::imageops::crop_imm(&full, x, y, w, h).to_image();
    // GPUI keeps images in BGRA order.
    for px in cut.pixels_mut() {
        px.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(cut)])))
}

impl OpenFx {
    /// Note the screen of this frame. A change starts the effect (not the first screen).
    pub(super) fn screen<V: 'static>(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<V>,
        back: impl Fn(&mut V) -> &mut OpenFx + 'static,
    ) {
        if self.key.as_ref() == Some(&key) {
            return;
        }
        let first = self.key.is_none();
        self.key = Some(key);
        if first {
            return;
        }
        if let State::Play { image, .. } = std::mem::take(&mut self.state) {
            let _ = window.drop_image(image);
        }
        self.state = State::Capture;
        let area = self.area.clone();
        let weak = cx.entity().downgrade();
        window.on_next_frame(move |window, cx: &mut App| {
            let image = area.get().and_then(|a| picture(window, a));
            let _ = weak.update(cx, |v, cx| {
                let fx = back(v);
                if matches!(fx.state, State::Capture) {
                    fx.state = match image {
                        Some(image) => State::Play {
                            image,
                            start: Instant::now(),
                        },
                        None => State::Idle,
                    };
                }
                cx.notify();
            });
        });
    }

    /// The content area with the effect over it.
    pub(super) fn wrap(&mut self, content: AnyElement, window: &mut Window) -> AnyElement {
        let area = self.area.clone();
        let probe = canvas(move |b, _, _| area.set(Some(b)), |_, _, _, _| {})
            .absolute()
            .top_0()
            .left_0()
            .size_full();
        let mut root = div()
            .relative()
            .flex_1()
            .min_h(px(0.))
            .child(content)
            .child(probe);
        let play = match &self.state {
            State::Play { image, start } => Some((image.clone(), start.elapsed().as_secs_f32())),
            _ => None,
        };
        if let Some((image, age)) = play {
            if age >= OPEN_TIME {
                let _ = window.drop_image(image);
                self.state = State::Idle;
            } else {
                root = root.child(cover(image, age));
            }
        }
        root.into_any_element()
    }
}

/// The picture at `age`: the page under it, the scaled picture, the white wash, the edges.
fn cover(image: Arc<RenderImage>, age: f32) -> impl IntoElement {
    let (sx, sy, opacity, glow) = keyframe(age);
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .child(
            canvas(
                |_, _, _| (),
                move |b, _, window, _| {
                    window.paint_quad(fill(b, ink(PAGE, 1.)));
                    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                    let (iw, ih) = (w * sx, h * sy);
                    let at = point(
                        b.origin.x + px((w - iw) / 2.),
                        b.origin.y + px((h - ih) / 2.),
                    );
                    let ib = Bounds::new(at, size(px(iw), px(ih)));
                    let _ = window.paint_image(ib, ib, Default::default(), image.clone(), 0, false);
                    // Opacity: the page over the picture; brightness: a light wash.
                    if opacity < 1. {
                        window.paint_quad(fill(ib, ink(PAGE, 1. - opacity)));
                    }
                    if glow > 1.02 {
                        let g = ((glow - 1.) / 4.).min(1.);
                        window.paint_quad(fill(ib, ink(PA, g * g * 0.45)));
                        let edge = ((glow - 1.) / 4.).clamp(0.3, 1.);
                        for y in [0., ih - 1.5] {
                            let line =
                                Bounds::new(point(at.x, at.y + px(y)), size(px(iw), px(1.5)));
                            window.paint_quad(fill(line, ink(PK, edge)));
                        }
                    }
                },
            )
            .size_full(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_open_effect_starts_as_a_slit_and_ends_at_full_size() {
        let (sx, sy, op, glow) = keyframe(0.);
        assert!((sx - 0.7).abs() < 1e-3 && (sy - 0.03).abs() < 1e-3 && op < 1e-3);
        assert!((glow - 4.).abs() < 1e-3);
        let (_, sy, _, _) = keyframe(OPEN_TIME * 0.45);
        assert!((sy - 1.04).abs() < 1e-3);
        let (sx, sy, op, glow) = keyframe(OPEN_TIME);
        assert!((sx - 1.).abs() < 1e-3 && (sy - 1.).abs() < 1e-3);
        assert!((op - 1.).abs() < 1e-3 && (glow - 1.).abs() < 1e-3);
        assert!(ease(0.).abs() < 1e-3 && (ease(1.) - 1.).abs() < 1e-3);
    }
}
