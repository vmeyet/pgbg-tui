//! Intervals as short durations: `1y 2mo 3d`, `2h 30m`, `350ms`.
use crate::db::model::Interval;

/// The most units a duration reads in; the last one shown is rounded.
const MOST_UNITS: usize = 3;
const MONTHS_IN_A_YEAR: i128 = 12;
const MICROSECONDS_IN_A_DAY: i128 = 86_400_000_000;
/// Each unit an interval's time reads in, largest first, and its microseconds.
const TIME_UNITS: [(i128, &str); 5] = [(3_600_000_000, "h"), (60_000_000, "m"), (1_000_000, "s"), (1000, "ms"), (1, "µs")];
const ZERO: &str = "0s";

/// One unit of a duration: how many, and its name.
struct Part {
    count: i128,
    unit: &'static str,
}

/// The parts of an interval, wide enough that no unit overflows.
#[derive(Clone, Copy)]
struct Wide {
    months: i128,
    days: i128,
    microseconds: i128,
}

/// `interval` in its three largest units, the last one rounded, without the units at zero: `1y 2mo 3d`, `0s`.
/// A sign shared by every unit leads, `-2h 30m`; units of mixed signs keep each its own, `1mo -2d`.
pub(super) fn duration(interval: Interval) -> String {
    let shown: Vec<Part> = parts(rounded(wide(interval))).into_iter().filter(|part| part.count != 0).take(MOST_UNITS).collect();
    if shown.is_empty() {
        return ZERO.to_owned();
    }
    let is_negative = shown.iter().all(|part| part.count < 0);
    let words: Vec<String> =
        shown.iter().map(|part| format!("{}{}", if is_negative { part.count.abs() } else { part.count }, part.unit)).collect();
    format!("{}{}", if is_negative { "-" } else { "" }, words.join(" "))
}

fn wide(interval: Interval) -> Wide {
    Wide { months: interval.months.into(), days: interval.days.into(), microseconds: interval.microseconds.into() }
}

/// Every unit, largest first: years, months and days, then the time's units.
fn parts(wide: Wide) -> Vec<Part> {
    let calendar = [(wide.months / MONTHS_IN_A_YEAR, "y"), (wide.months % MONTHS_IN_A_YEAR, "mo"), (wide.days, "d")];
    let time = TIME_UNITS.iter().enumerate().map(|(at, &(step, unit))| {
        let within = at.checked_sub(1).map_or(wide.microseconds, |larger| wide.microseconds % TIME_UNITS[larger].0);
        (within / step, unit)
    });
    calendar.into_iter().chain(time).map(|(count, unit)| Part { count, unit }).collect()
}

/// `wide` rounded to its third unit that is not zero, so its first three read it closest: the time to that unit, or
/// into whole days when the third is days.
fn rounded(wide: Wide) -> Wide {
    let Some(third) = parts(wide).into_iter().filter(|part| part.count != 0).nth(MOST_UNITS - 1) else { return wide };
    match TIME_UNITS.iter().find(|&&(_, unit)| unit == third.unit) {
        Some(&(step, _)) => Wide { microseconds: rounded_to(wide.microseconds, step), ..wide },
        None => {
            Wide { days: wide.days + rounded_to(wide.microseconds, MICROSECONDS_IN_A_DAY) / MICROSECONDS_IN_A_DAY, microseconds: 0, ..wide }
        }
    }
}

/// `value` to the nearest multiple of `step`, a half away from zero.
fn rounded_to(value: i128, step: i128) -> i128 {
    (value + value.signum() * (step / 2)) / step * step
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(months: i32, days: i32, microseconds: i64) -> String {
        duration(Interval { months, days, microseconds })
    }

    #[test]
    fn a_duration_reads_in_its_three_largest_units() {
        assert_eq!(read(14, 3, 14_706_789_000), "1y 2mo 3d");
    }

    #[test]
    fn a_duration_drops_its_units_at_zero() {
        assert_eq!([read(0, 0, 9_000_000_000), read(0, 0, 45_000_000), read(0, 0, 350_000)], ["2h 30m", "45s", "350ms"]);
    }

    #[test]
    fn a_zero_duration_reads_as_zero_seconds() {
        assert_eq!(read(0, 0, 0), "0s");
    }

    #[test]
    fn the_last_unit_shown_is_rounded_and_carries() {
        assert_eq!(read(0, 0, 9_045_600_000), "2h 30m 46s");
        assert_eq!(read(0, 0, 7_199_600_000), "2h");
    }

    #[test]
    fn days_shown_last_are_rounded_by_the_time() {
        assert_eq!(read(14, 2, 43_200_000_000), "1y 2mo 3d");
    }

    #[test]
    fn a_negative_duration_leads_with_its_sign() {
        assert_eq!([read(0, 0, -1_000_000), read(-14, 0, 0)], ["-1s", "-1y 2mo"]);
    }

    #[test]
    fn units_of_mixed_signs_keep_each_its_own() {
        assert_eq!([read(1, -2, 0), read(0, -1, 7_200_000_000)], ["1mo -2d", "-1d 2h"]);
    }

    #[test]
    fn microseconds_read_as_themselves() {
        assert_eq!(read(0, 0, 1), "1µs");
    }

    #[test]
    fn the_largest_parts_read_without_overflow() {
        assert_eq!(read(i32::MIN, i32::MIN, i64::MIN), "-178956970y 8mo 2254235639d");
    }
}
