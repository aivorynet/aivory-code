//! The ground of the village map: grass tiles, soft rivers in the gaps between rows of villages,
//! worn dirt paths, a forest per village, and more woods and rock formations. It is drawn again
//! only when the plots or the sites change.

use image::{Rgba, RgbaImage, imageops};

use super::layout::{Layout, SITE_H, SITE_W, Tool, trail};
use super::sprites;

const PATH_LO: Rgba<u8> = Rgba([104, 84, 54, 255]);
const PATH_HI: Rgba<u8> = Rgba([164, 140, 98, 255]);
const WATER: Rgba<u8> = Rgba([58, 98, 110, 255]);
const FOAM: Rgba<u8> = Rgba([126, 164, 172, 255]);

/// A small seeded random source.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo).max(1) as u64) as i32
    }
}

/// A filled disc.
fn disc(img: &mut RgbaImage, x: i32, y: i32, r: i32, color: Rgba<u8>) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                let (px, py) = (x + dx, y + dy);
                if px >= 0 && py >= 0 && px < w && py < h {
                    img.put_pixel(px as u32, py as u32, color);
                }
            }
        }
    }
}

/// A cheap hash of two integers.
fn hash(x: i32, y: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x85EB_CA6B) ^ (y as u32).wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^ (h >> 13)
}

/// A river in pixel style: stepped edges, a muddy and stony bank that breaks into the grass,
/// light water at the edges, dark water in the middle, and ripples and flecks.
fn river(img: &mut RgbaImage, gy: f32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    for x in 0..w {
        let f = x as f32;
        let mid = (gy + 9. * (f / 94.).sin() + 6. * (f / 38.).sin()) as i32;
        // the banks change in steps of 3 px, so the edge looks stepped and uneven
        let n = (hash(x / 3, 7) % 5) as i32;
        let half = 21 + n + (3. * (f / 61.).sin()) as i32;
        let bank = 6 + (hash(x / 2, 11) % 4) as i32;
        for dy in -(half + bank)..=(half + bank) {
            let y = mid + dy;
            if y < 0 || y >= h {
                continue;
            }
            let d = dy.abs();
            let r = hash(x, y);
            let px = if d > half {
                let o = d - half;
                // the outer bank pixels leave gaps, so the grass grows over the mud
                if o > bank - 3 && !r.is_multiple_of(3) {
                    continue;
                }
                match r % 7 {
                    0 | 1 => Rgba([84, 68, 46, 255]),
                    2 => Rgba([142, 130, 108, 255]),
                    3 => Rgba([128, 112, 82, 255]),
                    _ => Rgba([108, 90, 60, 255]),
                }
            } else {
                let depth = half - d;
                let ripple = (x / 2 + y * 5) % 11 == 0 && r.is_multiple_of(4);
                if depth <= 2 {
                    Rgba([100, 142, 150, 255])
                } else if depth <= 5 {
                    if r.is_multiple_of(6) {
                        FOAM
                    } else {
                        Rgba([78, 120, 130, 255])
                    }
                } else if ripple || r.is_multiple_of(53) {
                    FOAM
                } else if d < half - 12 {
                    if r.is_multiple_of(5) {
                        Rgba([46, 82, 98, 255])
                    } else {
                        Rgba([34, 66, 82, 255])
                    }
                } else if r.is_multiple_of(5) {
                    Rgba([66, 108, 120, 255])
                } else {
                    WATER
                }
            };
            img.put_pixel(x as u32, y as u32, px);
        }
    }
}

/// The ground image of a layout, in map pixels (the size of the layout).
/// A round spot of dirt with a broken edge. The texture itself sets where the edge breaks, so that
/// a chain of spots makes a path with worn, uneven sides.
fn stamp(img: &mut RgbaImage, dirt: &RgbaImage, at: (i32, i32), r: i32, shade: f32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let (tw, th) = (dirt.width() as i32, dirt.height() as i32);
    let reach = r as f32 + 0.5;
    for dy in -r - 3..=r + 3 {
        for dx in -r - 3..=r + 3 {
            let (x, y) = (at.0 + dx, at.1 + dy);
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let wide = dirt.get_pixel(((x / 3) % tw) as u32, ((y / 3) % th) as u32);
            let rough = f32::from(wide.0[1]) / 110. - 0.5;
            let dist = ((dx * dx + dy * dy * 3) as f32).sqrt() / reach;
            if dist + rough * 0.8 > 1. {
                continue;
            }
            let px = dirt.get_pixel((x % tw) as u32, (y % th) as u32);
            let f = |v: u8| (f32::from(v) * shade).min(255.) as u8;
            img.put_pixel(
                x as u32,
                y as u32,
                Rgba([f(px[0]), f(px[1]), f(px[2]), 255]),
            );
        }
    }
}

