//! The canvas painter that the skins share: batched strokes, the neon double stroke,
//! polygons, rings, arcs, dots and cached shaped text. Colors are `0xRRGGBB` plus an alpha
//! from 0 to 1.

use std::collections::HashMap;

use gpui::{
    App, Bounds, Font, FontWeight, Hsla, PathBuilder, Pixels, ShapedLine, TextAlign, TextRun,
    Window, fill, font, point, px, size,
};

use crate::ui;

/// The most segments in one path (the vertex index of a path has 16 bits).
const CHUNK: usize = 100;
/// The most shaped lines that the cache keeps.
const KEEP: usize = 6000;

/// A color `0xRRGGBB` with an alpha of 0 to 1.
pub(in crate::warroom) fn ink(rgb: u32, alpha: f32) -> Hsla {
    ui::hsla(rgb << 8 | (alpha.clamp(0., 1.) * 255.) as u32)
}

/// Text alignment around the anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::warroom) enum Align {
    Left,
    Center,
    Right,
}

/// The key of one shaped line.
type Key = (String, u32, u32, u8, &'static str, bool);

/// Shaped lines that live between frames.
#[derive(Default)]
pub(in crate::warroom) struct TextCache {
    lines: HashMap<Key, ShapedLine>,
}

/// The painter of one frame.
pub(in crate::warroom) struct Painter<'a> {
    pub window: &'a mut Window,
    pub cx: &'a mut App,
    cache: &'a mut TextCache,
}

impl<'a> Painter<'a> {
    pub fn new(window: &'a mut Window, cx: &'a mut App, cache: &'a mut TextCache) -> Self {
        Self { window, cx, cache }
    }

    /// Stroke line segments of one color in as few paths as possible.
    pub fn strokes(&mut self, segs: &[[f32; 4]], rgb: u32, width: f32, alpha: f32) {
        for chunk in segs.chunks(CHUNK) {
            let mut path = PathBuilder::stroke(px(width));
            for s in chunk {
                path.move_to(point(px(s[0]), px(s[1])));
                path.line_to(point(px(s[2]), px(s[3])));
            }
            if let Ok(path) = path.build() {
                self.window.paint_path(path, ink(rgb, alpha));
            }
        }
    }

    /// The neon line: a wide faint stroke under a thin bright one.
    pub fn neon(&mut self, segs: &[[f32; 4]], rgb: u32, width: f32, alpha: f32) {
        self.strokes(segs, rgb, width * 5., alpha * 0.16);
        self.strokes(segs, rgb, width, alpha);
    }

    /// A polyline through `pts`, closed on request.
    pub fn polyline(&mut self, pts: &[[f32; 2]], rgb: u32, width: f32, alpha: f32, closed: bool) {
        let mut path = PathBuilder::stroke(px(width));
        let Some(first) = pts.first() else {
            return;
        };
        path.move_to(point(px(first[0]), px(first[1])));
        for p in &pts[1..] {
            path.line_to(point(px(p[0]), px(p[1])));
        }
        if closed {
            path.close();
        }
        if let Ok(path) = path.build() {
            self.window.paint_path(path, ink(rgb, alpha));
        }
    }

    /// Fill a convex polygon.
    pub fn polygon(&mut self, pts: &[[f32; 2]], rgb: u32, alpha: f32) {
        self.polygon_bg(pts, ink(rgb, alpha));
    }

    /// Fill a convex polygon with a color or a gradient.
    pub fn polygon_bg(&mut self, pts: &[[f32; 2]], color: impl Into<gpui::Background>) {
        if pts.is_empty() {
            return;
        }
        let mut path = PathBuilder::fill();
        let pts: Vec<_> = pts.iter().map(|p| point(px(p[0]), px(p[1]))).collect();
        path.add_polygon(&pts, true);
        if let Ok(path) = path.build() {
            self.window.paint_path(path, color);
        }
    }

    /// An axis-aligned filled rect.
    pub fn rect(&mut self, (x, y, w, h): (f32, f32, f32, f32), rgb: u32, alpha: f32) {
        let b = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
        self.window.paint_quad(fill(b, ink(rgb, alpha)));
    }

