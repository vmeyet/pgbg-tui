//! What grouped and stacked charts share: one point per row, in the result's order, and each point's bars as pieces on a
//! value scale, whichever way they are drawn.
use super::YColumn;

/// The most points a bar chart reads, a bound on its work: the drawing then keeps what its pane holds.
/// Past it the first rows are kept: sampling categories would drop arbitrary ones, and `ORDER BY ... LIMIT` says which matter.
pub(super) const MAX_POINTS: usize = 64;

pub(super) fn keep(rows: usize) -> Vec<usize> {
    (0..rows.min(MAX_POINTS)).collect()
}

/// One stretch of a bar in the colour of y column `column`, from `from` to `to` on the value scale; `to` is further from zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Piece {
    pub(crate) column: usize,
    pub(crate) from: f64,
    pub(crate) to: f64,
}

/// One bar: its pieces, and the number it says, if any.
#[derive(Debug, PartialEq)]
pub(crate) struct Lane {
    pub(crate) pieces: Vec<Piece>,
    pub(crate) value: Option<f64>,
}

/// The bars of a chart, ready to draw: the lanes of each point, and the scale they all share, zero always on it.
#[derive(Debug, PartialEq)]
pub(crate) struct Bars {
    pub(crate) points: Vec<Vec<Lane>>,
    pub(crate) low: f64,
    pub(crate) high: f64,
}

impl Bars {
    pub(super) fn new(points: Vec<Vec<Lane>>) -> Self {
        let ends = points.iter().flatten().flat_map(|lane| &lane.pieces).map(|piece| piece.to);
        let (low, high) = ends.fold((0.0, 0.0), |(low, high): (f64, f64), end| (low.min(end), high.max(end)));
        Self { points, low, high }
    }
}

/// The indexes of the points of `y`.
pub(super) fn points(y: &[YColumn]) -> std::ops::Range<usize> {
    0..y.first().map_or(0, |column| column.values.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_that_fit_are_all_kept() {
        assert_eq!(keep(3), [0, 1, 2]);
    }

    #[test]
    fn the_scale_always_holds_zero() {
        let lane = |to| Lane { pieces: vec![Piece { column: 0, from: 0.0, to }], value: Some(to) };
        let up = Bars::new(vec![vec![lane(4.0)]]);
        let down = Bars::new(vec![vec![lane(-4.0)]]);
        assert_eq!([(up.low, up.high), (down.low, down.high)], [(0.0, 4.0), (-4.0, 0.0)]);
    }
}
