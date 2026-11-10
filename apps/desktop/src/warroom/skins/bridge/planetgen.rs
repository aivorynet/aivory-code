//! The procedural planet of the Bridge war room: a new planet for each day, the same planet for
//! the whole day. Three pictures come from one seed: the globe, the cloud strip and the close-up
//! of each landing site. All three read the same height field on the unit sphere, so a site crop
//! shows the real terrain under its site on the globe. The noise is original code.

use image::{Rgba, RgbaImage};

/// The size of the globe picture, of the cloud strip and of a site crop.
pub(super) const GLOBE_PX: u32 = 1400;
pub(super) const CLOUD_W: u32 = 2200;
pub(super) const CROP_W: u32 = 1866;
pub(super) const CROP_H: u32 = 1236;

/// The disc radius in globe space and the globe space itself (see `planet.rs`).
const DISC_R: f32 = 390.;
const SPACE_W: f32 = 1244.;
const SPACE_H: f32 = 824.;
/// Crop pixels for one unit of globe space, and globe units for one crop unit.
const CROP_SCALE: f32 = CROP_W as f32 / SPACE_W;
const CROP_ZOOM: f32 = 12.;

/// The seed of a calendar day: the days since 1970 of the local date.
pub(super) fn today_seed() -> u32 {
    use chrono::Datelike;
    let days = chrono::Local::now().date_naive().num_days_from_ce() - 719_163;
    days.max(0) as u32
}

// ---- noise ---------------------------------------------------------------------------------

type V3 = [f32; 3];

fn hash(x: i32, y: i32, z: i32, s: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (y as u32).wrapping_mul(0x85EB_CA77)
        ^ (z as u32).wrapping_mul(0xC2B2_AE3D)
        ^ s.wrapping_mul(0x27D4_EB2F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

const GRAD: [V3; 12] = [
    [1., 1., 0.],
    [-1., 1., 0.],
    [1., -1., 0.],
    [-1., -1., 0.],
    [1., 0., 1.],
    [-1., 0., 1.],
    [1., 0., -1.],
    [-1., 0., -1.],
    [0., 1., 1.],
    [0., -1., 1.],
    [0., 1., -1.],
    [0., -1., -1.],
];

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6. - 15.) + 10.)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// Gradient noise in 3D, about -1 to 1.
fn perlin(p: V3, s: u32) -> f32 {
    let f = [p[0].floor(), p[1].floor(), p[2].floor()];
    let (xi, yi, zi) = (f[0] as i32, f[1] as i32, f[2] as i32);
    let (x, y, z) = (p[0] - f[0], p[1] - f[1], p[2] - f[2]);
    let corner = |dx: i32, dy: i32, dz: i32| {
        let g = GRAD[(hash(xi + dx, yi + dy, zi + dz, s) % 12) as usize];
        g[0] * (x - dx as f32) + g[1] * (y - dy as f32) + g[2] * (z - dz as f32)
    };
    let (u, v, w) = (fade(x), fade(y), fade(z));
    let x0 = lerp(corner(0, 0, 0), corner(1, 0, 0), u);
    let x1 = lerp(corner(0, 1, 0), corner(1, 1, 0), u);
    let x2 = lerp(corner(0, 0, 1), corner(1, 0, 1), u);
    let x3 = lerp(corner(0, 1, 1), corner(1, 1, 1), u);
    lerp(lerp(x0, x1, v), lerp(x2, x3, v), w) * 0.85
}

/// A turn of the sample point between the octaves, so the lattice never shows.
fn turn(p: V3) -> V3 {
    [
        (0.80 * p[0] + 0.60 * p[1]) * 2.03,
        (-0.48 * p[0] + 0.64 * p[1] + 0.60 * p[2]) * 2.03,
        (0.36 * p[0] - 0.48 * p[1] + 0.80 * p[2]) * 2.03,
    ]
}