/// A picture on the ground by its center.
fn lay(img: &mut RgbaImage, sp: &RgbaImage, x: i32, y: i32) {
    imageops::overlay(
        img,
        sp,
        i64::from(x) - i64::from(sp.width() / 2),
        i64::from(y) - i64::from(sp.height() / 2),
    );
}

pub fn ground(l: &Layout) -> RgbaImage {
    let (w, h) = (l.w.ceil() as u32, l.h.ceil() as u32);
    let s = sprites();
    let mut img = RgbaImage::new(w, h);
    let (tw, th) = s.ground.dimensions();
    for y in (0..h).step_by(th as usize) {
        for x in (0..w).step_by(tw as usize) {
            imageops::replace(&mut img, &s.ground, i64::from(x), i64::from(y));
        }
    }
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    // rivers in the gaps between rows of villages; `off` keeps trees out of them
    let mut wet: Vec<(f32, f32)> = Vec::new();
    for gy in &l.gaps {
        wet.push((*gy - 44., *gy + 44.));
        river(&mut img, *gy);
    }
    // field paths: from the main building to each site and each resource
    let mut path: Vec<(i32, i32)> = Vec::new();
    let mut rp = Rng(0x1234_5678_9ABC_DEF1);
    let mut lane = |a: (f32, f32), b: (f32, f32), seed: u32, half: i32, img: &mut RgbaImage| {
        let n = (((b.0 - a.0).abs() + (b.1 - a.1).abs()) / 3.) as i32 + 2;
        let phase = (seed % 97) as f32;
        let mut pts = Vec::new();
        for k in 0..=n {
            let (x, y) = trail(a, b, seed, k as f32 / n as f32);
            // The width changes along the path, and one side wears more than the other.
            let f = k as f32 + phase * 9.;
            let wide = half as f32 + (f / 37.).sin() * 2.2 + (f / 11.).sin() * 1.2;
            let lean = (f / 23.).sin() * 2.;
            pts.push((x as i32, y as i32, wide.max(2.) as i32, lean as i32));
        }
        for &(x, y, w, lean) in &pts {
            stamp(img, &s.dirt, (x + lean, y), w + 2, 0.74);
        }
        for &(x, y, w, lean) in &pts {
            stamp(img, &s.dirt, (x + lean, y), w, 1.);
        }
        // Two worn tracks along the middle.
        for k in 1..pts.len().saturating_sub(1) {
            let (x, y, w, lean) = pts[k];
            let (dx, dy) = (pts[k + 1].0 - pts[k - 1].0, pts[k + 1].1 - pts[k - 1].1);
            let len = ((dx * dx + dy * dy) as f32).sqrt().max(1.);
            let (nx, ny) = (-dy as f32 / len, dx as f32 / len);
            let off = w as f32 * 0.42;
            for side in [-1., 1.] {
                let at = (
                    x + lean + (nx * off * side) as i32,
                    y + (ny * off * side * 0.7) as i32,
                );
                stamp(img, &s.dirt, at, 1, 0.8);
            }
        }
        // Worn tufts on the path and crumbs of soil beside it.
        for &(x, y, w, _) in &pts {
            match rp.next() % 9 {
                0 => {
                    let (dx, dy) = (rp.range(-w, w + 1), rp.range(-w, w + 1));
                    disc(img, x + dx, y + dy, rp.range(1, 3), PATH_LO);
                }
                1 => {
                    let (dx, dy) = (rp.range(-w, w + 1), rp.range(-w, w + 1));
                    disc(img, x + dx, y + dy, 1, PATH_HI);
                }
                2 => {
                    let (dx, dy) = (rp.range(-w - 7, w + 8), rp.range(-w - 5, w + 6));
                    disc(img, x + dx, y + dy, rp.range(0, 2), PATH_LO);
                }
                _ => {}
            }
        }
        path.extend(pts.iter().map(|p| (p.0, p.1)));
    };
    let mut taken: Vec<(f32, f32, f32, f32)> = Vec::new();
    for v in &l.villages {
        let door = v.door();
        lane(door, v.resource(Tool::Run), 11, 6, &mut img);
        lane(door, v.resource(Tool::Read), 12, 6, &mut img);
        lane(door, v.resource(Tool::Web), 13, 6, &mut img);
        taken.push((v.cx - 170., v.cy - 220., v.cx + 170., v.cy + 150.));
        taken.push((
            v.quarry.0 - 110.,
            v.quarry.1 - 90.,
            v.quarry.0 + 110.,
            v.quarry.1 + 70.,
        ));
        taken.push((
            v.well.0 - 60.,
            v.well.1 - 50.,
            v.well.0 + 60.,
            v.well.1 + 50.,
        ));
        for it in &v.items {
            if let (Some(yard), Some((sx, sy))) = (it.yard(), it.site) {
                lane(door, yard, it.seed, 6, &mut img);
                taken.push((sx - 24., sy - 34., sx + SITE_W + 24., sy + SITE_H + 34.));
            }
        }
    }
    // roads from village to village in a row
    for pair in l.villages.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if (a.cy - b.cy).abs() < 1. && b.cx > a.cx {
            lane(a.door(), b.door(), 2, 7, &mut img);
        }
    }
    let free = |x: f32, y: f32| {
        !taken
            .iter()
            .any(|t| x > t.0 && x < t.2 && y > t.1 && y < t.3)
            && !wet.iter().any(|g| y > g.0 && y < g.1)
            && !path
                .iter()
                .step_by(5)
                .any(|p| (p.0 as f32 - x).abs() < 24. && (p.1 as f32 - y).abs() < 20.)
    };
    // A wood or a rock formation fits where its whole footprint is free and no other one stands.
    // The ones further down are drawn later.
    let mut things: Vec<(i32, i32, &RgbaImage)> = Vec::new();
    for (k, v) in l.villages.iter().enumerate() {
        // The forest stands behind the end of its path.
        things.push((v.forest.0 as i32, v.forest.1 as i32 - 74, &s.forests[k % 3]));
    }
    let foot = |t: &(i32, i32, &RgbaImage)| {
        (
            t.1 as f32 + t.2.height() as f32 * 0.1,
            t.2.width() as f32 * 0.42,
            t.2.height() as f32 * 0.3,
        )
    };
    let room = |t: &(i32, i32, &RgbaImage), all: &[(i32, i32, &RgbaImage)]| {
        let (cy, rx, ry) = foot(t);
        let cx = t.0 as f32;
        let corners = [(0., 0.), (-rx, 0.), (rx, 0.), (0., -ry), (0., ry)];
        corners.iter().all(|c| free(cx + c.0, cy + c.1))
            && all.iter().all(|o| {
                let (oy, orx, ory) = foot(o);
                (o.0 as f32 - cx).abs() > (rx + orx) * 0.9 || (oy - cy).abs() > (ry + ory) * 0.9
            })
    };
    let tries = (w * h / 12_000).max(60);
    for k in 0..tries {
        let sp = match k % 5 {
            0 => &s.forests[r.range(0, 3) as usize],
            1 | 2 => &s.groves[r.range(0, 3) as usize],
            _ => &s.rocks[r.range(0, 3) as usize],
        };
        let t = (r.range(0, w as i32), r.range(0, h as i32), sp);
        if room(&t, &things) {
            things.push(t);
        }
    }
    things.sort_by_key(|t| t.1);
    for (x, y, sp) in things {
        lay(&mut img, sp, x, y);
    }
    img
}

