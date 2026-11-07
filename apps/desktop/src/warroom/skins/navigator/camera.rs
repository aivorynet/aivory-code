//! The camera of the Navigator: a pose (look-at point, turn, tilt, distance), the perspective
//! projection, the inverse to the ground plane, the eased flight between two poses, and small
//! 2D helpers for hit tests (convex hull, point in polygon).

/// Points nearer to the camera than this are behind the near plane.
pub(super) const NEAR: f32 = 24.;
/// The length of a camera flight, in seconds.
pub(super) const FLY: f32 = 0.8;

/// Where the camera looks from. World axes: x right, y away from the viewer, z up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Pose {
    /// The look-at point.
    pub target: [f32; 3],
    /// The turn around the vertical axis, in radians.
    pub yaw: f32,
    /// The tilt down from the horizon, in radians.
    pub pitch: f32,
    /// The distance from the look-at point.
    pub dist: f32,
}

impl Pose {
    /// The pose part way from `a` to `b`; the distance moves in log space, so a zoom feels even.
    pub fn lerp(a: &Pose, b: &Pose, t: f32) -> Pose {
        let l = |x: f32, y: f32| x + (y - x) * t;
        Pose {
            target: [
                l(a.target[0], b.target[0]),
                l(a.target[1], b.target[1]),
                l(a.target[2], b.target[2]),
            ],
            yaw: l(a.yaw, b.yaw),
            pitch: l(a.pitch, b.pitch),
            dist: l(a.dist.max(1.).ln(), b.dist.max(1.).ln()).exp(),
        }
    }
}

/// The smooth start and stop of a flight, `t` from 0 to 1.
pub(super) fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    if t < 0.5 {
        4. * t * t * t
    } else {
        1. - (-2. * t + 2.).powi(3) / 2.
    }
}

/// The projection of one frame: a pose, the focal length and the screen centre.
#[derive(Debug, Clone, Copy)]
pub(super) struct Cam {
    pub pose: Pose,
    cs: f32,
    sn: f32,
    cp: f32,
    sp: f32,
    /// The focal length in pixels.
    pub f: f32,
    /// The screen point of the look-at point.
    pub centre: [f32; 2],
}

impl Cam {
    pub fn new(pose: Pose, f: f32, centre: [f32; 2]) -> Self {
        Self {
            pose,
            cs: pose.yaw.cos(),
            sn: pose.yaw.sin(),
            cp: pose.pitch.cos(),
            sp: pose.pitch.sin(),
            f,
            centre,
        }
    }

    /// A world point in camera space: right, up, depth.
    pub fn eye(&self, p: [f32; 3]) -> [f32; 3] {
        let t = self.pose.target;
        let (x, y, z) = (p[0] - t[0], p[1] - t[1], p[2] - t[2]);
        let xr = x * self.cs - y * self.sn;
        let yr = x * self.sn + y * self.cs;
        let d = self.pose.dist;
        let vy = yr + d * self.cp;
        let vz = z - d * self.sp;
        [xr, vy * self.sp + vz * self.cp, vy * self.cp - vz * self.sp]
    }

    /// The distance of a world point from the camera along the view.
    pub fn depth(&self, p: [f32; 3]) -> f32 {
        self.eye(p)[2]
    }

    /// A camera-space point on the screen; the depth is held at the near plane.
    pub fn screen(&self, e: [f32; 3]) -> [f32; 2] {
        let z = e[2].max(NEAR);
        [
            self.centre[0] + e[0] * self.f / z,
            self.centre[1] - e[1] * self.f / z,
        ]
    }

    /// A world point on the screen.
    pub fn at(&self, p: [f32; 3]) -> [f32; 2] {
        self.screen(self.eye(p))
    }

    /// A world segment on the screen, cut at the near plane; `None` when it is all behind.
    pub fn segment(&self, a: [f32; 3], b: [f32; 3]) -> Option<[f32; 4]> {
        let (mut ea, mut eb) = (self.eye(a), self.eye(b));
        if ea[2] < NEAR && eb[2] < NEAR {
            return None;
        }
        let cut = |p: [f32; 3], q: [f32; 3]| {
            let k = (NEAR - p[2]) / (q[2] - p[2]);
            [p[0] + (q[0] - p[0]) * k, p[1] + (q[1] - p[1]) * k, NEAR]
        };
        if ea[2] < NEAR {
            ea = cut(ea, eb);
        } else if eb[2] < NEAR {
            eb = cut(eb, ea);
        }
        let (p, q) = (self.screen(ea), self.screen(eb));
        Some([p[0], p[1], q[0], q[1]])
    }

    /// True when every point is in front of the near plane.
    pub fn visible(&self, pts: &[[f32; 3]]) -> bool {
        pts.iter().all(|p| self.depth(*p) >= NEAR)
    }

    /// The screen height of the horizon (the ground far away).
    pub fn horizon(&self) -> f32 {
        self.centre[1] - self.f * self.sp / self.cp.max(1e-3)
    }

