//! How a cell reads: for each column kind, the formats it allows, raw first, and the text of one cell in one of them.
//! Pure: the clock and the time zone are passed in.
pub(crate) use columns::{ColumnFormat, Mode, opening, parse_column_formats};
pub(crate) use number::grouped_digits;
pub(crate) use time::relative;

use crate::db::model::{ColumnKind, Value};
use crate::render;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use std::fmt;
use std::str::FromStr;

mod columns;
mod number;
mod time;

/// The digits of a UUID that tell it apart at a glance.
pub(crate) const SHORT_UUID_DIGITS: usize = 8;

/// One way a column's cells read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Format {
    /// The value as PostgreSQL prints it.
    #[default]
    Raw,
    /// A timestamp with time zone in the local time zone, without its offset.
    Local,
    /// A timestamp as the time from now, `3 h ago`, `in 2 d`; a date as the days from today, `yesterday`, `in 2 d`.
    Relative,
    /// A number grouped by thousands: `1 234 567.5`.
    Grouped,
    /// A number of bytes, or a byte string's length, as a size: `1.5 KiB`.
    Bytes,
    /// A UUID's first eight digits.
    Short,
    /// A boolean as `✓` or `✗`.
    Check,
}

impl Format {
    const ALL: [Self; 7] = [Self::Raw, Self::Local, Self::Relative, Self::Grouped, Self::Bytes, Self::Short, Self::Check];

    /// The word a `-- format:` header and the config use for it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Local => "local",
            Self::Relative => "relative",
            Self::Grouped => "grouped",
            Self::Bytes => "bytes",
            Self::Short => "short",
            Self::Check => "check",
        }
    }

    /// The format after this one among those `kind` allows, back to raw after the last.
    pub(crate) fn next(self, kind: &ColumnKind) -> Self {
        let allowed = allowed(kind);
        let at = allowed.iter().position(|&format| format == self).map_or(0, |at| at + 1);
        allowed.get(at).copied().unwrap_or(Self::Raw)
    }
}

/// Why a word is not a format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnknownFormat(String);

impl fmt::Display for UnknownFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = Format::ALL.map(Format::name).join(", ");
        write!(f, "`{}` is not a format: {names}", self.0)
    }
}

impl std::error::Error for UnknownFormat {}

impl FromStr for Format {
    type Err = UnknownFormat;

    fn from_str(word: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|format| format.name() == word).ok_or_else(|| UnknownFormat(word.to_owned()))
    }
}

/// The formats a column of `kind` allows, raw first.
///
/// A timestamp without time zone has no local form, as its zone is unknown; its relative form reads it as UTC.
/// A date has no zone either: its relative form counts days from today's local date.
pub(crate) fn allowed(kind: &ColumnKind) -> &'static [Format] {
    match kind {
        ColumnKind::Int | ColumnKind::Numeric => &[Format::Raw, Format::Grouped, Format::Bytes],
        ColumnKind::Float => &[Format::Raw, Format::Grouped],
        ColumnKind::Timestamp | ColumnKind::Date => &[Format::Raw, Format::Relative],
        ColumnKind::TimestampTz => &[Format::Raw, Format::Local, Format::Relative],
        ColumnKind::Uuid => &[Format::Raw, Format::Short],
        ColumnKind::Bytes => &[Format::Raw, Format::Bytes],
        ColumnKind::Bool => &[Format::Raw, Format::Check],
        ColumnKind::Text | ColumnKind::Json | ColumnKind::Time | ColumnKind::Other => &[Format::Raw],
    }
}

/// The readable format of `kind`: the first after raw, or raw when it has no other.
///
/// A timestamp without time zone stays raw: its relative form reads it as UTC, wrong on a server in another zone.
pub(crate) fn readable(kind: &ColumnKind) -> Format {
    match kind {
        ColumnKind::Timestamp => Format::Raw,
        _ => Format::Raw.next(kind),
    }
}

/// The width a column in `format` needs whatever the clock says: a relative time grows as it runs, `1 min ago` to
/// `59 min ago`.
pub(crate) const fn least_width(format: Format) -> usize {
    match format {
        Format::Relative => time::WIDEST_UNDER_A_YEAR.len(),
        Format::Raw | Format::Local | Format::Grouped | Format::Bytes | Format::Short | Format::Check => 0,
    }
}

/// The moment and the time zone cells are read in, read once at the edge.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Clock<Tz: TimeZone> {
    pub(crate) now: DateTime<Utc>,
    pub(crate) zone: Tz,
}

impl<Tz: TimeZone> Clock<Tz> {
    fn today(&self) -> NaiveDate {
        self.now.with_timezone(&self.zone).date_naive()
    }
}

/// The text of `value` in `format`, escaped; a value the format has no form for (NULL, NaN) reads raw, its first `chars` characters.
pub(crate) fn cell<Tz: TimeZone>(value: &Value, format: Format, clock: &Clock<Tz>, chars: usize) -> String {
    formatted(value, format, clock).unwrap_or_else(|| render::cell_text(value, chars))
}

