//! Bars: one group per row, in the result's order.

/// The most bar groups a terminal shows side by side.
/// Past it the first rows are kept: sampling categories would drop arbitrary ones, and `ORDER BY ... LIMIT` says which matter.
pub(super) const MAX_POINTS: usize = 64;

pub(super) fn keep(rows: usize) -> Vec<usize> {
    (0..rows.min(MAX_POINTS)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_that_fit_are_all_kept() {
        assert_eq!(keep(3), [0, 1, 2]);
    }
}
