use super::{JsonText, ParamValue};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use serde::{Serialize, Serializer};
use std::cmp::Ordering;
use std::fmt;

/// One cell, decoded once at the boundary.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Kept as its exact decimal text: `NUMERIC` holds more digits than any float.
    Numeric(Box<str>),
    Text(Box<str>),
    Json(JsonText),
    Uuid(uuid::Uuid),
    Date(NaiveDate),
    Time(NaiveTime),
    Timestamp(NaiveDateTime),
    TimestampTz(DateTime<Utc>),
    Bytes(Box<[u8]>),
    /// A date or timestamp of `infinity`.
    Infinity,
    /// A date or timestamp of `-infinity`.
    NegativeInfinity,
    /// A value of a type pgbg does not decode, or out of the range it can hold.
    Unsupported,
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("NULL"),
            Self::Bool(value) => write!(f, "{value}"),
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(value) => match non_finite(*value) {
                Some(spelled) => f.write_str(spelled),
                None => write!(f, "{value}"),
            },
            Self::Numeric(text) | Self::Text(text) => f.write_str(text),
            Self::Json(json) => f.write_str(json.as_str()),
            Self::Uuid(value) => write!(f, "{value}"),
            Self::Date(value) => write!(f, "{value}"),
            Self::Time(value) => write!(f, "{value}"),
            Self::Timestamp(value) => write!(f, "{value}"),
            Self::TimestampTz(value) => write!(f, "{}", value.format("%Y-%m-%d %H:%M:%S%.f%:z")),
            Self::Bytes(bytes) => {
                f.write_str("\\x")?;
                bytes.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
            }
            Self::Infinity => f.write_str("infinity"),
            Self::NegativeInfinity => f.write_str("-infinity"),
            Self::Unsupported => f.write_str("<unsupported>"),
        }
    }
}

impl Value {
    /// The first `chars` characters it prints, formatting no further: a 10 MB JSON value costs no more than a short one.
    pub(crate) fn prefix(&self, chars: usize) -> String {
        prefixed(chars, format_args!("{self}"))
    }

    /// The text a parameter binds it as, for the server to read with the column's type; none for NULL, which no `=`
    /// matches, and for a value pgbg could not decode.
    pub(crate) fn as_param(&self) -> Option<ParamValue> {
        match self {
            Self::Null | Self::Unsupported => None,
            other => Some(ParamValue::new(other.to_string())),
        }
    }

    /// Like [`Value::prefix`], with JSON indented over several lines so a row's detail can show its shape.
    pub(crate) fn pretty_prefix(&self, chars: usize) -> String {
        match self {
            Self::Json(json) => json.pretty_prefix(chars),
            other => other.prefix(chars),
        }
    }

    /// How it sorts against `other`, a value of the same column: numbers as numbers, times as times, text by its
    /// characters, `-infinity` first and `infinity`, then NULL, last.
    pub(crate) fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => a.cmp(b),
            (Self::Numeric(a), Self::Numeric(b)) => Decimal::of(a).cmp(&Decimal::of(b)),
            (Self::Int(_) | Self::Float(_) | Self::Numeric(_), Self::Int(_) | Self::Float(_) | Self::Numeric(_)) => {
                self.number().total_cmp(&other.number())
            }
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            (Self::Json(a), Self::Json(b)) => a.as_str().cmp(b.as_str()),
            (Self::Uuid(a), Self::Uuid(b)) => a.cmp(b),
            (Self::Date(a), Self::Date(b)) => a.cmp(b),
            (Self::Time(a), Self::Time(b)) => a.cmp(b),
            (Self::Timestamp(a), Self::Timestamp(b)) => a.cmp(b),
            (Self::TimestampTz(a), Self::TimestampTz(b)) => a.cmp(b),
            (Self::Bytes(a), Self::Bytes(b)) => a.cmp(b),
            _ => self.rank().cmp(&other.rank()),
        }
    }

    /// A number as a float, so an integer, a float and a `NUMERIC` compare with one another; NaN, as PostgreSQL sorts it,
    /// after every number.
    fn number(&self) -> f64 {
        match self {
            #[expect(clippy::cast_precision_loss, reason = "only an integer and a float of one column compare this way")]
            Self::Int(int) => *int as f64,
            Self::Float(float) => *float,
            Self::Numeric(text) => text.parse().unwrap_or(f64::NAN),
            _ => f64::NAN,
        }
    }

    /// Where a value sorts among values of other kinds.
    const fn rank(&self) -> u8 {
        match self {
            Self::NegativeInfinity => 0,
            Self::Infinity => 2,
            Self::Unsupported => 3,
            Self::Null => 4,
            _ => 1,
        }
    }
}

