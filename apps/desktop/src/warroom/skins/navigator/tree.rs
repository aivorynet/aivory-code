//! The landscape of the Navigator: a root pedestal (the fleet), one pedestal per codebase in
//! rows behind it, one block per part on each pedestal, and the lines on the ground that join
//! them as a tree. Pure geometry in world units (x right, y away from the viewer, z up), plus
//! the routes that the agent cubes glide along.

/// Width and depth of a block.
pub(super) const BLOCK: f32 = 46.;
/// Distance between two block origins on a pedestal.
const PITCH: f32 = 76.;
/// Blocks per row of a pedestal.
const PER_ROW: usize = 3;
/// The free border of a pedestal around its blocks.
const MARGIN: f32 = 26.;
/// The height of a pedestal slab.
pub(super) const SLAB: f32 = 16.;
/// The side of the root pedestal.
const ROOT: f32 = 120.;
/// Pedestals per row of the tree: half left of the trunk, half right.
const PER_TREE_ROW: usize = 8;
/// The free ground between two pedestals of one row.
const GAP: f32 = 70.;
/// The width of the aisle that the trunk runs in.
const AISLE: f32 = 90.;
/// The free ground between two rows (the bus line runs in it).
const ROW_GAP: f32 = 200.;
/// The distance of a bus line in front of its row.
const BUS: f32 = 95.;
/// The lane on top of a pedestal, in front of the first block row.
const LANE: f32 = 13.;

/// One pedestal: the root or a codebase.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Pedestal {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub d: f32,
    /// The row of the tree; the root has none.
    pub row: Option<usize>,
}

impl Pedestal {
    /// The middle of the front edge.
    pub fn front(&self) -> [f32; 2] {
        [self.x + self.w / 2., self.y]
    }

    /// The middle of the top.
    pub fn centre(&self) -> [f32; 3] {
        [self.x + self.w / 2., self.y + self.d / 2., SLAB]
    }
}

/// One part of a codebase.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Block {
    /// The index of the pedestal (the root is 0).
    pub pedestal: usize,
    pub name: String,
    pub x: f32,
    pub y: f32,
}

impl Block {
    /// The middle of the foot, on the pedestal top.
    pub fn foot(&self) -> [f32; 3] {
        [self.x + BLOCK / 2., self.y + BLOCK / 2., SLAB]
    }
}

/// The landscape. Pedestal 0 is the root.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Tree {
    pub pedestals: Vec<Pedestal>,
    pub blocks: Vec<Block>,
    /// Ground lines `[x0, y0, x1, y1]` of the tree.
    pub lines: Vec<[f32; 4]>,
    /// The bus height `y` of each row.
    buses: Vec<f32>,
    /// The bounds of the ground in use: min x, min y, max x, max y.
    pub extent: [f32; 4],
}

impl Tree {
    /// The pedestal of a codebase.
    pub fn pedestal(&self, codebase: &str) -> Option<usize> {
        self.pedestals
            .iter()
            .skip(1)
            .position(|p| p.id == codebase)
            .map(|i| i + 1)
    }

    /// The block of a part.
    pub fn block(&self, codebase: &str, part: &str) -> Option<usize> {
        let p = self.pedestal(codebase)?;
        self.blocks
            .iter()
            .position(|b| b.pedestal == p && b.name == part)
    }

    /// The route from the root to the side of block `b`, `slot` places along it, as points
    /// on the ground and on the pedestal tops.
    pub fn route(&self, b: usize, slot: usize) -> Vec<[f32; 3]> {
        let root = &self.pedestals[0];
        let block = &self.blocks[b];
        let ped = &self.pedestals[block.pedestal];
        let row = ped.row.unwrap_or(0);
        let bus = self.buses.get(row).copied().unwrap_or(ped.y - BUS);
        let [fx, fy] = ped.front();
        let trunk = root.x + root.w / 2.;
        let lane = ped.y + LANE;
        // The cube rests in the aisle at the left of the block, one cube length per slot.
        let side = block.x - (PITCH - BLOCK) / 2.;
        let rest = block.y + BLOCK - 8. - slot as f32 * 14.;
        vec![
            [trunk, root.y + root.d / 2., SLAB],
            [trunk, root.y + root.d, SLAB],
            [trunk, root.y + root.d, 0.],
            [trunk, bus, 0.],
            [fx, bus, 0.],
            [fx, fy, 0.],
            [fx, fy, SLAB],
            [fx, lane, SLAB],
            [side, lane, SLAB],
            [side, rest.max(lane), SLAB],
        ]
    }
}