/// Fractal noise, about -1 to 1.
fn fbm(mut p: V3, s: u32, oct: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0., 1., 0.);
    for k in 0..oct {
        sum += amp * perlin(p, s.wrapping_add(k * 31));
        norm += amp;
        amp *= 0.5;
        p = turn(p);
    }
    sum / norm
}

/// Ridged noise: sharp crests, 0 to 1.
fn ridged(mut p: V3, s: u32, oct: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut weight) = (0., 1., 0., 1.);
    for k in 0..oct {
        let r = 1. - perlin(p, s.wrapping_add(k * 17)).abs().min(1.);
        let r = r * r * weight;
        weight = (r * 1.6).clamp(0., 1.);
        sum += amp * r;
        norm += amp;
        amp *= 0.5;
        p = turn(p);
    }
    sum / norm
}

fn add(a: V3, b: V3, k: f32) -> V3 {
    [a[0] + b[0] * k, a[1] + b[1] * k, a[2] + b[2] * k]
}

fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: V3) -> V3 {
    let l = dot(a, a).sqrt().max(1e-6);
    scale(a, 1. / l)
}

fn mix(a: V3, b: V3, t: f32) -> V3 {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

// ---- the planet ----------------------------------------------------------------------------

/// The colours and the climate of one kind of planet.
struct Palette {
    deep: V3,
    shallow: V3,
    sand: V3,
    dry: V3,
    green: V3,
    forest: V3,
    rock: V3,
    snow: V3,
    ice_sea: V3,
    /// The sea level, the wetness and the warmth of the land, the edge of the polar cap.
    sea: f32,
    moist: f32,
    warm: f32,
    cap: f32,
    cloud: f32,
}

const fn c(r: u8, g: u8, b: u8) -> V3 {
    [r as f32 / 255., g as f32 / 255., b as f32 / 255.]
}

const PALETTES: [Palette; 7] = [
    // Earth-like.
    Palette {
        deep: c(8, 34, 88),
        shallow: c(36, 138, 166),
        sand: c(206, 186, 132),
        dry: c(172, 150, 96),
        green: c(84, 126, 58),
        forest: c(36, 84, 50),
        rock: c(118, 104, 94),
        snow: c(238, 242, 247),
        ice_sea: c(214, 230, 242),
        sea: 0.10,
        moist: 0.0,
        warm: 0.0,
        cap: 0.93,
        cloud: 0.36,
    },
    // Desert.
    Palette {
        deep: c(16, 50, 84),
        shallow: c(62, 150, 150),
        sand: c(224, 192, 132),
        dry: c(208, 152, 88),
        green: c(158, 134, 74),
        forest: c(116, 100, 62),
        rock: c(136, 88, 66),
        snow: c(242, 236, 226),
        ice_sea: c(220, 228, 232),
        sea: -0.12,
        moist: -0.55,
        warm: 0.25,
        cap: 0.97,
        cloud: 0.18,
    },
    // Ice.
    Palette {
        deep: c(12, 38, 78),
        shallow: c(76, 144, 176),
        sand: c(204, 212, 222),
        dry: c(178, 192, 206),
        green: c(142, 166, 182),
        forest: c(102, 130, 150),
        rock: c(94, 100, 116),
        snow: c(246, 249, 253),
        ice_sea: c(226, 238, 248),
        sea: 0.06,
        moist: 0.0,
        warm: -0.7,
        cap: 0.62,
        cloud: 0.3,
    },
    // Ocean world.
    Palette {
        deep: c(6, 30, 86),
        shallow: c(28, 150, 180),
        sand: c(214, 200, 150),
        dry: c(170, 156, 100),
        green: c(70, 132, 68),
        forest: c(30, 90, 56),
        rock: c(112, 104, 98),
        snow: c(240, 244, 248),
        ice_sea: c(214, 232, 244),
        sea: 0.2,
        moist: 0.25,
        warm: 0.05,
        cap: 0.95,
        cloud: 0.42,
    },
    // Red and ochre.
    Palette {
        deep: c(8, 48, 80),
        shallow: c(44, 150, 158),
        sand: c(218, 166, 112),
        dry: c(192, 108, 62),
        green: c(152, 94, 56),
        forest: c(108, 62, 46),
        rock: c(92, 62, 56),
        snow: c(246, 228, 218),
        ice_sea: c(236, 214, 206),
        sea: -0.04,
        moist: -0.3,
        warm: 0.1,
        cap: 0.94,
        cloud: 0.22,
    },
    // Lush.
    Palette {
        deep: c(6, 46, 82),
        shallow: c(24, 160, 150),
        sand: c(216, 202, 152),
        dry: c(142, 162, 72),
        green: c(52, 152, 62),
        forest: c(20, 96, 56),
        rock: c(102, 92, 82),
        snow: c(244, 246, 248),
        ice_sea: c(216, 234, 238),
        sea: 0.05,
        moist: 0.55,
        warm: 0.1,
        cap: 0.96,
        cloud: 0.4,
    },
    // Twilight.
    Palette {
        deep: c(26, 20, 78),
        shallow: c(96, 104, 190),
        sand: c(214, 192, 184),
        dry: c(172, 142, 152),
        green: c(112, 140, 134),
        forest: c(62, 98, 112),
        rock: c(96, 82, 108),
        snow: c(242, 238, 252),
        ice_sea: c(222, 218, 246),
        sea: 0.08,
        moist: 0.1,
        warm: -0.1,
        cap: 0.92,
        cloud: 0.32,
    },
];

/// A tiny random stream from the seed.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = hash(self.0 as i32, 7, 13, 0x51ED);
        (self.0 >> 8) as f32 / 16_777_216.
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

struct Planet {
    seed: u32,
    pal: &'static Palette,
    sea: f32,
    freq: f32,
    warp: f32,
    moist: f32,
    warm: f32,
    cap: f32,
    cover: f32,
    tilt: f32,
    off: [V3; 3],
    /// A site: its direction, and how far the base terrain lies below the land there.
    sites: Vec<(V3, f32)>,
}

impl Planet {
    fn new(seed: u32, centres: &[(f32, f32)]) -> Self {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B1) ^ 0xA5A5);
        let index = (seed as usize + seed as usize / 7) % PALETTES.len();
        let pal = &PALETTES[index];
        let mut o = || [r.range(-50., 50.), r.range(-50., 50.), r.range(-50., 50.)];
        let off = [o(), o(), o()];
        let mut p = Planet {
            seed: hash(seed as i32, 1, 2, 3),
            pal,
            sea: pal.sea + r.range(-0.05, 0.05),
            freq: r.range(0.85, 1.5),
            warp: r.range(0.25, 0.55),
            moist: pal.moist + r.range(-0.12, 0.12),
            warm: pal.warm + r.range(-0.1, 0.1),
            cap: (pal.cap + r.range(-0.04, 0.04)).min(0.99),
            cover: (pal.cloud + r.range(-0.1, 0.1)).clamp(0.12, 0.5),
            tilt: r.range(0.3, 0.55),
            off,
            sites: Vec::new(),
        };
        let dirs: Vec<V3> = centres.iter().map(|&(cx, cy)| p.site_dir(cx, cy)).collect();
        let sea = p.sea;
        p.sites = dirs
            .into_iter()
            .map(|d| (d, (sea + 0.16 - p.base(d, 10)).max(0.)))
            .collect();
        p
    }

    /// The world direction of a point of the disc: x right, y down, z to the viewer.
    fn world(&self, v: V3) -> V3 {
        let (s, c) = self.tilt.sin_cos();
        unit([v[0], c * v[1] - s * v[2], s * v[1] + c * v[2]])
    }

    /// The world direction under a point of the globe space, from the middle of the disc.
    fn site_dir(&self, cx: f32, cy: f32) -> V3 {
        self.world(sphere_point(cx / DISC_R, cy / DISC_R))
    }

    /// The height without the land under the sites, about -0.6 to 0.9.
    fn base(&self, d: V3, oct: u32) -> f32 {
        let p = scale(d, self.freq);
        let w = |k: usize| fbm(add(scale(p, 0.9), self.off[k], 1.), self.seed, 2);
        let q = add(p, [w(0), w(1), w(2)], self.warp);
        let cont = fbm(add(q, self.off[0], 1.), self.seed ^ 1, oct) * 1.5;
        let crest = ridged(
            add(scale(q, 1.9), self.off[1], 1.),
            self.seed ^ 2,
            oct.saturating_sub(1),
        );
        let land = smooth(self.sea - 0.04, self.sea + 0.3, cont);
        cont + 0.5 * land * (crest - 0.3)
    }

    /// The height with the land under the sites.
    fn height(&self, d: V3, oct: u32) -> f32 {
        let mut h = self.base(d, oct);
        for &(sd, need) in &self.sites {
            let dist2 = (1. - dot(d, sd)).max(0.) * 2.;
            h += need * (-dist2 / 0.14).exp();
        }
        h
    }

    /// The climate: wetness and warmth from 0 to 1.
    fn climate(&self, d: V3, e: f32) -> (f32, f32) {
        let q = add(scale(d, 2.3), self.off[2], 1.);
        let wet = (0.5 + 0.9 * fbm(q, self.seed ^ 3, 3) + self.moist - 0.35 * e).clamp(0., 1.);
        let warm = (1.2 + self.warm - 1.1 * d[1].abs().powf(1.7) - 0.7 * e
            + 0.12 * fbm(scale(q, 1.7), self.seed ^ 4, 2))
        .clamp(0., 1.);
        (wet, warm)
    }
}

