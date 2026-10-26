use super::ChartKind;
use crate::db::model::{ColumnKind, Value};
use chrono::{DateTime, NaiveTime, Timelike, Utc};

/// What a point's x position means: its row, or its place in time, so a gap in time shows as a gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum XAxis {
    /// The row's place in the result: bars, and a line over text.
    Rows,
    /// Seconds since 1970-01-01 UTC, written as a date.
    Dates,
    /// Seconds since 1970-01-01 UTC, written as a date and a time.
    Timestamps,
    /// Seconds since midnight.
    TimesOfDay,
}

impl XAxis {
    /// Only a line goes on a time axis: bars stay one per row, in the result's order.
    pub(super) const fn of(kind: ChartKind, x: &ColumnKind) -> Self {
        match (kind, x) {
            (ChartKind::Line, ColumnKind::Date) => Self::Dates,
            (ChartKind::Line, ColumnKind::Timestamp | ColumnKind::TimestampTz) => Self::Timestamps,
            (ChartKind::Line, ColumnKind::Time) => Self::TimesOfDay,
            _ => Self::Rows,
        }
    }

    /// Where row `row`, whose x is `value`, sits on the axis; a value with no place in time (NULL, infinity) has none.
    #[expect(clippy::cast_precision_loss, reason = "a row index is far below 2^53")]
    pub(super) fn position(self, row: usize, value: &Value) -> Option<f64> {
        match (self, value) {
            (Self::Rows, _) => Some(row as f64),
            (Self::Dates, Value::Date(date)) => Some(seconds(date.and_time(NaiveTime::MIN).and_utc())),
            (Self::Timestamps, Value::Timestamp(at)) => Some(seconds(at.and_utc())),
            (Self::Timestamps, Value::TimestampTz(at)) => Some(seconds(*at)),
            (Self::TimesOfDay, Value::Time(time)) => Some(f64::from(time.num_seconds_from_midnight())),
            _ => None,
        }
    }

    /// A position on a time axis written back as the column shows it; `None` on the rows axis, or out of chrono's range.
    pub(crate) fn time_label(self, position: f64) -> Option<String> {
        let seconds = position.round() as i64;
        match self {
            Self::Rows => None,
            Self::Dates => Some(DateTime::from_timestamp(seconds, 0)?.format("%Y-%m-%d").to_string()),
            Self::Timestamps => Some(DateTime::from_timestamp(seconds, 0)?.format("%Y-%m-%d %H:%M").to_string()),
            Self::TimesOfDay => Some(NaiveTime::from_num_seconds_from_midnight_opt(seconds as u32, 0)?.format("%H:%M:%S").to_string()),
        }
    }
}

#[expect(clippy::cast_precision_loss, reason = "milliseconds since 1970 stay below 2^53 until the year 287 396")]
fn seconds(at: DateTime<Utc>) -> f64 {
    at.timestamp_millis() as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, day).unwrap()
    }

    #[test]
    fn a_line_over_time_goes_on_a_time_axis() {
        assert_eq!(XAxis::of(ChartKind::Line, &ColumnKind::Date), XAxis::Dates);
        assert_eq!(XAxis::of(ChartKind::Line, &ColumnKind::TimestampTz), XAxis::Timestamps);
        assert_eq!(XAxis::of(ChartKind::Line, &ColumnKind::Time), XAxis::TimesOfDay);
    }

    #[test]
    fn a_timestamp_with_or_without_zone_sits_at_its_utc_second() {
        let at = date(1).and_hms_opt(9, 30, 0).unwrap();
        assert_eq!(XAxis::Timestamps.position(0, &Value::Timestamp(at)), Some(1_767_259_800.0));
        assert_eq!(XAxis::Timestamps.position(0, &Value::TimestampTz(at.and_utc())), Some(1_767_259_800.0));
    }

    #[test]
    fn a_time_of_day_sits_at_its_second_since_midnight() {
        assert_eq!(XAxis::TimesOfDay.position(0, &Value::Time(NaiveTime::from_hms_opt(1, 0, 30).unwrap())), Some(3630.0));
    }

    #[test]
    fn a_time_position_is_written_back_as_the_column_shows_it() {
        assert_eq!(XAxis::Dates.time_label(1_767_259_800.0).as_deref(), Some("2026-01-01"));
        assert_eq!(XAxis::Timestamps.time_label(1_767_259_800.0).as_deref(), Some("2026-01-01 09:30"));
        assert_eq!(XAxis::TimesOfDay.time_label(3630.0).as_deref(), Some("01:00:30"));
    }
}
