//! The Command look (boards `docs/design/looks/rts/`): an isometric building game. Each codebase is
//! a village, each session a worker robot, each task a house that its robot builds (`MAPPING.md`).
//! All sprites come from one pipeline: Kenney "Fantasy Town Kit" models, one camera and one light
//! (`render_village.py`), repainted as realistic pixel art (`repaint.py`). A sprite is stored at 2x.
//!
//! This module holds the image side: the sprites, 9-slice frames, the terrain and the layout.

pub mod layout;
pub mod terrain;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use gpui::RenderImage;
use image::{Frame, RgbaImage, imageops};

/// One sprite: the pixels and the image that GPUI draws.
pub struct Sprite {
    pub rgba: RgbaImage,
    pub render: Arc<RenderImage>,
}

impl Sprite {
    fn new(rgba: RgbaImage) -> Self {
        let render = to_render(&rgba);
        Self { rgba, render }
    }

    /// The width on the map (the image is 2x).
    pub fn w(&self) -> f32 {
        self.rgba.width() as f32 / 2.
    }

    /// The height on the map (the image is 2x).
    pub fn h(&self) -> f32 {
        self.rgba.height() as f32 / 2.
    }
}

/// All the sprites of the look.
pub struct Sprites {
    /// The main building of a village.
    pub hall: Sprite,
    /// The timber frame sites (a raw house: the work in progress).
    pub sites: [Sprite; 4],
    /// The finished cottages: a Claude, Codex, Gemini or other session.
    pub cottages: [Sprite; 4],
    pub quarry: Sprite,
    pub well: Sprite,
    /// The ground tiles and the woods and rocks at map size (pixels only: the ground image
    /// holds them).
    pub ground: RgbaImage,
    pub dirt: RgbaImage,
    pub forests: [RgbaImage; 3],
    pub groves: [RgbaImage; 3],
    pub rocks: [RgbaImage; 3],
    /// The robot in the sidebar.
    pub bot: Sprite,
    /// The frames of the robot on the map (`robot`).
    robots: Vec<Sprite>,
    pub panel: RgbaImage,
    pub button: RgbaImage,
}

impl Sprites {
    /// The finished cottage of a provider.
    pub fn cottage(&self, provider: &str) -> &Sprite {
        &self.cottages[match provider {
            "claude" => 1,
            "codex" => 0,
            "gemini" => 2,
            _ => 3,
        }]
    }

    /// A frame of the robot on the map: `dir` from 0 (south) clockwise on the screen to 7.
    pub fn robot(&self, dir: usize, frame: usize) -> &Sprite {
        &self.robots[(dir % 8) * ROBOT_FRAMES + frame % ROBOT_FRAMES]
    }

    /// A site, from the seed of its session.
    pub fn site(&self, seed: u32) -> &Sprite {
        &self.sites[seed as usize % self.sites.len()]
    }
}

/// The frames of a direction: 6 of a walk, 6 of a hammer blow, 1 at rest.
const ROBOT_FRAMES: usize = 13;
pub const ROBOT_WALK: usize = 0;
pub const ROBOT_WORK: usize = 6;
pub const ROBOT_REST: usize = 12;
/// A cell of the robot sheet in map pixels, and the foot point in the cell.
pub const ROBOT_CELL: f32 = 72.;
pub const ROBOT_FOOT: (f32, f32) = (36., 57.);

/// The eight directions from the five rows of the sheet: the west side is mirrored.
fn robot_frames(sheet: &RgbaImage) -> Vec<Sprite> {
    let cell = ROBOT_CELL as u32 * 2;
    let mut out = Vec::with_capacity(8 * ROBOT_FRAMES);
    for dir in 0..8 {
        let row = if dir < 5 { dir } else { 8 - dir };
        for f in 0..ROBOT_FRAMES as u32 {
            let px = imageops::crop_imm(sheet, f * cell, row * cell, cell, cell).to_image();
            out.push(Sprite::new(if dir < 5 {
                px
            } else {
                imageops::flip_horizontal(&px)
            }));
        }
    }
    out
}