#[expect(clippy::cast_precision_loss, reason = "a size reads to one decimal, far from the digits a float loses")]
fn formatted<Tz: TimeZone>(value: &Value, format: Format, clock: &Clock<Tz>) -> Option<String> {
    match (format, value) {
        (Format::Local, Value::TimestampTz(at)) => Some(time::local(*at, &clock.zone)),
        (Format::Relative, Value::TimestampTz(at)) => Some(relative(*at, clock.now)),
        (Format::Relative, Value::Timestamp(at)) => Some(relative(at.and_utc(), clock.now)),
        (Format::Relative, Value::Date(day)) => Some(time::relative_day(*day, clock.today())),
        (Format::Grouped, Value::Int(int)) => Some(number::grouped_int(*int)),
        (Format::Grouped, Value::Float(float)) => number::grouped_decimal(&float.to_string()),
        (Format::Grouped, Value::Numeric(text)) => number::grouped_decimal(text),
        (Format::Bytes, Value::Int(int)) => Some(number::size(*int as f64)),
        (Format::Bytes, Value::Numeric(text)) => text.parse().ok().filter(|bytes: &f64| bytes.is_finite()).map(number::size),
        (Format::Bytes, Value::Bytes(bytes)) => Some(number::size(bytes.len() as f64)),
        (Format::Short, Value::Uuid(id)) => Some(id.simple().to_string()[..SHORT_UUID_DIGITS].to_owned()),
        (Format::Check, Value::Bool(value)) => Some(if *value { "✓" } else { "✗" }.to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    const CHARS: usize = 40;

    fn paris_summer() -> Clock<FixedOffset> {
        let now = DateTime::parse_from_rfc3339("2026-07-14T12:00:00Z").unwrap().to_utc();
        Clock { now, zone: FixedOffset::east_opt(2 * 3600).unwrap() }
    }

    fn read(value: &Value, format: Format) -> String {
        cell(value, format, &paris_summer(), CHARS)
    }

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339).unwrap().to_utc()
    }

    #[test]
    fn every_kind_allows_raw_first() {
        let kinds = [ColumnKind::Int, ColumnKind::TimestampTz, ColumnKind::Text, ColumnKind::Other];
        assert!(kinds.iter().all(|kind| allowed(kind)[0] == Format::Raw));
    }

    #[test]
    fn next_steps_through_a_kinds_formats_then_back_to_raw() {
        let steps = std::iter::successors(Some(Format::Raw), |format| Some(format.next(&ColumnKind::TimestampTz))).take(4);
        assert_eq!(steps.collect::<Vec<_>>(), [Format::Raw, Format::Local, Format::Relative, Format::Raw]);
    }

    #[test]
    fn next_on_a_format_the_kind_does_not_allow_goes_back_to_raw() {
        assert_eq!(Format::Short.next(&ColumnKind::Int), Format::Raw);
    }

    #[test]
    fn a_kind_with_one_format_stays_raw() {
        assert_eq!(Format::Raw.next(&ColumnKind::Text), Format::Raw);
    }

    #[test]
    fn the_readable_format_is_the_first_after_raw() {
        assert_eq!([readable(&ColumnKind::Int), readable(&ColumnKind::Uuid)], [Format::Grouped, Format::Short]);
    }

    #[test]
    fn a_timestamp_without_time_zone_stays_raw_when_readable_and_steps_to_relative() {
        assert_eq!((readable(&ColumnKind::Timestamp), Format::Raw.next(&ColumnKind::Timestamp)), (Format::Raw, Format::Relative));
    }

    #[test]
    fn a_relative_column_is_as_wide_as_the_widest_relative_time_under_a_year() {
        let widest =
            (1..60 * 24 * 365).map(|minutes| relative(paris_summer().now - chrono::Duration::minutes(minutes), paris_summer().now));
        assert_eq!(widest.map(|text| text.len()).max(), Some(least_width(Format::Relative)));
        assert_eq!(least_width(Format::Grouped), 0);
    }

    #[test]
    fn a_relative_time_up_to_a_thousand_years_either_way_fits_a_relative_column() {
        let now = paris_summer().now;
        let years = (1..=1000).flat_map(|years| [now - chrono::Months::new(12 * years), now + chrono::Months::new(12 * years)]);
        assert!(years.map(|at| relative(at, now).len()).all(|width| width <= least_width(Format::Relative)));
    }

    #[test]
    fn a_kind_with_no_readable_form_reads_raw() {
        assert_eq!(readable(&ColumnKind::Json), Format::Raw);
    }

    #[test]
    fn a_format_parses_from_its_name() {
        assert_eq!(Format::ALL.map(|format| format.name().parse()), Format::ALL.map(Ok));
    }

    #[test]
    fn an_unknown_format_name_says_which_ones_exist() {
        let error = "pretty".parse::<Format>().unwrap_err();
        assert_eq!(error.to_string(), "`pretty` is not a format: raw, local, relative, grouped, bytes, short, check");
    }

    #[test]
    fn a_raw_cell_reads_as_postgres_prints_it_escaped() {
        assert_eq!(read(&Value::Text("nina\nnoel".into()), Format::Raw), "nina\\nnoel");
    }

    #[test]
    fn a_raw_cell_stops_after_its_characters() {
        assert_eq!(cell(&Value::Text("Zoë Nakamura".into()), Format::Raw, &paris_summer(), 3), "Zoë");
    }

    #[test]
    fn a_null_reads_raw_in_any_format() {
        assert_eq!(read(&Value::Null, Format::Relative), "NULL");
    }

    #[test]
    fn a_timestamp_with_time_zone_reads_in_the_local_zone() {
        assert_eq!(read(&Value::TimestampTz(at("2026-07-14T09:30:15.25Z")), Format::Local), "2026-07-14 11:30:15");
    }

    #[test]
    fn a_timestamp_with_time_zone_reads_relative_to_now() {
        assert_eq!(read(&Value::TimestampTz(at("2026-07-14T09:00:00Z")), Format::Relative), "3 h ago");
    }

    #[test]
    fn a_timestamp_without_time_zone_reads_relative_as_utc() {
        let at = at("2026-07-13T12:00:00Z").naive_utc();
        assert_eq!(read(&Value::Timestamp(at), Format::Relative), "1 d ago");
    }

    #[test]
    fn a_date_reads_relative_to_todays_local_date() {
        let day = "2026-07-13".parse().unwrap();
        assert_eq!(read(&Value::Date(day), Format::Relative), "yesterday");
    }

    #[test]
    fn a_date_reads_relative_to_the_local_date_when_it_differs_from_the_utc_one() {
        let late_evening_in_new_york = Clock { now: at("2026-07-15T02:00:00Z"), zone: FixedOffset::west_opt(4 * 3600).unwrap() };
        let today = "2026-07-14".parse().unwrap();
        assert_eq!(cell(&Value::Date(today), Format::Relative, &late_evening_in_new_york, CHARS), "today");
    }

    #[test]
    fn a_date_steps_to_relative_and_is_relative_when_readable() {
        assert_eq!((Format::Raw.next(&ColumnKind::Date), readable(&ColumnKind::Date)), (Format::Relative, Format::Relative));
    }

    #[test]
    fn an_infinite_timestamp_reads_raw() {
        assert_eq!(read(&Value::Infinity, Format::Relative), "infinity");
    }

    #[test]
    fn an_integer_reads_grouped() {
        assert_eq!(read(&Value::Int(-1234567), Format::Grouped), "-1 234 567");
    }

    #[test]
    fn a_float_reads_grouped_with_its_fraction() {
        assert_eq!(read(&Value::Float(12345.5), Format::Grouped), "12 345.5");
    }

    #[test]
    fn a_float_that_is_not_finite_reads_raw_when_grouped() {
        assert_eq!(read(&Value::Float(f64::NAN), Format::Grouped), "NaN");
    }

    #[test]
    fn a_numeric_reads_grouped_with_all_its_digits() {
        assert_eq!(read(&Value::Numeric("98765432109876543210.0001".into()), Format::Grouped), "98 765 432 109 876 543 210.0001");
    }

    #[test]
    fn a_numeric_not_a_number_reads_raw_when_grouped() {
        assert_eq!(read(&Value::Numeric("NaN".into()), Format::Grouped), "NaN");
    }

    #[test]
    fn an_integer_reads_as_a_size_in_bytes() {
        assert_eq!(read(&Value::Int(1536), Format::Bytes), "1.5 KiB");
    }

    #[test]
    fn a_numeric_reads_as_a_size_in_bytes() {
        assert_eq!(read(&Value::Numeric("5368709120".into()), Format::Bytes), "5.0 GiB");
    }

    #[test]
    fn a_byte_string_reads_as_its_length() {
        assert_eq!(read(&Value::Bytes(vec![0; 300].into()), Format::Bytes), "300 B");
    }

    #[test]
    fn a_uuid_reads_short_as_its_first_eight_digits() {
        let id = uuid::Uuid::parse_str("1b4e28ba-2fa1-11d2-883f-0016d3cca427").unwrap();
        assert_eq!(read(&Value::Uuid(id), Format::Short), "1b4e28ba");
    }

    #[test]
    fn a_boolean_reads_as_a_check_mark_or_a_cross() {
        assert_eq!([read(&Value::Bool(true), Format::Check), read(&Value::Bool(false), Format::Check)], ["✓", "✗"]);
    }

    #[test]
    fn a_boolean_steps_to_check_and_is_a_check_when_readable() {
        assert_eq!((Format::Raw.next(&ColumnKind::Bool), readable(&ColumnKind::Bool)), (Format::Check, Format::Check));
    }

    #[test]
    fn a_value_of_another_kind_than_the_format_reads_raw() {
        assert_eq!(read(&Value::Text("nina".into()), Format::Grouped), "nina");
    }
}