/// Lay out `(codebase, parts)` in order: the root at the front, rows of pedestals behind it,
/// the first half of a row left of the trunk and the rest right of it.
pub(super) fn layout(input: &[(String, Vec<String>)]) -> Tree {
    let mut tree = Tree::default();
    tree.pedestals.push(Pedestal {
        id: "fleet".to_string(),
        x: -ROOT / 2.,
        y: 0.,
        w: ROOT,
        d: ROOT,
        row: None,
    });
    let size = |n: usize| {
        let n = n.max(1);
        let cols = n.min(PER_ROW) as f32;
        let rows = n.div_ceil(PER_ROW) as f32;
        (
            2. * MARGIN + (cols - 1.) * PITCH + BLOCK,
            2. * MARGIN + LANE + (rows - 1.) * PITCH + BLOCK,
        )
    };
    let mut y = ROOT + ROW_GAP;
    for (r, chunk) in input.chunks(PER_TREE_ROW).enumerate() {
        let left = chunk.len().div_ceil(2);
        let mut deepest = 0f32;
        // The left half grows from the aisle to the left, the right half to the right.
        let mut xl = -AISLE / 2.;
        let mut xr = AISLE / 2.;
        let mut placed = Vec::new();
        for (k, (id, parts)) in chunk.iter().enumerate() {
            let (w, d) = size(parts.len());
            let x = if k < left {
                // The nearest to the aisle is the last of the left half.
                0.
            } else {
                let x = xr;
                xr += w + GAP;
                x
            };
            placed.push((id.clone(), parts.clone(), x, w, d));
            deepest = deepest.max(d);
        }
        for item in placed.iter_mut().take(left).rev() {
            xl -= item.3;
            item.2 = xl;
            xl -= GAP;
        }
        tree.buses.push(y - BUS);
        for (id, parts, x, w, d) in placed {
            let p = tree.pedestals.len();
            tree.pedestals.push(Pedestal {
                id,
                x,
                y,
                w,
                d,
                row: Some(r),
            });
            for (i, name) in parts.iter().enumerate() {
                tree.blocks.push(Block {
                    pedestal: p,
                    name: name.clone(),
                    x: x + MARGIN + (i % PER_ROW) as f32 * PITCH,
                    y: y + MARGIN + LANE + (i / PER_ROW) as f32 * PITCH,
                });
            }
        }
        y += deepest + ROW_GAP;
    }

    // The lines: the trunk from the root to the last bus, one bus per row, one stub per pedestal.
    let root = tree.pedestals[0].clone();
    let trunk = root.x + root.w / 2.;
    if let Some(last) = tree.buses.last() {
        tree.lines.push([trunk, root.y + root.d, trunk, *last]);
    }
    for (r, bus) in tree.buses.iter().enumerate() {
        let stubs: Vec<[f32; 2]> = tree
            .pedestals
            .iter()
            .filter(|p| p.row == Some(r))
            .map(|p| p.front())
            .collect();
        let lo = stubs.iter().map(|s| s[0]).fold(trunk, f32::min);
        let hi = stubs.iter().map(|s| s[0]).fold(trunk, f32::max);
        tree.lines.push([lo, *bus, hi, *bus]);
        for [x, y] in stubs {
            tree.lines.push([x, *bus, x, y]);
        }
    }
    let mut e = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in &tree.pedestals {
        e = [
            e[0].min(p.x),
            e[1].min(p.y),
            e[2].max(p.x + p.w),
            e[3].max(p.y + p.d),
        ];
    }
    tree.extent = e;
    tree
}