/// The point of the unit sphere that faces the viewer at a spot of the disc.
fn sphere_point(x: f32, y: f32) -> V3 {
    let r2 = x * x + y * y;
    if r2 < 0.998 {
        [x, y, (1. - r2).sqrt()]
    } else {
        let k = 0.999 / r2.sqrt();
        [x * k, y * k, (1. - 0.998_f32).sqrt()]
    }
}

// ---- the render ----------------------------------------------------------------------------

/// What a pixel looks at.
struct Look {
    dir: V3,
    /// The normal of the unperturbed surface.
    n: V3,
    cover: f32,
}

fn rows<T: Send>(buf: &mut [T], w: usize, f: &(impl Fn(usize, &mut [T]) + Sync)) {
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(8));
    let per = (buf.len() / w).div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (ci, chunk) in buf.chunks_mut(per * w).enumerate() {
            s.spawn(move || {
                for (r, row) in chunk.chunks_mut(w).enumerate() {
                    f(ci * per + r, row);
                }
            });
        }
    });
}

fn tone(c: f32) -> u8 {
    (c.clamp(0., 1.) * 255.) as u8
}

/// Paint a picture of the terrain. `px_rad` is the number of pixels for one radian.
fn render(
    pl: &Planet,
    (w, h): (u32, u32),
    oct: u32,
    px_rad: f32,
    globe: bool,
    look: &(impl Fn(u32, u32) -> Look + Sync),
) -> RgbaImage {
    let (wu, hu) = (w as usize, h as usize);
    let mut height = vec![0f32; wu * hu];
    rows(&mut height, wu, &|y, row: &mut [f32]| {
        for (x, v) in row.iter_mut().enumerate() {
            let l = look(x as u32, y as u32);
            *v = if l.cover > 0. {
                pl.height(l.dir, oct)
            } else {
                0.
            };
        }
    });
    let pal = pl.pal;
    let light = unit([-0.5, -0.55, 0.7]);
    let at = |x: i64, y: i64| {
        let (x, y) = (
            x.clamp(0, w as i64 - 1) as usize,
            y.clamp(0, h as i64 - 1) as usize,
        );
        height[y * wu + x].max(pl.sea)
    };
    // The slope of the relief: the crop is a flat map, so it gets more of it.
    let relief = if globe { 0.55 } else { 0.9 };
    let grain = if globe { 36. } else { 380. };
    let mut px = vec![[0u8; 4]; wu * hu];
    rows(&mut px, wu, &|y, row: &mut [[u8; 4]]| {
        for (x, out) in row.iter_mut().enumerate() {
            let l = look(x as u32, y as u32);
            if l.cover <= 0. {
                *out = [0, 0, 0, 255];
                continue;
            }
            let (xi, yi) = (x as i64, y as i64);
            let hh = height[y * wu + x];
            let e = hh - pl.sea;
            let gx = (at(xi + 1, yi) - at(xi - 1, yi)) * 0.5 * px_rad * relief * 0.05;
            let gy = (at(xi, yi + 1) - at(xi, yi - 1)) * 0.5 * px_rad * relief * 0.05;
            let n = unit([l.n[0] - gx, l.n[1] - gy, l.n[2]]);
            let slope = (gx * gx + gy * gy).sqrt();
            let d = l.dir;
            let (wet, warm) = pl.climate(d, e.max(0.));
            let tex = fbm(scale(d, grain), pl.seed ^ 5, 3);
            let cap_edge = d[1].abs() + 0.07 * fbm(scale(d, 5.), pl.seed ^ 6, 4);
            let ice = smooth(pl.cap - 0.02, pl.cap + 0.03, cap_edge);
            let lit;
            let mut col;
            if e <= 0. {
                let depth = (-e * 3.4).min(1.);
                col = mix(pal.shallow, pal.deep, depth.powf(0.7));
                col = mix(
                    col,
                    scale(pal.shallow, 1.25),
                    (1. - smooth(0., 0.05, -e)) * 0.8,
                );
                col = scale(col, 1. + 0.05 * tex);
                // The sea is smooth: only the sphere lights it, with a soft glint.
                let nl = dot(l.n, light).max(0.);
                let half = unit(add(light, [0., 0., 1.], 1.));
                let glint = dot(l.n, half).max(0.).powi(40) * 0.22;
                lit = 0.5 + 0.62 * nl;
                col = scale(col, lit);
                col = add(col, [1., 1., 1.], glint);
                col = mix(col, scale(pal.ice_sea, 0.5 + 0.55 * nl), ice * 0.9);
            } else {
                let lowland = mix(pal.dry, pal.green, smooth(0.2, 0.6, wet));
                let lowland = mix(lowland, pal.forest, smooth(0.6, 0.95, wet) * 0.8);
                col = mix(pal.sand, lowland, smooth(0.0, 0.035, e));
                let rocky = smooth(0.3, 0.6, e + slope * 1.2);
                col = mix(col, pal.rock, rocky);
                let snow = smooth(0.3, 0.14, warm + 0.08 * tex) * smooth(0.08, 0.3, e);
                col = mix(col, pal.snow, snow.max(ice));
                col = scale(col, 1. + 0.16 * tex);
                let nl = dot(n, light).max(0.);
                lit = 0.42 + 0.68 * nl;
                col = scale(col, lit);
            }
            if globe {
                // A thin haze at the limb.
                let r2 = 1. - l.n[2] * l.n[2];
                col = mix(col, [0.45, 0.62, 1.], r2 * r2 * 0.28);
            }
            let k = if globe { l.cover } else { 1. };
            let t = scale(col, k);
            *out = [tone(t[0]), tone(t[1]), tone(t[2]), 255];
        }
    });
    RgbaImage::from_fn(w, h, |x, y| Rgba(px[y as usize * wu + x as usize]))
}