/// A `NUMERIC`'s decimal text read so two compare digit by digit, as PostgreSQL sorts them: `-Infinity`, the finite
/// values, `Infinity`, then `NaN`.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Decimal<'a> {
    NegativeInfinity,
    Finite(Finite<'a>),
    Infinity,
    NaN,
}

/// A finite decimal as its sign and its digits, without the leading zeros of its integer part or the trailing zeros of
/// its fraction, so one value written two ways reads the same.
#[derive(PartialEq, Eq)]
struct Finite<'a> {
    is_negative: bool,
    integer: &'a str,
    fraction: &'a str,
}

impl<'a> Decimal<'a> {
    fn of(text: &'a str) -> Self {
        match text {
            "NaN" => Self::NaN,
            "Infinity" => Self::Infinity,
            "-Infinity" => Self::NegativeInfinity,
            _ => Self::Finite(Finite::of(text)),
        }
    }
}

impl<'a> Finite<'a> {
    fn of(text: &'a str) -> Self {
        let (has_minus, digits) = text.strip_prefix('-').map_or((false, text), |digits| (true, digits));
        let (integer, fraction) = digits.split_once('.').unwrap_or((digits, ""));
        let (integer, fraction) = (integer.trim_start_matches('0'), fraction.trim_end_matches('0'));
        let is_zero = integer.is_empty() && fraction.is_empty();
        Self { is_negative: has_minus && !is_zero, integer, fraction }
    }

    /// How its distance from zero compares with `other`'s: a longer integer part is the greater, then digit by digit.
    fn compare_size(&self, other: &Self) -> Ordering {
        let integer = self.integer.len().cmp(&other.integer.len()).then_with(|| self.integer.cmp(other.integer));
        integer.then_with(|| self.fraction.cmp(other.fraction))
    }
}

impl Ord for Finite<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.is_negative, other.is_negative) {
            (false, false) => self.compare_size(other),
            (true, true) => other.compare_size(self),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        }
    }
}

impl PartialOrd for Finite<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(super) fn prefixed(chars: usize, formatted: fmt::Arguments<'_>) -> String {
    let mut prefix = Prefix { text: String::new(), room: chars };
    // The only error is `Prefix` running out of room, which is the point.
    _ = fmt::write(&mut prefix, formatted);
    prefix.text
}

/// A `fmt::Write` that keeps `room` characters, then fails so the formatting stops.
struct Prefix {
    text: String,
    room: usize,
}

impl fmt::Write for Prefix {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for c in text.chars() {
            if self.room == 0 {
                return Err(fmt::Error);
            }
            self.text.push(c);
            self.room -= 1;
        }
        Ok(())
    }
}

/// `NaN`, `Infinity` or `-Infinity` as PostgreSQL spells them, for a float that is not finite.
fn non_finite(value: f64) -> Option<&'static str> {
    match value {
        _ if value.is_nan() => Some("NaN"),
        f64::INFINITY => Some("Infinity"),
        f64::NEG_INFINITY => Some("-Infinity"),
        _ => None,
    }
}

