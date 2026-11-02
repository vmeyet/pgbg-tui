//! Stacked bars: each point has one bar, its y columns piled along it.
use super::YColumn;
use super::bars::{self, Bars, Lane, Piece};

/// Positive values pile up from zero and negative ones pile down from it, each in the order of the y columns, so no value
/// hides another and each end reads as the sum of its sign.
/// A missing value (NULL, NaN, infinity) adds nothing and takes no room.
/// The bar says the net sum of the values it has, or nothing when it has none.
pub(crate) fn bars(y: &[YColumn]) -> Bars {
    Bars::new(bars::points(y).map(|point| vec![lane(y, point)]).collect())
}

fn lane(y: &[YColumn], point: usize) -> Lane {
    let values: Vec<(usize, f64)> = y.iter().enumerate().filter_map(|(column, y)| Some((column, y.values[point]?))).collect();
    let above = pile(values.iter().copied().filter(|&(_, value)| value > 0.0));
    let below = pile(values.iter().copied().filter(|&(_, value)| value < 0.0));
    let net = (!values.is_empty()).then(|| values.iter().map(|&(_, value)| value).sum());
    Lane { pieces: above.into_iter().chain(below).collect(), value: net }
}

/// `values`, each starting where the one before it ended.
fn pile(values: impl Iterator<Item = (usize, f64)>) -> Vec<Piece> {
    values
        .scan(0.0, |end, (column, value)| {
            let from = *end;
            *end += value;
            Some(Piece { column, from, to: *end })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::ColumnName;

    fn y(values: &[Option<f64>]) -> YColumn {
        YColumn { name: ColumnName::new("orders"), values: values.to_vec() }
    }

    fn piece(column: usize, from: f64, to: f64) -> Piece {
        Piece { column, from, to }
    }

    #[test]
    fn values_pile_up_in_the_order_of_the_columns() {
        let bars = bars(&[y(&[Some(3.0)]), y(&[Some(2.0)])]);
        assert_eq!(bars.points[0], [Lane { pieces: vec![piece(0, 0.0, 3.0), piece(1, 3.0, 5.0)], value: Some(5.0) }]);
        assert_eq!((bars.low, bars.high), (0.0, 5.0));
    }

    #[test]
    fn negative_values_pile_down_from_zero_and_the_bar_says_the_net_sum() {
        let bars = bars(&[y(&[Some(3.0)]), y(&[Some(-1.0)]), y(&[Some(-2.0)]), y(&[Some(4.0)])]);
        let pieces = vec![piece(0, 0.0, 3.0), piece(3, 3.0, 7.0), piece(1, 0.0, -1.0), piece(2, -1.0, -3.0)];
        assert_eq!(bars.points[0], [Lane { pieces, value: Some(4.0) }]);
        assert_eq!((bars.low, bars.high), (-3.0, 7.0));
    }

    #[test]
    fn a_missing_value_adds_nothing_and_takes_no_room() {
        let bars = bars(&[y(&[Some(3.0)]), y(&[None]), y(&[Some(2.0)])]);
        assert_eq!(bars.points[0], [Lane { pieces: vec![piece(0, 0.0, 3.0), piece(2, 3.0, 5.0)], value: Some(5.0) }]);
    }

    #[test]
    fn a_point_with_no_value_has_an_empty_bar_that_says_nothing() {
        assert_eq!(bars(&[y(&[None]), y(&[None])]).points[0], [Lane { pieces: vec![], value: None }]);
    }

    #[test]
    fn zeros_take_no_room_and_the_bar_says_zero() {
        assert_eq!(bars(&[y(&[Some(0.0)])]).points[0], [Lane { pieces: vec![], value: Some(0.0) }]);
    }
}