/// The globe: a lit sphere of 1400 by 1400 pixels, black outside the disc.
pub(super) fn planet(seed: u32, centres: &[(f32, f32)]) -> RgbaImage {
    globe_sized(seed, centres, GLOBE_PX)
}

fn globe_sized(seed: u32, centres: &[(f32, f32)], side: u32) -> RgbaImage {
    let pl = Planet::new(seed, centres);
    let half = side as f32 / 2.;
    let look = |x: u32, y: u32| {
        let (u, v) = (
            (x as f32 + 0.5 - half) / half,
            (y as f32 + 0.5 - half) / half,
        );
        let r = u.hypot(v);
        let cover = ((1. - r) * half).clamp(0., 1.);
        let n = if r < 1. {
            sphere_point(u, v)
        } else {
            [0., 0., 1.]
        };
        Look {
            dir: pl.world(n),
            n,
            cover,
        }
    };
    let oct = if side >= 1000 { 7 } else { 5 };
    render(&pl, (side, side), oct, half * 0.9, true, &look)
}

/// The clouds: an alpha strip of 2:1 that wraps at the sides, since it is cut from the sphere.
pub(super) fn clouds(seed: u32) -> RgbaImage {
    clouds_sized(seed, CLOUD_W)
}

fn clouds_sized(seed: u32, w: u32) -> RgbaImage {
    let pl = Planet::new(seed, &[]);
    let h = w / 2;
    let (wu, hu) = (w as usize, h as usize);
    let cover = pl.cover;
    let mut px = vec![[0u8; 4]; wu * hu];
    rows(&mut px, wu, &|y, row: &mut [[u8; 4]]| {
        let lat = (0.5 - (y as f32 + 0.5) / hu as f32) * std::f32::consts::PI;
        for (x, out) in row.iter_mut().enumerate() {
            let lon = (x as f32 + 0.5) / wu as f32 * std::f32::consts::TAU;
            let d = [lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin()];
            // Bands along the parallels, bent by a warp.
            let p = [d[0] * 1.5, d[1] * 3.6, d[2] * 1.5];
            let p = add(p, pl.off[1], 1.);
            let wv = [
                fbm(scale(p, 0.7), pl.seed ^ 11, 2),
                fbm(add(scale(p, 0.7), pl.off[0], 1.), pl.seed ^ 12, 2),
                fbm(add(scale(p, 0.7), pl.off[2], 1.), pl.seed ^ 13, 2),
            ];
            let q = add(p, wv, 0.7);
            let f = 0.5 + 0.62 * fbm(q, pl.seed ^ 14, 6);
            let wisp = 0.78 + 0.22 * fbm(scale(q, 5.), pl.seed ^ 15, 3);
            let a = smooth(1. - cover, 1. - cover + 0.24, f) * wisp;
            let shade = 0.93 + 0.07 * fbm(scale(q, 3.), pl.seed ^ 16, 2);
            let v = tone(shade);
            *out = [v, v, v.saturating_add(4), (a.clamp(0., 1.) * 255.) as u8];
        }
    });
    RgbaImage::from_fn(w, h, |x, y| Rgba(px[y as usize * wu + x as usize]))
}

