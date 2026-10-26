use anyhow::{Result, anyhow, bail};
use serde::{Serialize, Serializer};
use std::fmt;
use std::time::Duration;

/// The units `statement_timeout` takes, as PostgreSQL spells them, largest first.
const UNITS: [(&str, u64); 4] = [("h", 3_600_000), ("min", 60_000), ("s", 1_000), ("ms", 1)];
/// PostgreSQL keeps `statement_timeout` in an `int` of milliseconds.
const MAX_MS: u64 = i32::MAX as u64;
/// How no timeout prints, in a form `parse` reads back.
const PRINTED_NONE: &str = "0s";
const SPELLING: &str = "`statement_timeout` is a number and a unit, `ms`, `s`, `min` or `h`, such as \"2min\"; \"0s\" turns it off";

/// How long the server lets one statement run before it cancels it; zero lets it run to its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct StatementTimeout(Duration);

impl StatementTimeout {
    pub(super) const NONE: Self = Self(Duration::ZERO);
    pub(super) const STAGING: Self = Self(Duration::from_secs(60));
    pub(super) const PRODUCTION: Self = Self(Duration::from_secs(30));

    /// The timeout `text` says, such as `2min`; zero of any unit is none.
    pub(super) fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let digits = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
        let (number, unit) = text.split_at(digits);
        let number: u64 = number.parse().map_err(|_not_a_number| anyhow!(SPELLING))?;
        let (_, unit_ms) = UNITS.iter().find(|(name, _)| *name == unit.trim()).ok_or_else(|| anyhow!(SPELLING))?;
        let ms = number.checked_mul(*unit_ms).filter(|&ms| ms <= MAX_MS);
        let Some(ms) = ms else { bail!("`statement_timeout` is longer than PostgreSQL keeps: {} days at most", MAX_MS / 86_400_000) };
        Ok(Self(Duration::from_millis(ms)))
    }

    /// The timeout of `ms` milliseconds, as `pg_settings` gives it; zero is none.
    pub(crate) fn from_millis(ms: u64) -> Self {
        Self(Duration::from_millis(ms))
    }

    fn is_none(self) -> bool {
        self == Self::NONE
    }

    /// The shorter of the two, where none is the longest of all.
    fn stricter(self, other: Self) -> Self {
        if self.is_none() {
            return other;
        }
        if other.is_none() {
            return self;
        }
        self.min(other)
    }

    /// The milliseconds `SET statement_timeout` takes; zero for none, as PostgreSQL reads it.
    pub(crate) fn as_millis(self) -> u128 {
        self.0.as_millis()
    }
}

/// The timeout a connection asks for: one it chose, which always holds, or its tier's, which a stricter one the
/// server already applies wins over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimeoutChoice {
    Own(StatementTimeout),
    Tier(StatementTimeout),
}

impl TimeoutChoice {
    /// The timeout a session runs with, given `server`, the one the server would apply to it.
    pub(crate) fn in_effect(self, server: StatementTimeout) -> StatementTimeout {
        match self {
            Self::Own(own) => own,
            Self::Tier(tier) => tier.stricter(server),
        }
    }

    fn timeout(self) -> StatementTimeout {
        match self {
            Self::Own(timeout) | Self::Tier(timeout) => timeout,
        }
    }
}

impl Serialize for TimeoutChoice {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.timeout().serialize(serializer)
    }
}

impl fmt::Display for StatementTimeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = self.0.as_millis() as u64;
        if ms == 0 {
            return f.write_str(PRINTED_NONE);
        }
        let (unit, unit_ms) = UNITS.iter().find(|(_, unit_ms)| ms.is_multiple_of(*unit_ms)).unwrap_or(&("ms", 1));
        write!(f, "{}{unit}", ms / unit_ms)
    }
}

impl Serialize for StatementTimeout {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> String {
        StatementTimeout::parse(text).unwrap().to_string()
    }

    #[test]
    fn a_timeout_is_a_number_and_a_unit() {
        assert_eq!(parsed("2min"), "2min");
        assert_eq!(parsed("90s"), "90s");
        assert_eq!(parsed("1500ms"), "1500ms");
        assert_eq!(parsed("1h"), "1h");
    }

    #[test]
    fn a_timeout_prints_in_its_largest_whole_unit() {
        assert_eq!(parsed("120s"), "2min");
        assert_eq!(parsed("60min"), "1h");
    }

    #[test]
    fn a_space_before_the_unit_is_allowed() {
        assert_eq!(parsed("2 min"), "2min");
    }

    #[test]
    fn zero_of_any_unit_is_no_timeout() {
        assert_eq!(StatementTimeout::parse("0s").unwrap(), StatementTimeout::NONE);
        assert_eq!(parsed("0min"), "0s");
    }

    #[test]
    fn a_timeout_without_a_unit_or_with_an_unknown_one_is_refused() {
        for text in ["30", "30sec", "min", "-5s", "1.5s", ""] {
            assert!(StatementTimeout::parse(text).unwrap_err().to_string().contains("\"2min\""), "{text}");
        }
    }

    #[test]
    fn a_timeout_longer_than_postgresql_keeps_is_refused() {
        assert!(StatementTimeout::parse("1000h").unwrap_err().to_string().contains("24 days"));
    }

    #[test]
    fn a_printed_timeout_parses_back() {
        for text in ["0s", "1500ms", "2min"] {
            assert_eq!(parsed(&parsed(text)), text);
        }
    }

    fn timeout(text: &str) -> StatementTimeout {
        StatementTimeout::parse(text).unwrap()
    }

    #[test]
    fn a_stricter_server_timeout_wins_over_the_tier() {
        assert_eq!(TimeoutChoice::Tier(timeout("30s")).in_effect(timeout("5s")), timeout("5s"));
        assert_eq!(TimeoutChoice::Tier(timeout("30s")).in_effect(timeout("2min")), timeout("30s"));
    }

    #[test]
    fn no_timeout_on_one_side_keeps_the_other() {
        assert_eq!(TimeoutChoice::Tier(StatementTimeout::NONE).in_effect(timeout("5s")), timeout("5s"));
        assert_eq!(TimeoutChoice::Tier(timeout("30s")).in_effect(StatementTimeout::NONE), timeout("30s"));
    }

    #[test]
    fn a_timeout_the_connection_chose_wins_over_the_server() {
        assert_eq!(TimeoutChoice::Own(timeout("2min")).in_effect(timeout("5s")), timeout("2min"));
        assert_eq!(TimeoutChoice::Own(StatementTimeout::NONE).in_effect(timeout("5s")), StatementTimeout::NONE);
    }

    #[test]
    fn the_server_reports_its_timeout_in_milliseconds() {
        assert_eq!(StatementTimeout::from_millis(5_000), timeout("5s"));
    }

    #[test]
    fn the_set_statement_takes_milliseconds() {
        assert_eq!(StatementTimeout::parse("2min").unwrap().as_millis(), 120_000);
    }
}
