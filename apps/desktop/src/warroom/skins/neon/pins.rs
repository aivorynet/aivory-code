//! The map pins of the Neon: a small box on a leader line. A collision pass gives each
//! pin the nearest free spot; a pin with no free spot stays off the map. Pure screen geometry.

/// A screen rect: x, y, width, height.
pub(super) type Rect = [f32; 4];

/// The leader lengths that a pin tries, the shortest first.
const REACH: [f32; 5] = [16., 34., 56., 84., 116.];
/// The free space that a pin keeps around its box.
const PAD: f32 = 3.;

/// True when two rects overlap.
pub(super) fn overlap(a: &Rect, b: &Rect) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

/// True when `r` lies fully inside `area`.
pub(super) fn inside(r: &Rect, area: &Rect) -> bool {
    r[0] >= area[0]
        && r[1] >= area[1]
        && r[0] + r[2] <= area[0] + area[2]
        && r[1] + r[3] <= area[1] + area[3]
}

/// A pin with its spot: the box and the knee where the leader meets the box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Placed {
    pub rect: Rect,
    pub knee: [f32; 2],
}

/// Find a free spot for a box of `w` x `h` above `anchor`. The leader goes up at a slant to
/// the bottom corner of the box, to the right first. A found box joins `taken`.
pub(super) fn place(
    anchor: [f32; 2],
    (w, h): (f32, f32),
    taken: &mut Vec<Rect>,
    area: &Rect,
) -> Option<Placed> {
    for reach in REACH {
        for right in [true, false] {
            let dx = reach * 0.55;
            let knee = [anchor[0] + if right { dx } else { -dx }, anchor[1] - reach];
            let x = if right { knee[0] } else { knee[0] - w };
            let rect = [x, knee[1] - h, w, h];
            let room = [x - PAD, rect[1] - PAD, w + PAD * 2., h + PAD * 2.];
            if inside(&rect, area) && !taken.iter().any(|t| overlap(&room, t)) {
                taken.push(rect);
                return Some(Placed { rect, knee });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = [0., 0., 1000., 800.];

    #[test]
    fn the_first_pin_takes_the_short_leader_to_the_right() {
        let mut taken = Vec::new();
        let p = place([500., 400.], (80., 20.), &mut taken, &AREA).expect("a spot");
        assert!(p.rect[0] > 500.);
        assert!((p.knee[1] - 384.).abs() < 0.01);
        assert_eq!(taken.len(), 1);
    }

    #[test]
    fn pins_at_one_anchor_never_overlap() {
        let mut taken = Vec::new();
        let placed: Vec<Placed> = (0..6)
            .filter_map(|_| place([500., 400.], (90., 20.), &mut taken, &AREA))
            .collect();
        assert!(placed.len() >= 4);
        for (i, a) in placed.iter().enumerate() {
            for b in &placed[i + 1..] {
                assert!(!overlap(&a.rect, &b.rect));
            }
        }
    }

    #[test]
    fn a_pin_avoids_a_blocked_rect_and_the_edge() {
        let mut taken = vec![[480., 300., 200., 120.]];
        let p = place([500., 400.], (60., 18.), &mut taken, &AREA).expect("a spot");
        assert!(!overlap(&p.rect, &[480., 300., 200., 120.]));
        let mut none = Vec::new();
        assert!(place([2., 10.], (60., 18.), &mut none, &AREA).is_none());
    }
}
