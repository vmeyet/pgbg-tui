//! Timestamps in a time zone, and as the time from now.
use chrono::{DateTime, TimeZone, Utc};

/// Each unit a relative time reads in, largest first, and its seconds: a month is 30 days, a year 365.
const UNITS: [(u64, &str); 5] = [(365 * 86_400, "y"), (30 * 86_400, "mo"), (86_400, "d"), (3600, "h"), (60, "min")];

/// `at` in `zone`, to the second, without its offset: `2026-07-14 11:30:15`.
pub(super) fn local<Tz: TimeZone>(at: DateTime<Utc>, zone: &Tz) -> String {
    at.with_timezone(zone).naive_local().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// The time from `now` to `at` in its largest whole unit: `3 h ago`, `in 2 d`, `now` under a minute either way.
pub(super) fn relative(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = (now - at).num_seconds();
    let Some(&(step, name)) = UNITS.iter().find(|&&(step, _)| seconds.unsigned_abs() >= step) else {
        return "now".to_owned();
    };
    let amount = seconds.unsigned_abs() / step;
    if seconds < 0 { format!("in {amount} {name}") } else { format!("{amount} {name} ago") }
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
}