/// Numbers, booleans, JSON and null as themselves; every other value as the text it prints.
impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_none(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Int(value) => serializer.serialize_i64(*value),
            Self::Float(value) => match non_finite(*value) {
                Some(spelled) => serializer.serialize_str(spelled),
                None => serializer.serialize_f64(*value),
            },
            Self::Json(json) => json.serialize(serializer),
            other => serializer.collect_str(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_holds_no_more_than_its_widest_inline_value_a_uuid() {
        assert!(size_of::<Value>() <= 24, "{} bytes", size_of::<Value>());
    }

    #[test]
    fn a_null_binds_no_parameter_and_a_number_binds_its_text() {
        assert_eq!((Value::Null.as_param(), Value::Int(7).as_param()), (None, Some(ParamValue::new("7"))));
    }

    #[test]
    fn json_prints_compact_with_its_keys_in_order() {
        let json = Value::Json(JsonText::of(&serde_json::json!({"paid": true, "id": 7})));
        assert_eq!(json.to_string(), r#"{"id":7,"paid":true}"#);
    }

    #[test]
    fn a_prefix_stops_after_its_characters() {
        assert_eq!(Value::Text("Zoë Nakamura".into()).prefix(3), "Zoë");
        assert_eq!(Value::Bytes([0xde, 0xad, 0x01].into()).prefix(4), "\\xde");
    }

    #[test]
    fn a_prefix_of_a_short_value_is_all_of_it() {
        assert_eq!(Value::Json(JsonText::of(&serde_json::json!([1, 2]))).prefix(40), "[1,2]");
    }

    #[test]
    fn a_pretty_prefix_indents_json_and_stops_after_its_characters() {
        let json = Value::Json(JsonText::of(&serde_json::json!({"paid": true})));
        assert_eq!(json.pretty_prefix(40), "{\n  \"paid\": true\n}");
        assert_eq!(json.pretty_prefix(3), "{\n ");
    }

    #[test]
    fn a_pretty_prefix_of_text_is_its_prefix() {
        assert_eq!(Value::Text("nina\nnoel".into()).pretty_prefix(40), "nina\nnoel");
    }

    #[test]
    fn bytes_print_as_postgres_hex() {
        assert_eq!(Value::Bytes([0xde, 0xad, 0x01].into()).to_string(), "\\xdead01");
    }

    #[test]
    fn a_timestamp_with_time_zone_prints_its_offset() {
        let at = DateTime::parse_from_rfc3339("2026-10-26T09:30:00Z").unwrap().to_utc();
        assert_eq!(Value::TimestampTz(at).to_string(), "2026-10-26 09:30:00+00:00");
    }

    #[test]
    fn non_finite_floats_print_and_serialize_as_postgres_spells_them() {
        let floats = [Value::Float(f64::NAN), Value::Float(f64::INFINITY), Value::Float(f64::NEG_INFINITY), Value::Float(1.5)];
        assert_eq!(floats.iter().map(Value::to_string).collect::<Vec<_>>(), ["NaN", "Infinity", "-Infinity", "1.5"]);
        assert_eq!(serde_json::to_string(&floats).unwrap(), r#"["NaN","Infinity","-Infinity",1.5]"#);
    }

    #[test]
    fn infinity_prints_and_serializes_as_postgres_spells_it() {
        assert_eq!(Value::NegativeInfinity.to_string(), "-infinity");
        assert_eq!(serde_json::to_string(&Value::Infinity).unwrap(), r#""infinity""#);
    }

    #[test]
    fn json_keeps_numbers_and_nulls_and_prints_the_rest_as_text() {
        let row =
            [Value::Int(3), Value::Null, Value::Numeric("12.50".into()), Value::Json(JsonText::of(&serde_json::json!({"paid": true})))];
        assert_eq!(serde_json::to_string(&row).unwrap(), r#"[3,null,"12.50",{"paid":true}]"#);
    }

    fn sorted(mut values: Vec<Value>) -> Vec<Value> {
        values.sort_by(Value::compare);
        values
    }

    #[test]
    fn numbers_compare_as_numbers_not_as_text() {
        let numerics = ["9.5", "10", "-2"].map(|text| Value::Numeric(text.into()));
        assert_eq!(sorted(numerics.to_vec()), ["-2", "9.5", "10"].map(|text| Value::Numeric(text.into())));
    }

    fn numerics(texts: &[&str]) -> Vec<Value> {
        texts.iter().map(|&text| Value::Numeric(text.into())).collect()
    }

    #[test]
    fn numerics_past_a_floats_digits_still_compare_apart() {
        let [a, b] = [Value::Numeric("12345678901234567890.1".into()), Value::Numeric("12345678901234567890.2".into())];
        assert_eq!(a.compare(&b), Ordering::Less);
    }

    #[test]
    fn numerics_compare_by_their_decimal_value_signs_and_specials_included() {
        let unsorted = numerics(&["NaN", "-9.5", "0.07", "Infinity", "-10", "100", "-Infinity", "0", "99.99"]);
        assert_eq!(sorted(unsorted), numerics(&["-Infinity", "-10", "-9.5", "0", "0.07", "99.99", "100", "Infinity", "NaN"]));
    }

    #[test]
    fn numerics_of_one_value_written_two_ways_are_equal() {
        assert_eq!(Value::Numeric("1.50".into()).compare(&Value::Numeric("01.5".into())), Ordering::Equal);
    }

    #[test]
    fn dates_compare_as_dates_with_infinity_at_either_end() {
        let day = |text: &str| Value::Date(text.parse().unwrap());
        let days = vec![Value::Infinity, day("2026-10-02"), Value::NegativeInfinity, day("2025-12-31")];
        assert_eq!(sorted(days), [Value::NegativeInfinity, day("2025-12-31"), day("2026-10-02"), Value::Infinity]);
    }

    #[test]
    fn null_compares_after_every_value() {
        assert_eq!(sorted(vec![Value::Null, Value::Infinity, Value::Int(3)]), [Value::Int(3), Value::Infinity, Value::Null]);
    }
}
