//! Dense packing of the grid groups into rows, as CSS `grid-auto-flow: row dense` places
//! one-row items: each group goes into the first row with enough free columns.

/// One row of the outer grid: `(group index, first column)` pairs, left to right.
pub type PackedRow = Vec<(usize, u32)>;

pub fn pack(spans: &[u32], columns: u32) -> Vec<PackedRow> {
    let columns = columns.max(1);
    let mut rows: Vec<(u32, PackedRow)> = Vec::new();
    for (i, &span) in spans.iter().enumerate() {
        let span = span.clamp(1, columns);
        match rows.iter_mut().find(|(used, _)| columns - *used >= span) {
            Some((used, row)) => {
                row.push((i, *used));
                *used += span;
            }
            None => rows.push((span, vec![(i, 0)])),
        }
    }
    rows.into_iter().map(|(_, row)| row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_rows_stay_alone() {
        assert_eq!(pack(&[3, 3], 3), vec![vec![(0, 0)], vec![(1, 0)]]);
    }

    #[test]
    fn small_groups_fill_earlier_gaps() {
        // Needs you (3), a (2), b (3), c (1): c fills the gap next to a.
        assert_eq!(
            pack(&[3, 2, 3, 1], 3),
            vec![vec![(0, 0)], vec![(1, 0), (3, 2)], vec![(2, 0)]]
        );
    }

    #[test]
    fn spans_clamp_to_the_columns() {
        assert_eq!(pack(&[5, 0], 2), vec![vec![(0, 0)], vec![(1, 0)]]);
    }
}
