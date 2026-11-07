//! Text turned 90 degrees clockwise (`writing-mode: vertical-rl` of the design). GPUI cannot
//! rotate text, but it can rotate an SVG. The glyph outlines of the bundled IBM Plex Sans
//! SemiBold become an SVG path that the asset source serves under [`PREFIX`].

use std::fmt::Write as _;

use ttf_parser::{Face, OutlineBuilder};

/// The asset path prefix of a turned text: `vtext/<size>/<text>.svg`.
pub const PREFIX: &str = "vtext/";

const FONT: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf");

/// The asset path of `text` at `size` px.
pub fn asset_path(text: &str, size: u32) -> String {
    format!("{PREFIX}{size}/{text}.svg")
}

/// The pixel size of the image of `text`: (width, height). The width is the line height.
pub fn measure(text: &str, size: f32) -> (f32, f32) {
    let Ok(face) = Face::parse(FONT, 0) else {
        return (size, size);
    };
    let scale = size / f32::from(face.units_per_em());
    let height: f32 = text.chars().map(|c| advance(&face, c) * scale).sum::<f32>();
    (line_width(&face, scale), height)
}

fn advance(face: &Face, c: char) -> f32 {
    face.glyph_index(c)
        .and_then(|g| face.glyph_hor_advance(g))
        .map_or(0., f32::from)
}

fn line_width(face: &Face, scale: f32) -> f32 {
    (f32::from(face.ascender()) - f32::from(face.descender())) * scale
}

/// The SVG file for an asset path made by [`asset_path`].
pub fn load(path: &str) -> Option<Vec<u8>> {
    let rest = path.strip_prefix(PREFIX)?.strip_suffix(".svg")?;
    let (size, text) = rest.split_once('/')?;
    svg(text, size.parse().ok()?)
}

fn svg(text: &str, size: f32) -> Option<Vec<u8>> {
    let face = Face::parse(FONT, 0).ok()?;
    let scale = size / f32::from(face.units_per_em());
    let (w, h) = measure(text, size);
    let ascender = f32::from(face.ascender()) * scale;
    let mut d = String::new();
    let mut pen = 0.;
    for c in text.chars() {
        if let Some(glyph) = face.glyph_index(c) {
            let mut out = Outline {
                d: &mut d,
                scale,
                pen,
                ascender,
            };
            face.outline_glyph(glyph, &mut out);
        }
        pen += advance(&face, c) * scale;
    }
    Some(
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.2}\" height=\"{h:.2}\" \
             viewBox=\"0 0 {w:.2} {h:.2}\"><path fill=\"#000\" d=\"{d}\"/></svg>"
        )
        .into_bytes(),
    )
}

/// Writes the outline with every point turned: the baseline runs down, the glyph tops point
/// right. A font point (x, y up) lands at (ascender - y, pen + x) of the image.
struct Outline<'a> {
    d: &'a mut String,
    scale: f32,
    pen: f32,
    ascender: f32,
}

impl Outline<'_> {
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (self.ascender - y * self.scale, self.pen + x * self.scale)
    }

    fn put(&mut self, op: char, points: &[(f32, f32)]) {
        let _ = write!(self.d, "{op}");
        for &(x, y) in points {
            let (px, py) = self.point(x, y);
            let _ = write!(self.d, "{px:.2} {py:.2} ");
        }
    }
}

impl OutlineBuilder for Outline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.put('M', &[(x, y)]);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.put('L', &[(x, y)]);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.put('Q', &[(x1, y1), (x, y)]);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.put('C', &[(x1, y1), (x2, y2), (x, y)]);
    }

    fn close(&mut self) {
        self.d.push('Z');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turned_text_is_taller_than_wide() {
        let (w, h) = measure("Done · 5", 13.);
        assert!(h > w, "{w} x {h}");
        let svg = String::from_utf8(load(&asset_path("Done · 5", 13)).unwrap()).unwrap();
        assert!(svg.starts_with("<svg") && svg.contains("<path"));
        assert!(load("icons/mic.svg").is_none());
    }
}