/// The ground scaled down for the radar.
pub fn radar(ground: &RgbaImage, max_w: u32, max_h: u32) -> RgbaImage {
    let (w, h) = ground.dimensions();
    let s = (max_w as f32 / w as f32).min(max_h as f32 / h as f32);
    let mut small = imageops::resize(
        ground,
        ((w as f32 * s) as u32).max(1),
        ((h as f32 * s) as u32).max(1),
        imageops::FilterType::Triangle,
    );
    for p in small.pixels_mut() {
        for k in 0..3 {
            p[k] = (u32::from(p[k]) * 5 / 4).min(255) as u8;
        }
    }
    small
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rts::layout::Village;

    #[test]
    fn ground_has_the_size_of_the_layout() {
        let l = Layout {
            w: 400.,
            h: 300.,
            villages: vec![Village {
                name: "a".into(),
                x: 40.,
                y: 40.,
                w: 200.,
                h: 120.,
                cx: 140.,
                cy: 100.,
                forest: (60., 60.),
                quarry: (60., 140.),
                well: (220., 60.),
                slots: "2 FREE".into(),
                free: true,
                items: Vec::new(),
            }],
            gaps: vec![250.],
        };
        let g = ground(&l);
        assert_eq!(g.dimensions(), (400, 300));
    }
}