    /// The ground point under a screen point. A ray that does not reach the ground within
    /// `far` (it points at the sky) stops at `far`.
    #[cfg(test)]
    pub fn ground(&self, s: [f32; 2], far: f32) -> [f32; 2] {
        let d0 = (s[0] - self.centre[0]) / self.f;
        let d1 = -(s[1] - self.centre[1]) / self.f;
        // The ray in the turned world frame.
        let dir = [d0, d1 * self.sp + self.cp, d1 * self.cp - self.sp];
        let d = self.pose.dist;
        let t = self.pose.target;
        let eye = [0., -d * self.cp, t[2] + d * self.sp];
        let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt().max(1e-6);
        let k = if dir[2] < -1e-6 {
            (-eye[2] / dir[2]).min(far / len)
        } else {
            far / len
        };
        let (xr, yr) = (eye[0] + dir[0] * k, eye[1] + dir[1] * k);
        [
            t[0] + xr * self.cs + yr * self.sn,
            t[1] - xr * self.sn + yr * self.cs,
        ]
    }
}

/// The convex hull of points, counter-clockwise on the screen (monotone chain).
pub(super) fn hull(pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut p: Vec<[f32; 2]> = pts.to_vec();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(p.len() * 2);
    for pass in 0..2 {
        let start = out.len();
        let iter: Box<dyn Iterator<Item = &[f32; 2]>> = if pass == 0 {
            Box::new(p.iter())
        } else {
            Box::new(p.iter().rev())
        };
        for &q in iter {
            while out.len() >= start + 2 && cross(out[out.len() - 2], out[out.len() - 1], q) <= 0. {
                out.pop();
            }
            out.push(q);
        }
        out.pop();
    }
    out
}

/// True when `p` lies inside the polygon `poly` (even-odd rule).
pub(super) fn inside(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut odd = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            odd = !odd;
        }
    }
    odd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(yaw: f32, pitch: f32) -> Cam {
        let pose = Pose {
            target: [100., 300., 0.],
            yaw,
            pitch,
            dist: 1200.,
        };
        Cam::new(pose, 900., [700., 450.])
    }

    #[test]
    fn the_target_is_at_the_centre_and_depth_is_the_distance() {
        let c = cam(0.3, 0.6);
        let p = c.at([100., 300., 0.]);
        assert!((p[0] - 700.).abs() < 1e-3 && (p[1] - 450.).abs() < 1e-3);
        assert!((c.depth([100., 300., 0.]) - 1200.).abs() < 1e-2);
    }

    #[test]
    fn far_points_are_higher_and_up_is_up() {
        let c = cam(0., 0.5);
        let near = c.at([100., 0., 0.]);
        let far = c.at([100., 800., 0.]);
        assert!(far[1] < near[1]);
        assert!(c.at([100., 300., 100.])[1] < 450.);
        assert!(c.horizon() < far[1]);
    }

    #[test]
    fn the_ground_ray_inverts_the_projection() {
        let c = cam(-0.4, 0.7);
        for p in [[0., 0.], [400., 900.], [-300., 250.]] {
            let s = c.at([p[0], p[1], 0.]);
            let g = c.ground(s, 1e6);
            assert!(
                (g[0] - p[0]).abs() < 0.5 && (g[1] - p[1]).abs() < 0.5,
                "{g:?}"
            );
        }
        // A ray at the sky stops at the far distance.
        let sky = c.ground([700., c.horizon() - 50.], 5000.);
        assert!(sky[0].is_finite() && sky[1].is_finite());
    }

    #[test]
    fn a_segment_is_cut_at_the_near_plane() {
        let c = cam(0., 0.3);
        // From far in front of the camera to behind it.
        assert!(c.segment([100., 300., 0.], [100., -3000., 0.]).is_some());
        assert!(c.segment([100., -3000., 0.], [100., -3100., 0.]).is_none());
    }

    #[test]
    fn flights_ease_and_end_at_the_goal() {
        assert_eq!(ease_in_out(0.), 0.);
        assert_eq!(ease_in_out(1.), 1.);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
        assert!(ease_in_out(0.1) < 0.1);
        let a = cam(0., 0.5).pose;
        let mut b = a;
        b.dist = 300.;
        b.target = [0., 0., 50.];
        assert_eq!(Pose::lerp(&a, &b, 1.).target, b.target);
        assert!((Pose::lerp(&a, &b, 1.).dist - 300.).abs() < 0.01);
        let mid = Pose::lerp(&a, &b, 0.5).dist;
        assert!((mid - 600.).abs() < 1.);
    }

    #[test]
    fn the_hull_and_the_inside_test() {
        let h = hull(&[
            [0., 0.],
            [10., 0.],
            [10., 10.],
            [0., 10.],
            [5., 5.],
            [5., 0.],
        ]);
        assert_eq!(h.len(), 4);
        assert!(inside(&h, [5., 5.]));
        assert!(!inside(&h, [11., 5.]));
    }
}