/// The length of a polyline.
pub(super) fn length(path: &[[f32; 3]]) -> f32 {
    path.windows(2).map(|w| dist(w[0], w[1])).sum()
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The point at distance `s` along a polyline.
pub(super) fn point_at(path: &[[f32; 3]], s: f32) -> [f32; 3] {
    let mut left = s.max(0.);
    for w in path.windows(2) {
        let l = dist(w[0], w[1]);
        if left <= l && l > 0. {
            let k = left / l;
            return [
                w[0][0] + (w[1][0] - w[0][0]) * k,
                w[0][1] + (w[1][1] - w[0][1]) * k,
                w[0][2] + (w[1][2] - w[0][2]) * k,
            ];
        }
        left -= l;
    }
    path.last().copied().unwrap_or([0.; 3])
}

/// The way from the point at `s` on `old` to the end of `new`: back along `old` to the last
/// point that the two routes share, then out along `new`.
pub(super) fn splice(old: &[[f32; 3]], s: f32, new: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let shared = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    // The segment that the cube is on: old[seg] .. old[seg + 1].
    let mut left = s.max(0.);
    let mut seg = old.len().saturating_sub(1);
    for (i, w) in old.windows(2).enumerate() {
        let l = dist(w[0], w[1]);
        if left <= l {
            seg = i;
            break;
        }
        left -= l;
    }
    let mut out = vec![point_at(old, s)];
    if shared == 0 {
        out.extend_from_slice(new);
        return out;
    }
    if seg + 1 >= shared {
        // Back to the branch point.
        for i in (shared - 1..=seg).rev() {
            out.push(old[i]);
        }
        out.extend_from_slice(&new[shared..]);
    } else {
        // Still on the shared part: straight on along the new route.
        out.extend_from_slice(&new[seg + 1..]);
    }
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(counts: &[usize]) -> Vec<(String, Vec<String>)> {
        counts
            .iter()
            .enumerate()
            .map(|(i, n)| (format!("cb{i}"), (0..*n).map(|k| format!("p{k}")).collect()))
            .collect()
    }

    #[test]
    fn blocks_stand_on_their_pedestal() {
        let t = layout(&input(&[6, 4, 2, 7, 3, 1]));
        assert_eq!(t.blocks.len(), 23);
        assert_eq!(t.pedestals.len(), 7);
        for b in &t.blocks {
            let p = &t.pedestals[b.pedestal];
            assert!(b.x >= p.x && b.x + BLOCK <= p.x + p.w);
            assert!(b.y >= p.y && b.y + BLOCK <= p.y + p.d);
        }
    }

    #[test]
    fn pedestals_do_not_overlap_and_keep_the_aisle_free() {
        let t = layout(&input(&[6, 6, 6, 3, 2, 4, 5, 1, 2, 3]));
        for (i, a) in t.pedestals.iter().enumerate().skip(1) {
            assert!(a.x + a.w <= -AISLE / 2. + 0.01 || a.x >= AISLE / 2. - 0.01);
            for b in &t.pedestals[i + 1..] {
                let apart =
                    a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.d <= b.y || b.y + b.d <= a.y;
                assert!(apart, "{} and {} overlap", a.id, b.id);
            }
        }
        // Two rows (8 per row): one bus each, a trunk, a stub per pedestal.
        assert_eq!(t.lines.len(), 1 + 2 + 10);
    }

    #[test]
    fn a_part_finds_its_block_and_its_route_ends_beside_it() {
        let t = layout(&input(&[2, 3]));
        let b = t.block("cb1", "p2").unwrap();
        assert_eq!(t.block("cb1", "none"), None);
        let r = t.route(b, 0);
        let end = r.last().unwrap();
        let blk = &t.blocks[b];
        assert!(end[0] < blk.x && end[0] > blk.x - (PITCH - BLOCK));
        assert_eq!(r[0], t.route(0, 0)[0]);
    }

    #[test]
    fn walking_a_route() {
        let path = [[0., 0., 0.], [10., 0., 0.], [10., 10., 0.]];
        assert_eq!(length(&path), 20.);
        assert_eq!(point_at(&path, 15.), [10., 5., 0.]);
        assert_eq!(point_at(&path, 99.), [10., 10., 0.]);
    }

    #[test]
    fn a_new_route_goes_back_to_the_branch_point() {
        let old = [[0., 0., 0.], [10., 0., 0.], [10., 10., 0.]];
        let new = [[0., 0., 0.], [10., 0., 0.], [20., 0., 0.]];
        // At the end of the old route: back to (10, 0), then on to (20, 0).
        let w = splice(&old, 20., &new);
        assert_eq!(w, vec![[10., 10., 0.], [10., 0., 0.], [20., 0., 0.]]);
        // Still on the first segment: straight on.
        let w = splice(&old, 5., &new);
        assert_eq!(w, vec![[5., 0., 0.], [10., 0., 0.], [20., 0., 0.]]);
    }
}