    /// `s` cut with a trailing `…` so that it is at most `max_w` pixels wide.
    pub fn fit(
        &mut self,
        s: &str,
        size: f32,
        family: &'static str,
        bold: bool,
        max_w: f32,
    ) -> String {
        let width =
            |p: &mut Self, t: &str| f32::from(p.shaped(t, size, family, bold, 0, 1.).width());
        if width(self, s) <= max_w {
            return s.to_string();
        }
        let chars: Vec<char> = s.chars().collect();
        let (mut lo, mut hi) = (0, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let t: String = chars[..mid].iter().collect::<String>() + "…";
            if width(self, &t) <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let cut: String = chars[..lo].iter().collect();
        format!("{}…", cut.trim_end())
    }

    /// An arc of `r` around `c` from `a.0` to `a.1` radians (a ring for 0 to TAU). The points
    /// go through `map` first (a projection; the identity for a flat canvas).
    #[allow(clippy::too_many_arguments)]
    pub fn arc(
        &mut self,
        c: [f32; 2],
        r: f32,
        a: (f32, f32),
        width: f32,
        rgb: u32,
        alpha: f32,
        map: &dyn Fn([f32; 2]) -> [f32; 2],
    ) {
        let steps = (((a.1 - a.0).abs() / 0.08) as usize).max(2);
        let pts: Vec<[f32; 2]> = (0..=steps)
            .map(|i| {
                let ang = a.0 + (a.1 - a.0) * i as f32 / steps as f32;
                map([c[0] + ang.cos() * r, c[1] + ang.sin() * r])
            })
            .collect();
        self.polyline(&pts, rgb, width, alpha, false);
    }

    /// A round dot.
    pub fn dot(&mut self, at: [f32; 2], radius: f32, rgb: u32, alpha: f32) {
        let b = Bounds::new(
            point(px(at[0] - radius), px(at[1] - radius)),
            size(px(radius * 2.), px(radius * 2.)),
        );
        self.window
            .paint_quad(fill(b, ink(rgb, alpha)).corner_radii(px(radius)));
    }

    /// One line of shaped text, from the cache. `size` is in pixels.
    pub fn shaped(
        &mut self,
        s: &str,
        size: f32,
        family: &'static str,
        bold: bool,
        rgb: u32,
        alpha: f32,
    ) -> ShapedLine {
        let key: Key = (
            s.to_string(),
            (size * 2.) as u32,
            rgb,
            (alpha.clamp(0., 1.) * 255.) as u8,
            family,
            bold,
        );
        if let Some(line) = self.cache.lines.get(&key) {
            return line.clone();
        }
        if self.cache.lines.len() > KEEP {
            self.cache.lines.clear();
        }
        let run = TextRun {
            len: s.len(),
            font: Font {
                weight: if bold {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                },
                ..font(family)
            },
            color: ink(rgb, alpha),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line =
            self.window
                .text_system()
                .shape_line(s.to_string().into(), px(size), &[run], None);
        self.cache.lines.insert(key, line.clone());
        line
    }

    /// Paint text of `size` pixels with the line box `lh`. `y` is the top of the line.
    /// Returns the width.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        s: &str,
        size: f32,
        lh: f32,
        family: &'static str,
        bold: bool,
        rgb: u32,
        alpha: f32,
        (x, y): (f32, f32),
        align: Align,
    ) -> f32 {
        if s.is_empty() || alpha <= 0. {
            return 0.;
        }
        let line = self.shaped(s, size, family, bold, rgb, alpha);
        let w = f32::from(line.width());
        let x0 = match align {
            Align::Left => x,
            Align::Center => x - w / 2.,
            Align::Right => x - w,
        };
        self.paint_line(&line, (x0, y), lh);
        w
    }

    /// Paint a shaped line with its top left corner at `at`.
    pub fn paint_line(&mut self, line: &ShapedLine, at: (f32, f32), lh: f32) {
        let _ = line.paint(
            point(px(at.0), px(at.1)),
            px(lh),
            TextAlign::Left,
            None,
            self.window,
            self.cx,
        );
    }

    /// The pixel size of a point size of the interface scale.
    pub fn scaled(size: f32) -> f32 {
        let p: Pixels = ui::ts(size);
        f32::from(p)
    }
}