/// The crop of a site: the terrain around its centre, at ten times the detail of the globe.
pub(super) fn site_crop(seed: u32, site: usize, centres: &[(f32, f32)]) -> RgbaImage {
    crop_sized(seed, site, centres, CROP_W)
}

fn crop_sized(seed: u32, site: usize, centres: &[(f32, f32)], w: u32) -> RgbaImage {
    let pl = Planet::new(seed, centres);
    let (cx, cy) = centres.get(site).copied().unwrap_or((0., 0.));
    let h = (w as f32 * CROP_H as f32 / CROP_W as f32) as u32;
    let k = w as f32 / CROP_W as f32;
    let look = |x: u32, y: u32| {
        let (u, v) = (
            (x as f32 + 0.5) / (CROP_SCALE * k),
            (y as f32 + 0.5) / (CROP_SCALE * k),
        );
        let (du, dv) = (
            (u - SPACE_W / 2.) / CROP_ZOOM,
            (v - SPACE_H / 2.) / CROP_ZOOM,
        );
        let n = sphere_point((cx + du) / DISC_R, (cy + dv) / DISC_R);
        Look {
            dir: pl.world(n),
            n: [0., 0., 1.],
            cover: 1.,
        }
    };
    let oct = if w >= 1000 { 10 } else { 7 };
    // One pixel of the crop covers 1 / (12 * 1.5) globe units, and 390 units are one radian.
    let px_rad = DISC_R * CROP_ZOOM * CROP_SCALE * k;
    render(&pl, (w, h), oct, px_rad, false, &look)
}