fn decode(bytes: &[u8]) -> RgbaImage {
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map(|i| i.to_rgba8())
        .unwrap_or_else(|_| RgbaImage::new(1, 1))
}

/// A GPUI image of RGBA pixels (GPUI keeps images in BGRA order).
pub fn to_render(rgba: &RgbaImage) -> Arc<RenderImage> {
    let mut bgra = rgba.clone();
    for p in bgra.pixels_mut() {
        p.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![Frame::new(bgra)]))
}

/// A 2x image at map size.
fn half(bytes: &[u8]) -> RgbaImage {
    let px = decode(bytes);
    let (w, h) = px.dimensions();
    imageops::resize(&px, w / 2, h / 2, imageops::FilterType::Triangle)
}

/// The sprites, decoded once.
pub fn sprites() -> &'static Sprites {
    static S: OnceLock<Sprites> = OnceLock::new();
    S.get_or_init(|| {
        let button = decode(include_bytes!("../../assets/rts/uibutton.png"));
        let site = |b: &[u8]| Sprite::new(decode(b));
        Sprites {
            hall: site(include_bytes!("../../assets/rts/village/hall.png")),
            sites: [
                site(include_bytes!("../../assets/rts/village/site0.png")),
                site(include_bytes!("../../assets/rts/village/site1.png")),
                site(include_bytes!("../../assets/rts/village/site2.png")),
                site(include_bytes!("../../assets/rts/village/site3.png")),
            ],
            cottages: [
                site(include_bytes!("../../assets/rts/village/cottage0.png")),
                site(include_bytes!("../../assets/rts/village/cottage1.png")),
                site(include_bytes!("../../assets/rts/village/cottage2.png")),
                site(include_bytes!("../../assets/rts/village/cottage3.png")),
            ],
            quarry: site(include_bytes!("../../assets/rts/village/quarry.png")),
            well: site(include_bytes!("../../assets/rts/village/well.png")),
            ground: decode(include_bytes!("../../assets/rts/village/ground.png")),
            dirt: decode(include_bytes!("../../assets/rts/village/dirt.png")),
            forests: [
                half(include_bytes!("../../assets/rts/village/forest0.png")),
                half(include_bytes!("../../assets/rts/village/forest1.png")),
                half(include_bytes!("../../assets/rts/village/forest2.png")),
            ],
            groves: [
                half(include_bytes!("../../assets/rts/village/grove0.png")),
                half(include_bytes!("../../assets/rts/village/grove1.png")),
                half(include_bytes!("../../assets/rts/village/grove2.png")),
            ],
            rocks: [
                half(include_bytes!("../../assets/rts/village/rock0.png")),
                half(include_bytes!("../../assets/rts/village/rock1.png")),
                half(include_bytes!("../../assets/rts/village/rock2.png")),
            ],
            bot: site(include_bytes!("../../assets/rts/village/bot.png")),
            robots: robot_frames(&decode(include_bytes!(
                "../../assets/rts/village/robots.png"
            ))),
            panel: decode(include_bytes!("../../assets/rts/uipanel.png")),
            button,
        }
    })
}

/// The kinds of 9-slice frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Frame9 {
    /// The wood panel (windows and the sidebar); the center keeps its texture.
    Panel,
    /// A wood button.
    Button,
    /// A lit (green) button: the screen on view, the main answer.
    ButtonOn,
}

/// Paste `src` stretched (nearest) into `out` at the box.
fn put(
    out: &mut RgbaImage,
    src: &RgbaImage,
    region: (u32, u32, u32, u32),
    at: (u32, u32, u32, u32),
) {
    let (x, y, w, h) = region;
    let (dx, dy, dw, dh) = at;
    if dw == 0 || dh == 0 || w == 0 || h == 0 {
        return;
    }
    let piece = imageops::crop_imm(src, x, y, w, h).to_image();
    let piece = imageops::resize(&piece, dw, dh, imageops::FilterType::Nearest);
    imageops::overlay(out, &piece, i64::from(dx), i64::from(dy));
}

