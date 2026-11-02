//! Bars, whatever their kind: lanes of pieces from `chart::Bars`, on their side, and as many points as the pane holds.
//! A number sits on its bar at zero, so a bar reads from there.
use super::{colour, short_number};
use crate::chart::{Bars, Lane, Series};
use crate::render::{self, printable};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols::block::FULL as BLOCK;
use ratatui::text::Line;
use vtui::text::truncate;
use vtui::theme::Theme;

/// The share of the width a label takes at most from bars on their side.
const COLUMNS_PER_LABEL_COLUMN: u16 = 3;

/// What every part of the drawing reads.
struct Drawing<'a> {
    bars: &'a Bars,
    labels: &'a [String],
    /// Rows left out before the drawing, so a count of its own is only a lower bound.
    is_cut: bool,
    theme: &'a Theme,
}

pub(super) fn draw(f: &mut Frame<'_>, bars: &Bars, series: &Series, theme: &Theme, area: Rect) {
    Drawing { bars, labels: &series.labels, is_cut: series.is_cut, theme }.on_their_side(f, area);
}

impl Drawing<'_> {
    /// How many lanes each point has; a `Series` with no point never reaches the drawing, as it is `ChartError::Empty`.
    fn lanes_per_point(&self) -> u16 {
        cells(self.bars.points.first().map_or(1, Vec::len))
    }

    /// One row per lane and a blank one between points of several lanes; labels on the left, the first lane's row.
    fn on_their_side(&self, f: &mut Frame<'_>, area: Rect) {
        let lanes = self.lanes_per_point();
        let gap = u16::from(lanes > 1);
        let points = self.bars.points.len();
        let has_note = self.is_cut || fitting(area.height, lanes, gap) < points;
        let rows = if has_note { area.height.saturating_sub(1) } else { area.height };
        let shown = fitting(rows, lanes, gap).min(points);
        let area = self.say_left_out(f, points - shown, area);
        let label_width = cells(self.widest_label(shown)).min(area.width / COLUMNS_PER_LABEL_COLUMN);
        let [labels, _, plot] =
            Layout::horizontal([Constraint::Length(label_width), Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        let tops = (plot.y..).step_by(usize::from(lanes.saturating_add(gap)));
        for (index, (point, top)) in self.bars.points[..shown].iter().zip(tops).enumerate() {
            self.draw_label(f, Rect { y: top, height: 1, ..labels }, index);
            for (lane, y) in point.iter().zip(top..) {
                self.draw_lane(f.buffer_mut(), Rect { y, height: 1, ..plot }, lane);
            }
        }
    }

    /// The widest of the first `shown` labels, in columns.
    fn widest_label(&self, shown: usize) -> usize {
        self.labels.iter().take(shown).map(|label| render::width(&printable(label))).max().unwrap_or(0)
    }

    fn draw_label(&self, f: &mut Frame<'_>, area: Rect, point: usize) {
        let label = truncate(&printable(&self.labels[point]), usize::from(area.width));
        f.render_widget(Line::styled(label, Style::default().fg(self.theme.muted)), area);
    }

    fn draw_lane(&self, buffer: &mut Buffer, track: Rect, lane: &Lane) {
        for piece in &lane.pieces {
            let [from, to] = [piece.from, piece.to].map(|value| self.cell_of(value, track));
            for cell in from.min(to)..from.max(to) {
                fill(buffer, Self::slice(track, cell), colour(self.theme, piece.column));
            }
        }
        if let Some(value) = lane.value {
            self.draw_value(buffer, track, value);
        }
    }

    /// `value` on its bar, at zero, if the bar's track holds it whole: a number with a digit cut off would read as another number.
    fn draw_value(&self, buffer: &mut Buffer, track: Rect, value: f64) {
        let text = short_number(value);
        let Some(start) = self.value_start(track, cells(text.len()), value < 0.0) else { return };
        for (position, symbol) in (start.x..).map(|x| Position { x, y: start.y }).zip(text.chars()) {
            if let Some(cell) = buffer.cell_mut(position) {
                let on_bar = cell.symbol() == BLOCK;
                let style = if on_bar { Style::default().fg(self.theme.base).bg(cell.fg) } else { Style::default().fg(self.theme.muted) };
                cell.set_char(symbol).set_style(style);
            }
        }
    }

    /// Where a number of `width` starts: from zero along its bar.
    fn value_start(&self, track: Rect, width: u16, is_negative: bool) -> Option<Position> {
        if track.width < width {
            return None;
        }
        let zero = self.cell_of(0.0, track);
        let from_zero = if is_negative { zero.saturating_sub(width) } else { zero };
        Some(Position { x: track.x + from_zero.min(track.width - width), y: track.y })
    }

    /// The cell of `track` a value falls on, counted from the low end of the scale.
    fn cell_of(&self, value: f64, track: Rect) -> u16 {
        let span = self.bars.high - self.bars.low;
        if span > 0.0 { ((value - self.bars.low) / span * f64::from(track.width)).round() as u16 } else { 0 }
    }

    /// Cell `cell` of `track`, across its thickness.
    const fn slice(track: Rect, cell: u16) -> Rect {
        Rect { x: track.x + cell, width: 1, ..track }
    }

    /// Says on the last row of `area` what is not drawn, and gives the rows above it; all of `area` when nothing is left out.
    fn say_left_out(&self, f: &mut Frame<'_>, did_not_fit: usize, area: Rect) -> Rect {
        let Some(note) = left_out_note(did_not_fit, self.is_cut) else { return area };
        let [rest, note_row] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        f.render_widget(Line::styled(note, Style::default().fg(self.theme.faded)), note_row);
        rest
    }
}

/// `did_not_fit` points the pane has no room for; when `is_cut`, the series left out rows before them, how many it does not know.
fn left_out_note(did_not_fit: usize, is_cut: bool) -> Option<String> {
    match (did_not_fit, is_cut) {
        (0, false) => None,
        (0, true) => Some("more rows not drawn".to_owned()),
        (count, false) => Some(format!("+{count} more")),
        (count, true) => Some(format!("+{count} or more")),
    }
}

/// A count of cells, past what a terminal holds when it does not fit.
fn cells(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

/// How many points fit along `room` cells when each takes `size` and two of them are `gap` apart.
fn fitting(room: u16, size: u16, gap: u16) -> usize {
    (usize::from(room) + usize::from(gap)) / (usize::from(size) + usize::from(gap))
}

fn fill(buffer: &mut Buffer, area: Rect, colour: Color) {
    for position in area.positions() {
        if let Some(cell) = buffer.cell_mut(position) {
            cell.set_symbol(BLOCK).set_fg(colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{daily_orders_and_refunds, render, sales, series};
    use super::*;
    use crate::chart::ChartError;
    use crate::db::model::Value;

    const KINDS: [&str; 1] = ["bar"];

    #[test]
    fn bars_are_as_long_as_their_value_and_say_it() {
        insta::assert_snapshot!("bars", render(&series(&sales(), "bar y=orders"), 40, 4).backend());
    }

    #[test]
    fn a_gap_draws_no_bar() {
        let sales = sales().with_cell(1, 1, Value::Null);
        let drawn: Result<_, ChartError> = series(&sales, "bar y=orders");
        insta::assert_snapshot!("bars_gap", render(&drawn, 40, 4).backend());
    }

    #[test]
    fn grouped_bars_lie_one_under_the_other() {
        insta::assert_snapshot!("grouped_horizontal", render(&series(&daily_orders_and_refunds(), "bar"), 48, 10).backend());
    }

    #[test]
    fn a_negative_value_runs_the_other_way_from_zero() {
        let daily = daily_orders_and_refunds().with_cell(0, 2, Value::Int(-40));
        insta::assert_snapshot!("bars_negative", render(&series(&daily, "bar"), 48, 10).backend());
    }

    #[test]
    fn a_narrow_pane_says_how_many_points_it_left_out() {
        insta::assert_snapshot!("left_out_on_their_side", render(&series(&daily_orders_and_refunds(), "bar"), 20, 5).backend());
    }

    #[test]
    fn rows_left_out_before_the_drawing_make_its_count_a_lower_bound() {
        assert_eq!(left_out_note(0, false), None);
        assert_eq!(left_out_note(3, false).as_deref(), Some("+3 more"));
        assert_eq!(left_out_note(3, true).as_deref(), Some("+3 or more"));
        assert_eq!(left_out_note(0, true).as_deref(), Some("more rows not drawn"));
    }

    #[test]
    fn no_pane_is_too_small_to_draw_in() {
        for kind in KINDS {
            let chart = series(&daily_orders_and_refunds(), kind);
            for (width, height) in (0..30).flat_map(|width| (0..12).map(move |height| (width, height))) {
                render(&chart, width, height);
            }
        }
    }

    #[test]
    fn hundreds_of_columns_draw_in_a_small_pane() {
        let wide = super::super::tests::wide(300);
        for kind in KINDS {
            render(&series(&wide, kind), 20, 6);
        }
    }

    #[test]
    fn a_result_of_zeros_has_no_scale_to_divide_by() {
        let cells = (0..4).flat_map(|row| [(row, 1), (row, 2)]);
        let daily = cells.fold(daily_orders_and_refunds(), |daily, (row, column)| daily.with_cell(row, column, Value::Int(0)));
        for kind in KINDS {
            render(&series(&daily, kind), 30, 8);
        }
    }
}
