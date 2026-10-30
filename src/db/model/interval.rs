//! An `interval` as PostgreSQL keeps it: months, days and microseconds, each with its own sign, never folded together.
use std::cmp::Ordering;
use std::fmt;

const MONTHS_IN_A_YEAR: i32 = 12;
const MICROSECONDS_IN_A_SECOND: u64 = 1_000_000;
const MICROSECONDS_IN_A_MINUTE: u64 = 60 * MICROSECONDS_IN_A_SECOND;
const MICROSECONDS_IN_AN_HOUR: u64 = 60 * MICROSECONDS_IN_A_MINUTE;
const MICROSECONDS_IN_A_DAY: i128 = 86_400_000_000;
/// How PostgreSQL compares a month with days: as 30 of them.
const DAYS_IN_A_MONTH: i128 = 30;

/// The three parts of an interval: a month and a day have no fixed length, so each part stays apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Interval {
    pub(crate) months: i32,
    pub(crate) days: i32,
    pub(crate) microseconds: i64,
}

impl Interval {
    /// How it sorts against `other`, as PostgreSQL does: a month as 30 days, a day as 24 hours, so `1 mon` equals
    /// `30 days`.
    pub(crate) fn compare(&self, other: &Self) -> Ordering {
        self.length().cmp(&other.length())
    }

    /// Whether it is shorter than nothing, as [`Interval::compare`] reads it: `1 mon -40 days` is.
    pub(crate) fn is_negative(&self) -> bool {
        self.length() < 0
    }

    fn length(&self) -> i128 {
        let days = i128::from(self.months) * DAYS_IN_A_MONTH + i128::from(self.days);
        days * MICROSECONDS_IN_A_DAY + i128::from(self.microseconds)
    }
}

/// As PostgreSQL prints it with `IntervalStyle` `postgres`: `1 year 2 mons -3 days +04:05:06.789`, `00:00:00`.
impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts = [(self.months / MONTHS_IN_A_YEAR, "year"), (self.months % MONTHS_IN_A_YEAR, "mon"), (self.days, "day")];
        let mut is_first = true;
        let mut is_after_negative = false;
        for (count, unit) in parts.into_iter().filter(|&(count, _)| count != 0) {
            let space = if is_first { "" } else { " " };
            let plus = if is_after_negative && count > 0 { "+" } else { "" };
            let plural = if count == 1 { "" } else { "s" };
            write!(f, "{space}{plus}{count} {unit}{plural}")?;
            (is_first, is_after_negative) = (false, count < 0);
        }
        if is_first || self.microseconds != 0 {
            let space = if is_first { "" } else { " " };
            let sign = if self.microseconds < 0 {
                "-"
            } else if is_after_negative {
                "+"
            } else {
                ""
            };
            write!(f, "{space}{sign}{}", TimeOfDay(self.microseconds.unsigned_abs()))?;
        }
        Ok(())
    }
}

/// Microseconds as `hh:mm:ss` and the fraction's digits that are not trailing zeros: `04:05:06.789`, `100:00:00`.
struct TimeOfDay(u64);

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (hours, minutes) = (self.0 / MICROSECONDS_IN_AN_HOUR, self.0 % MICROSECONDS_IN_AN_HOUR / MICROSECONDS_IN_A_MINUTE);
        let seconds = self.0 % MICROSECONDS_IN_A_MINUTE / MICROSECONDS_IN_A_SECOND;
        write!(f, "{hours:02}:{minutes:02}:{seconds:02}")?;
        let fraction = self.0 % MICROSECONDS_IN_A_SECOND;
        if fraction == 0 {
            return Ok(());
        }
        write!(f, ".{}", format!("{fraction:06}").trim_end_matches('0'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interval(months: i32, days: i32, microseconds: i64) -> Interval {
        Interval { months, days, microseconds }
    }

    #[test]
    fn an_interval_prints_its_years_months_days_and_time_as_postgres_does() {
        assert_eq!(interval(14, 3, 14_706_789_000).to_string(), "1 year 2 mons 3 days 04:05:06.789");
    }

    #[test]
    fn a_zero_interval_prints_as_a_zero_time() {
        assert_eq!(Interval::default().to_string(), "00:00:00");
    }

    #[test]
    fn a_negative_time_prints_its_sign_before_the_hours() {
        assert_eq!(interval(0, 0, -1_000_000).to_string(), "-00:00:01");
    }

    #[test]
    fn a_part_of_another_sign_than_the_one_before_prints_it() {
        assert_eq!(interval(1, -2, 0).to_string(), "1 mon -2 days");
        assert_eq!(interval(0, -1, 7_200_000_000).to_string(), "-1 days +02:00:00");
        assert_eq!(interval(-14, 0, 0).to_string(), "-1 years -2 mons");
    }

    #[test]
    fn one_of_a_unit_is_singular_and_whole_days_print_no_time() {
        assert_eq!([interval(12, 0, 0), interval(1, 1, 0)].map(|interval| interval.to_string()), ["1 year", "1 mon 1 day"]);
    }

    #[test]
    fn a_fraction_keeps_its_microseconds_and_hours_go_past_a_day() {
        assert_eq!(interval(0, 0, 1).to_string(), "00:00:00.000001");
        assert_eq!(interval(0, 0, 360_000_000_000).to_string(), "100:00:00");
    }

    #[test]
    fn a_month_compares_as_thirty_days() {
        let [month, thirty, thirty_one] = [interval(1, 0, 0), interval(0, 30, 0), interval(0, 31, 0)];
        assert_eq!((month.compare(&thirty), month.compare(&thirty_one)), (Ordering::Equal, Ordering::Less));
    }

    #[test]
    fn a_day_compares_as_twenty_four_hours() {
        assert_eq!(interval(0, 1, 0).compare(&interval(0, 0, 90_000_000_000)), Ordering::Less);
    }

    #[test]
    fn the_largest_parts_compare_without_overflow() {
        assert_eq!(interval(i32::MAX, i32::MAX, i64::MAX).compare(&interval(i32::MIN, i32::MIN, i64::MIN)), Ordering::Greater);
    }

    #[test]
    fn an_interval_is_negative_when_its_parts_add_up_below_zero() {
        assert_eq!([interval(1, -40, 0).is_negative(), interval(1, -2, 0).is_negative()], [true, false]);
    }
}
