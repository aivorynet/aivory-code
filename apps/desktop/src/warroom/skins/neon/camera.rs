//! The camera of the Neon city: it turns around the city (mouse drag, Q/E, the arrow keys),
//! tilts (vertical drag) and zooms (wheel, Up/Down, + and -). The target moves at once; the
//! shown value eases to it, so a key press is smooth. A drag sets both at once, so the city
//! follows the mouse one to one. Pure state, no drawing.

/// The default turn of the city, in radians.
pub(super) const YAW0: f32 = -0.62;
/// The default tilt of the camera, in radians (0 looks at the horizon, pi/2 straight down).
pub(super) const PITCH0: f32 = 0.95;
const PITCH_RANGE: (f32, f32) = (0.55, 1.25);
const ZOOM_RANGE: (f32, f32) = (0.55, 2.4);
/// The turn of one key press, in radians.
const KEY_TURN: f32 = 0.2;
/// The zoom of one key press.
const KEY_ZOOM: f32 = 1.15;
/// Radians of turn per pixel of drag, and of tilt per pixel.
const DRAG_TURN: f32 = 0.007;
const DRAG_TILT: f32 = 0.004;
/// How fast the shown value follows its target (per second).
const EASE: f32 = 9.;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Camera {
    yaw_to: f32,
    yaw: f32,
    pitch: f32,
    zoom_to: f32,
    zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw_to: YAW0,
            yaw: YAW0,
            pitch: PITCH0,
            zoom_to: 1.,
            zoom: 1.,
        }
    }
}

impl Camera {
    /// The start camera of a screenshot run: `AIVORY_CITY_YAW` (degrees from the default),
    /// `AIVORY_CITY_ZOOM` and `AIVORY_CITY_PITCH` (degrees).
    pub fn from_env() -> Self {
        let get = |k: &str| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.trim().parse::<f32>().ok())
        };
        let mut c = Self::default();
        if let Some(d) = get("AIVORY_CITY_YAW") {
            c.yaw_to = YAW0 + d.to_radians();
            c.yaw = c.yaw_to;
        }
        if let Some(z) = get("AIVORY_CITY_ZOOM") {
            c.zoom_to = z.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
            c.zoom = c.zoom_to;
        }
        if let Some(d) = get("AIVORY_CITY_PITCH") {
            c.pitch = d.to_radians().clamp(PITCH_RANGE.0, PITCH_RANGE.1);
        }
        c
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// A drag of `dx`, `dy` pixels: the city follows the mouse.
    pub fn drag(&mut self, dx: f32, dy: f32) {
        self.yaw_to += dx * DRAG_TURN;
        self.yaw = self.yaw_to;
        self.pitch = (self.pitch - dy * DRAG_TILT).clamp(PITCH_RANGE.0, PITCH_RANGE.1);
    }

    /// The wheel: a positive `dy` (the wheel moves up) zooms in.
    pub fn wheel(&mut self, dy: f32) {
        self.zoom_by((dy * 0.0016).exp());
    }

    fn zoom_by(&mut self, k: f32) {
        self.zoom_to = (self.zoom_to * k).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
    }

    /// A key of the city. `true`: the camera handles the key.
    pub fn key(&mut self, key: &str) -> bool {
        match key {
            "q" | "left" => self.yaw_to -= KEY_TURN,
            "e" | "right" => self.yaw_to += KEY_TURN,
            "up" | "+" | "=" => self.zoom_by(KEY_ZOOM),
            "down" | "-" | "_" => self.zoom_by(1. / KEY_ZOOM),
            "home" | "0" => {
                self.yaw_to = YAW0;
                self.pitch = PITCH0;
                self.zoom_to = 1.;
            }
            _ => return false,
        }
        true
    }

    /// Ease toward the target. `true`: the camera still moves.
    pub fn step(&mut self, dt: f32) -> bool {
        let k = 1. - (-dt * EASE).exp();
        self.yaw += (self.yaw_to - self.yaw) * k;
        self.zoom += (self.zoom_to - self.zoom) * k;
        (self.yaw_to - self.yaw).abs() > 1e-3 || (self.zoom_to - self.zoom).abs() > 1e-3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_turns_and_tilts_at_once() {
        let mut c = Camera::default();
        c.drag(100., 0.);
        assert!((c.yaw() - (YAW0 + 0.7)).abs() < 1e-5);
        c.drag(0., -1000.);
        assert!((c.pitch() - PITCH_RANGE.1).abs() < 1e-6);
        c.drag(0., 1000.);
        assert!((c.pitch() - PITCH_RANGE.0).abs() < 1e-6);
    }

    #[test]
    fn keys_turn_zoom_and_reset() {
        let mut c = Camera::default();
        assert!(c.key("e"));
        for _ in 0..200 {
            c.step(0.05);
        }
        assert!((c.yaw() - (YAW0 + KEY_TURN)).abs() < 1e-2);
        assert!(c.key("q") && c.key("left"));
        assert!(c.key("up") && c.zoom_to > 1.);
        assert!(!c.key("x"));
        assert!(c.key("home"));
        for _ in 0..200 {
            c.step(0.05);
        }
        assert!((c.yaw() - YAW0).abs() < 1e-2 && (c.zoom() - 1.).abs() < 1e-2);
    }

    #[test]
    fn zoom_stays_in_range() {
        let mut c = Camera::default();
        c.wheel(100_000.);
        assert_eq!(c.zoom_to, ZOOM_RANGE.1);
        c.wheel(-100_000.);
        assert_eq!(c.zoom_to, ZOOM_RANGE.0);
    }

    #[test]
    fn easing_is_smooth_and_ends() {
        let mut c = Camera::default();
        c.key("e");
        assert!(c.step(0.016));
        assert!(c.yaw() > YAW0 && c.yaw() < YAW0 + KEY_TURN);
    }
}
