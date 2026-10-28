//! Timestamps in a time zone, and timestamps and dates as the time from now.
use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeZone, Utc};

/// Each unit under a month a relative time reads in, largest first, and its seconds.
const UNITS: [(u64, &str); 3] = [(86_400, "d"), (3600, "h"), (60, "min")];

const MONTHS_IN_A_YEAR: u64 = 12;

/// The widest text `relative` gives for a time less than a year away.
pub(super) const WIDEST_UNDER_A_YEAR: &str = "59 min ago";

/// `at` in `zone`, to the second, without its offset: `2026-07-14 11:30:15`.
pub(super) fn local<Tz: TimeZone>(at: DateTime<Utc>, zone: &Tz) -> String {
    at.with_timezone(zone).naive_local().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// The time from `now` to `at` in its largest whole unit: `3 h ago`, `in 2 d`, `now` under a minute either way.
///
/// Months and years count on the calendar, so a year is a year whatever its leap days.
pub(crate) fn relative(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    match (span(at.min(now), at.max(now)), at > now) {
        (None, _) => "now".to_owned(),
        (Some(span), true) => format!("in {span}"),
        (Some(span), false) => format!("{span} ago"),
    }
}

/// The calendar days from `today` to `day` in words, `today`, `yesterday` or `tomorrow`, else as `relative` reads
/// their midnights: `3 d ago`, `in 1 mo`.
pub(super) fn relative_day(day: NaiveDate, today: NaiveDate) -> String {
    match (day - today).num_days() {
        -1 => "yesterday".to_owned(),
        0 => "today".to_owned(),
        1 => "tomorrow".to_owned(),
        _ => relative(midnight(day), midnight(today)),
    }
}

fn midnight(day: NaiveDate) -> DateTime<Utc> {
    day.and_time(NaiveTime::MIN).and_utc()
}

/// The time from `from` to the later `to` in its largest whole unit, as `3 h`; none under a minute.
fn span(from: DateTime<Utc>, to: DateTime<Utc>) -> Option<String> {
    let months = whole_months(from, to);
    if months >= MONTHS_IN_A_YEAR {
        return Some(format!("{} y", months / MONTHS_IN_A_YEAR));
    }
    if months > 0 {
        return Some(format!("{months} mo"));
    }
    let seconds = (to - from).num_seconds().unsigned_abs();
    UNITS.iter().find(|&&(step, _)| seconds >= step).map(|&(step, name)| format!("{} {name}", seconds / step))
}

/// The calendar months from `from` to the later `to`, a month short when `to` has not reached `from`'s day and time.
fn whole_months(from: DateTime<Utc>, to: DateTime<Utc>) -> u64 {
    let short_of_a_month = (to.day(), to.time()) < (from.day(), from.time());
    (month_number(to) - month_number(from) - i64::from(short_of_a_month)).unsigned_abs()
}

fn month_number(at: DateTime<Utc>) -> i64 {
    i64::from(at.year()) * 12 + i64::from(at.month0())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, FixedOffset};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-14T12:00:00Z").unwrap().to_utc()
    }

    fn ago(seconds: i64) -> String {
        relative(now() - Duration::seconds(seconds), now())
    }

    fn since(at: &str) -> String {
        relative(DateTime::parse_from_rfc3339(at).unwrap().to_utc(), now())
    }

    #[test]
    fn local_time_drops_the_fraction_of_a_second() {
        let at = DateTime::parse_from_rfc3339("2026-07-14T09:30:15.999Z").unwrap().to_utc();
        assert_eq!(local(at, &FixedOffset::west_opt(5 * 3600).unwrap()), "2026-07-14 04:30:15");
    }

    #[test]
    fn under_a_minute_either_way_is_now() {
        assert_eq!([ago(0), ago(59), ago(-59)], ["now", "now", "now"]);
    }

    #[test]
    fn a_past_time_reads_in_its_largest_whole_unit() {
        let units = [ago(60), ago(59 * 60), ago(3 * 3600 + 59 * 60), ago(2 * 86_400), ago(45 * 86_400), ago(800 * 86_400)];
        assert_eq!(units, ["1 min ago", "59 min ago", "3 h ago", "2 d ago", "1 mo ago", "2 y ago"]);
    }

    #[test]
    fn a_future_time_reads_as_in() {
        assert_eq!(ago(-2 * 86_400), "in 2 d");
    }

    #[test]
    fn a_day_short_of_a_calendar_month_reads_in_days() {
        assert_eq!(since("2026-06-14T12:00:01Z"), "29 d ago");
    }

    #[test]
    fn a_calendar_month_reads_as_one_month() {
        assert_eq!(since("2026-06-14T12:00:00Z"), "1 mo ago");
    }

    #[test]
    fn a_day_short_of_a_calendar_year_reads_as_eleven_months() {
        assert_eq!(since("2025-07-15T12:00:00Z"), "11 mo ago");
    }

    #[test]
    fn a_calendar_year_reads_as_one_year_across_a_leap_day() {
        assert_eq!([since("2025-07-14T12:00:00Z"), since("2023-07-14T12:00:00Z")], ["1 y ago", "3 y ago"]);
    }

    #[test]
    fn a_long_span_counts_calendar_years() {
        assert_eq!(since("0026-07-14T12:00:00Z"), "2000 y ago");
    }

    #[test]
    fn a_future_span_over_a_year_reads_as_in() {
        assert_eq!(since("2028-07-14T11:59:59Z"), "in 1 y");
    }

    fn day(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    fn from_bastille_day(text: &str) -> String {
        relative_day(day(text), day("2026-07-14"))
    }

    #[test]
    fn a_day_next_to_today_reads_in_words() {
        let days = [from_bastille_day("2026-07-13"), from_bastille_day("2026-07-14"), from_bastille_day("2026-07-15")];
        assert_eq!(days, ["yesterday", "today", "tomorrow"]);
    }

    #[test]
    fn a_day_further_away_reads_in_whole_days() {
        assert_eq!([from_bastille_day("2026-07-11"), from_bastille_day("2026-07-16")], ["3 d ago", "in 2 d"]);
    }

    #[test]
    fn a_day_a_month_or_more_away_reads_in_calendar_months_and_years() {
        assert_eq!([from_bastille_day("2026-06-14"), from_bastille_day("2024-07-14")], ["1 mo ago", "2 y ago"]);
    }
}