/// A 9-slice of `src` with margin `m`: corners kept, edges stretched from the middle 1 px strip.
pub fn nine(src: &RgbaImage, w: u32, h: u32, m: u32, tile_center: bool) -> RgbaImage {
    let (sw, sh) = src.dimensions();
    let (w, h) = (w.max(2 * m + 1), h.max(2 * m + 1));
    let (cx, cy) = (sw / 2, sh / 2);
    let mut out = RgbaImage::new(w, h);
    if tile_center {
        let tex = imageops::crop_imm(src, 30, 30, sw - 60, sh - 60).to_image();
        let (tw, th) = tex.dimensions();
        let mut y = m;
        while y < h - m {
            let mut x = m;
            while x < w - m {
                let part =
                    imageops::crop_imm(&tex, 0, 0, tw.min(w - m - x), th.min(h - m - y)).to_image();
                imageops::overlay(&mut out, &part, i64::from(x), i64::from(y));
                x += tw;
            }
            y += th;
        }
    } else {
        put(&mut out, src, (cx, cy, 1, 1), (m, m, w - 2 * m, h - 2 * m));
    }
    put(&mut out, src, (cx, 0, 1, m), (m, 0, w - 2 * m, m));
    put(&mut out, src, (cx, sh - m, 1, m), (m, h - m, w - 2 * m, m));
    put(&mut out, src, (0, cy, m, 1), (0, m, m, h - 2 * m));
    put(&mut out, src, (sw - m, cy, m, 1), (w - m, m, m, h - 2 * m));
    for (sx, sy, dx, dy) in [
        (0, 0, 0, 0),
        (sw - m, 0, w - m, 0),
        (0, sh - m, 0, h - m),
        (sw - m, sh - m, w - m, h - m),
    ] {
        put(&mut out, src, (sx, sy, m, m), (dx, dy, m, m));
    }
    out
}

/// The made frames by kind and size.
type FrameCache = HashMap<(Frame9, u32, u32), Arc<RenderImage>>;

/// A 9-slice frame of a size, made once per size.
pub fn frame(kind: Frame9, w: f32, h: f32) -> Arc<RenderImage> {
    static CACHE: OnceLock<Mutex<FrameCache>> = OnceLock::new();
    let (w, h) = (w.max(8.).round() as u32, h.max(8.).round() as u32);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock()
        && let Some(img) = map.get(&(kind, w, h))
    {
        return img.clone();
    }
    let s = sprites();
    let mut px = match kind {
        Frame9::Panel => nine(&s.panel, w, h, 22, true),
        Frame9::Button | Frame9::ButtonOn => nine(&s.button, w, h, 14, false),
    };
    if kind == Frame9::ButtonOn {
        for p in px.pixels_mut() {
            p[0] = ((u32::from(p[0]) * 55 + 110 * 45) / 100) as u8;
            p[1] = ((u32::from(p[1]) * 55 + 190 * 45) / 100) as u8;
            p[2] = ((u32::from(p[2]) * 55 + 70 * 45) / 100) as u8;
        }
    }
    let img = to_render(&px);
    if let Ok(mut map) = cache.lock() {
        if map.len() > 64 {
            map.clear();
        }
        map.insert((kind, w, h), img.clone());
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn nine_slice_keeps_the_size() {
        let src = RgbaImage::from_pixel(60, 60, Rgba([10, 20, 30, 255]));
        let out = nine(&src, 200, 44, 14, false);
        assert_eq!(out.dimensions(), (200, 44));
        assert_eq!(out.get_pixel(100, 22)[0], 10);
    }

    #[test]
    fn sprites_decode() {
        let s = sprites();
        assert!(s.hall.w() > 50.);
        assert!(s.bot.h() > 20.);
        assert_eq!(s.cottage("codex").rgba, s.cottages[0].rgba);
        assert_eq!(s.cottage("other").rgba, s.cottages[3].rgba);
        assert!(s.site(7).w() > 100.);
    }
}
