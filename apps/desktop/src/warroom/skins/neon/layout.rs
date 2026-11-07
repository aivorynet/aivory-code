//! The map of the Neon: one district per codebase, one building per part. Pure
//! geometry in world units (x right, y away from the viewer, z up), centred on the origin.

/// Width and depth of a building.
pub(super) const BUILDING: f32 = 70.;
/// Distance between two building origins.
const PITCH: f32 = 150.;
/// Buildings per row of a district.
const PER_ROW: usize = 3;
/// Free ground between two districts (the road runs in the middle).
const GAP: f32 = 90.;
/// The widest shelf of districts.
const SHELF: f32 = 1500.;
/// Distance of the ring road from the outer districts.
const RING: f32 = 45.;

/// One part of a codebase.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Building {
    pub district: usize,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub h: f32,
}

/// One codebase with live sessions.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct District {
    pub id: String,
    pub color: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub d: f32,
}

/// The city: districts, buildings and road segments `[x0, y0, x1, y1]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct City {
    pub districts: Vec<District>,
    pub buildings: Vec<Building>,
    pub roads: Vec<[f32; 4]>,
    /// Half extent of the ground (x, y), roads included.
    pub half: (f32, f32),
}

/// The district colors. Magenta and red stay free: they mark a beacon and an error.
const COLORS: [u32; 6] = [0x00F0FF, 0xFCEE0A, 0xA97CFF, 0x3CFFB4, 0xFF9F1C, 0x4D8DFF];

/// A stable height from the name: 60 to 149.
fn height(name: &str) -> f32 {
    60. + (name.chars().map(|c| c as u32).sum::<u32>() * 37 % 90) as f32
}

impl City {
    /// The building of a part.
    pub fn building(&self, codebase: &str, part: &str) -> Option<usize> {
        let d = self.districts.iter().position(|d| d.id == codebase)?;
        self.buildings
            .iter()
            .position(|b| b.district == d && b.name == part)
    }
}

/// Lay out `(codebase, parts)` in order. Districts fill shelves from the front to the back.
pub(super) fn layout(input: &[(String, Vec<String>)]) -> City {
    let mut city = City::default();
    let mut shelves: Vec<Vec<usize>> = vec![Vec::new()];
    let (mut x, mut y, mut deepest) = (0f32, 0f32, 0f32);
    for (i, (id, parts)) in input.iter().enumerate() {
        let n = parts.len().max(1);
        let cols = n.min(PER_ROW);
        let rows = n.div_ceil(PER_ROW);
        let w = 40. + (cols - 1) as f32 * PITCH + BUILDING + 60.;
        let d = 50. + (rows - 1) as f32 * PITCH + BUILDING + 60.;
        if x > 0. && x + w > SHELF {
            y += deepest + GAP;
            x = 0.;
            deepest = 0.;
            shelves.push(Vec::new());
        }
        let color = COLORS[i % COLORS.len()];
        city.districts.push(District {
            id: id.clone(),
            color,
            x,
            y,
            w,
            d,
        });
        shelves.last_mut().expect("a shelf").push(i);
        for (k, part) in parts.iter().enumerate() {
            city.buildings.push(Building {
                district: i,
                name: part.clone(),
                x: x + 40. + (k % PER_ROW) as f32 * PITCH,
                y: y + 50. + (k / PER_ROW) as f32 * PITCH,
                h: height(part),
            });
        }
        x += w + GAP;
        deepest = deepest.max(d);
    }
    // Centre the city on the origin.
    let max_x = city.districts.iter().map(|d| d.x + d.w).fold(0., f32::max);
    let max_y = city.districts.iter().map(|d| d.y + d.d).fold(0., f32::max);
    let (ox, oy) = (max_x / 2., max_y / 2.);
    for d in &mut city.districts {
        d.x -= ox;
        d.y -= oy;
    }
    for b in &mut city.buildings {
        b.x -= ox;
        b.y -= oy;
    }
    // The ring road, the roads between shelves and between districts of one shelf.
    let (x0, x1, y0, y1) = (-ox - RING, ox + RING, -oy - RING, oy + RING);
    city.roads.extend([
        [x0, y0, x1, y0],
        [x1, y0, x1, y1],
        [x1, y1, x0, y1],
        [x0, y1, x0, y0],
    ]);
    for pair in shelves.windows(2) {
        let edge = pair[0]
            .iter()
            .map(|&i| city.districts[i].y + city.districts[i].d)
            .fold(f32::MIN, f32::max);
        let at = edge + GAP / 2.;
        city.roads.push([x0, at, x1, at]);
    }
    for shelf in &shelves {
        let low = shelf
            .iter()
            .map(|&i| city.districts[i].y)
            .fold(f32::MAX, f32::min);
        let high = shelf
            .iter()
            .map(|&i| city.districts[i].y + city.districts[i].d)
            .fold(f32::MIN, f32::max);
        for pair in shelf.windows(2) {
            let at = city.districts[pair[0]].x + city.districts[pair[0]].w + GAP / 2.;
            city.roads.push([at, low - RING, at, high + RING]);
        }
    }
    city.half = (ox + RING, oy + RING);
    city
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(counts: &[usize]) -> Vec<(String, Vec<String>)> {
        counts
            .iter()
            .enumerate()
            .map(|(i, n)| {
                (
                    format!("cb{i}"),
                    (0..*n).map(|k| format!("part{k}")).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn buildings_stand_inside_their_district() {
        let city = layout(&input(&[6, 4, 2, 7, 3]));
        assert_eq!(city.buildings.len(), 22);
        for b in &city.buildings {
            let d = &city.districts[b.district];
            assert!(b.x >= d.x && b.x + BUILDING <= d.x + d.w);
            assert!(b.y >= d.y && b.y + BUILDING <= d.y + d.d);
        }
    }

    #[test]
    fn districts_do_not_overlap() {
        let city = layout(&input(&[6, 6, 6, 3, 2, 4, 5]));
        for (i, a) in city.districts.iter().enumerate() {
            for b in &city.districts[i + 1..] {
                let apart =
                    a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.d <= b.y || b.y + b.d <= a.y;
                assert!(apart, "{} and {} overlap", a.id, b.id);
            }
        }
    }

    #[test]
    fn the_city_is_centred_and_has_a_ring_road() {
        let city = layout(&input(&[3, 3]));
        let min_x = city.districts.iter().map(|d| d.x).fold(f32::MAX, f32::min);
        let max_x = city
            .districts
            .iter()
            .map(|d| d.x + d.w)
            .fold(f32::MIN, f32::max);
        assert!((min_x + max_x).abs() < 0.01);
        assert!(city.roads.len() >= 5);
    }

    #[test]
    fn a_part_finds_its_building() {
        let city = layout(&input(&[2, 2]));
        assert_eq!(city.building("cb1", "part1"), Some(3));
        assert_eq!(city.building("cb1", "none"), None);
    }
}
