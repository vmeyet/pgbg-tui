//! A line: one point per row, in the result's order, drawn left to right.

/// The most points a line keeps: braille draws two dots per cell, so this covers a 512-column terminal.
/// Past it rows are sampled evenly, first and last kept; a spike between two samples can be missed.
pub(super) const MAX_POINTS: usize = 1024;

pub(super) fn keep(rows: usize) -> Vec<usize> {
    if rows <= MAX_POINTS {
        return (0..rows).collect();
    }
    (0..MAX_POINTS).map(|sample| sample * (rows - 1) / (MAX_POINTS - 1)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_past_the_cap_are_sampled_down_to_it() {
        let kept = keep(MAX_POINTS * 3 + 7);
        assert_eq!(kept.len(), MAX_POINTS);
        assert!(kept.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn a_sampled_line_keeps_its_first_and_last_rows() {
        let kept = keep(5000);
        assert_eq!((kept[0], kept[MAX_POINTS - 1]), (0, 4999));
    }
}
