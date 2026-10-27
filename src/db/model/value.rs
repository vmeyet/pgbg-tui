use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use serde::{Serialize, Serializer};
use std::fmt;

/// One cell, decoded once at the boundary.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Kept as its exact decimal text: `NUMERIC` holds more digits than any float.
    Numeric(String),
    Text(String),
    Json(serde_json::Value),
    Uuid(uuid::Uuid),
    Date(NaiveDate),
    Time(NaiveTime),
    Timestamp(NaiveDateTime),
    TimestampTz(DateTime<Utc>),
    Bytes(Vec<u8>),
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
            Self::Json(value) => write!(f, "{value}"),
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

    /// Like [`Value::prefix`], with JSON indented over several lines so a row's detail can show its shape.
    pub(crate) fn pretty_prefix(&self, chars: usize) -> String {
        match self {
            Self::Json(json) => prefixed(chars, format_args!("{json:#}")),
            other => other.prefix(chars),
        }
    }
}

fn prefixed(chars: usize, formatted: fmt::Arguments<'_>) -> String {
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
            Self::Json(value) => value.serialize(serializer),
            other => serializer.collect_str(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_stops_after_its_characters() {
        assert_eq!(Value::Text("Zoë Nakamura".into()).prefix(3), "Zoë");
        assert_eq!(Value::Bytes(vec![0xde, 0xad, 0x01]).prefix(4), "\\xde");
    }

    #[test]
    fn a_prefix_of_a_short_value_is_all_of_it() {
        assert_eq!(Value::Json(serde_json::json!([1, 2])).prefix(40), "[1,2]");
    }

    #[test]
    fn a_pretty_prefix_indents_json_and_stops_after_its_characters() {
        let json = Value::Json(serde_json::json!({"paid": true}));
        assert_eq!(json.pretty_prefix(40), "{\n  \"paid\": true\n}");
        assert_eq!(json.pretty_prefix(3), "{\n ");
    }

    #[test]
    fn a_pretty_prefix_of_text_is_its_prefix() {
        assert_eq!(Value::Text("nina\nnoel".into()).pretty_prefix(40), "nina\nnoel");
    }

    #[test]
    fn bytes_print_as_postgres_hex() {
        assert_eq!(Value::Bytes(vec![0xde, 0xad, 0x01]).to_string(), "\\xdead01");
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
        let row = [Value::Int(3), Value::Null, Value::Numeric("12.50".into()), Value::Json(serde_json::json!({"paid": true}))];
        assert_eq!(serde_json::to_string(&row).unwrap(), r#"[3,null,"12.50",{"paid":true}]"#);
    }
}
