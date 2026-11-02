//! Grouped bars: each point has one bar per y column, side by side, from zero to its value.
use super::YColumn;
use super::bars::{self, Bars, Lane, Piece};

/// A negative value runs the other way from zero; a missing one draws nothing and says nothing.
pub(crate) fn bars(y: &[YColumn]) -> Bars {
    let lanes_at = |point: usize| y.iter().enumerate().map(|(column, y)| lane(column, y.values[point])).collect();
    Bars::new(bars::points(y).map(lanes_at).collect())
}

fn lane(column: usize, value: Option<f64>) -> Lane {
    let piece = value.filter(|&value| value != 0.0).map(|to| Piece { column, from: 0.0, to });
    Lane { pieces: piece.into_iter().collect(), value }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::ColumnName;

    fn y(name: &str, values: &[Option<f64>]) -> YColumn {
        YColumn { name: ColumnName::new(name), values: values.to_vec() }
    }

    #[test]
    fn each_column_has_its_own_bar_at_each_point() {
        let bars = bars(&[y("orders", &[Some(3.0)]), y("refunds", &[Some(-1.0)])]);
        let pieces: Vec<&[Piece]> = bars.points[0].iter().map(|lane| lane.pieces.as_slice()).collect();
        assert_eq!(pieces, [[Piece { column: 0, from: 0.0, to: 3.0 }], [Piece { column: 1, from: 0.0, to: -1.0 }]]);
        assert_eq!((bars.low, bars.high), (-1.0, 3.0));
    }

    #[test]
    fn a_missing_value_keeps_its_place_empty_and_says_nothing() {
        let bars = bars(&[y("orders", &[None]), y("refunds", &[Some(2.0)])]);
        assert_eq!(bars.points[0][0], Lane { pieces: vec![], value: None });
    }

    #[test]
    fn a_zero_draws_nothing_and_says_zero() {
        assert_eq!(bars(&[y("orders", &[Some(0.0)])]).points[0][0], Lane { pieces: vec![], value: Some(0.0) });
    }
}
