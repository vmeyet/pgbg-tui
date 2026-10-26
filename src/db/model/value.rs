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
    Unsupported,
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("NULL"),
            Self::Bool(value) => write!(f, "{value}"),
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(value) => write!(f, "{value}"),
            Self::Numeric(text) | Self::Text(text) => f.write_str(text),
            Self::Json(value) => write!(f, "{value}"),
            Self::Uuid(value) => write!(f, "{value}"),
            Self::Date(value) => write!(f, "{value}"),
            Self::Time(value) => write!(f, "{value}"),
            Self::Timestamp(value) => write!(f, "{value}"),
            Self::TimestampTz(value) => write!(f, "{}", value.format("%Y-%m-%d %H:%M:%S%.f%:z")),
            Self::Bytes(bytes) => write!(f, "\\x{}", bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
            Self::Unsupported => f.write_str("<unsupported>"),
        }
    }
}

/// Numbers, booleans, JSON and null as themselves; every other value as the text it prints.
impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_none(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Int(value) => serializer.serialize_i64(*value),
            Self::Float(value) => serializer.serialize_f64(*value),
            Self::Json(value) => value.serialize(serializer),
            other => serializer.collect_str(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn json_keeps_numbers_and_nulls_and_prints_the_rest_as_text() {
        let row = [Value::Int(3), Value::Null, Value::Numeric("12.50".into()), Value::Json(serde_json::json!({"paid": true}))];
        assert_eq!(serde_json::to_string(&row).unwrap(), r#"[3,null,"12.50",{"paid":true}]"#);
    }
}