// ---- tests ---------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn centres() -> Vec<(f32, f32)> {
        let raw: serde_json::Value =
            serde_json::from_str(include_str!("../../../../assets/bridge/sites.json")).unwrap();
        raw.as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["cx"].as_f64().unwrap() as f32,
                    s["cy"].as_f64().unwrap() as f32,
                )
            })
            .collect()
    }

    fn sum(img: &RgbaImage) -> u64 {
        img.as_raw().iter().enumerate().fold(0u64, |h, (i, &b)| {
            h.wrapping_mul(31).wrapping_add(u64::from(b) + i as u64)
        })
    }

    #[test]
    fn same_seed_same_picture_other_seed_other_picture() {
        let c = centres();
        let a = globe_sized(20_000, &c, 160);
        assert_eq!(sum(&a), sum(&globe_sized(20_000, &c, 160)));
        assert_ne!(sum(&a), sum(&globe_sized(20_001, &c, 160)));
        let k = clouds_sized(20_000, 200);
        assert_eq!(sum(&k), sum(&clouds_sized(20_000, 200)));
        assert_ne!(sum(&k), sum(&clouds_sized(20_001, 200)));
    }

    #[test]
    fn sites_lie_on_land() {
        let c = centres();
        assert_eq!(c.len(), 8);
        for seed in 20_000..20_040 {
            let pl = Planet::new(seed, &c);
            for &(cx, cy) in &c {
                let d = pl.site_dir(cx, cy);
                assert!(
                    pl.height(d, 10) > pl.sea + 0.1,
                    "seed {seed} site {cx} {cy}"
                );
            }
        }
    }

    #[test]
    fn sizes_match_the_old_pictures() {
        let c = centres();
        assert_eq!(crop_sized(5, 0, &c, 233).dimensions(), (233, 154));
        assert_eq!(clouds_sized(5, 220).dimensions(), (220, 110));
    }

    #[test]
    #[ignore = "writes pictures into PLANET_OUT"]
    fn write_pictures() {
        let out = std::path::PathBuf::from(std::env::var("PLANET_OUT").unwrap());
        std::fs::create_dir_all(&out).unwrap();
        let c = centres();
        let t = std::time::Instant::now();
        let day = today_seed();
        for seed in [
            20_400u32,
            20_401,
            20_402,
            20_403,
            20_404,
            20_405,
            day,
            day + 1,
            day + 2,
        ] {
            let t0 = std::time::Instant::now();
            planet(seed, &c)
                .save(out.join(format!("globe{seed}.png")))
                .unwrap();
            let a = t0.elapsed();
            clouds(seed)
                .save(out.join(format!("clouds{seed}.png")))
                .unwrap();
            let b = t0.elapsed();
            site_crop(seed, (seed % 8) as usize, &c)
                .save(out.join(format!("crop{seed}.png")))
                .unwrap();
            println!(
                "seed {seed}: globe {a:?}, +clouds {b:?}, +crop {:?}",
                t0.elapsed()
            );
        }
        println!("all {:?}", t.elapsed());
    }
}
