//! Wire values to `Value`, by column type; a type pgbg does not know decodes as `Value::Unsupported`.
use super::model::{Column, ColumnKind, Value};
use chrono::{DateTime, Utc};
use tokio_postgres::Row;
use tokio_postgres::types::{FromSql, Kind, Type};

const NUMERIC_DIGITS_PER_WORD: usize = 4;
const NUMERIC_NEGATIVE: u16 = 0x4000;
const NUMERIC_NAN: u16 = 0xC000;
const NUMERIC_INFINITY: u16 = 0xD000;
const NUMERIC_NEGATIVE_INFINITY: u16 = 0xF000;

/// A column as `ResultSet` holds it.
pub(super) fn column(column: &tokio_postgres::Column) -> Column {
    Column { name: column.name().to_owned(), kind: kind(column.type_()), type_name: column.type_().name().to_owned() }
}

/// Every value of `row`, in column order.
pub(super) fn row(row: &Row) -> Result<Vec<Value>, tokio_postgres::Error> {
    row.columns().iter().enumerate().map(|(index, column)| value(row, index, column.type_())).collect()
}

fn kind(ty: &Type) -> ColumnKind {
    match *ty {
        Type::BOOL => ColumnKind::Bool,
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID => ColumnKind::Int,
        Type::FLOAT4 | Type::FLOAT8 => ColumnKind::Float,
        Type::NUMERIC => ColumnKind::Numeric,
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => ColumnKind::Text,
        Type::JSON | Type::JSONB => ColumnKind::Json,
        Type::UUID => ColumnKind::Uuid,
        Type::DATE => ColumnKind::Date,
        Type::TIME => ColumnKind::Time,
        Type::TIMESTAMP => ColumnKind::Timestamp,
        Type::TIMESTAMPTZ => ColumnKind::TimestampTz,
        Type::BYTEA => ColumnKind::Bytes,
        _ if matches!(ty.kind(), Kind::Enum(_)) || ty.name() == "citext" => ColumnKind::Text,
        _ => ColumnKind::Other,
    }
}

fn value(row: &Row, index: usize, ty: &Type) -> Result<Value, tokio_postgres::Error> {
    match kind(ty) {
        ColumnKind::Bool => get(row, index, Value::Bool),
        ColumnKind::Int if *ty == Type::INT2 => get(row, index, |int: i16| Value::Int(int.into())),
        ColumnKind::Int if *ty == Type::INT4 => get(row, index, |int: i32| Value::Int(int.into())),
        ColumnKind::Int if *ty == Type::OID => get(row, index, |oid: u32| Value::Int(oid.into())),
        ColumnKind::Int => get(row, index, Value::Int),
        ColumnKind::Float if *ty == Type::FLOAT4 => get(row, index, |float: f32| Value::Float(float.into())),
        ColumnKind::Float => get(row, index, Value::Float),
        ColumnKind::Numeric => get(row, index, |Raw(bytes)| numeric_text(bytes).map_or(Value::Unsupported, Value::Numeric)),
        ColumnKind::Text => get(row, index, |Raw(bytes)| Value::Text(String::from_utf8_lossy(bytes).into_owned())),
        ColumnKind::Json => get(row, index, Value::Json),
        ColumnKind::Uuid => get(row, index, Value::Uuid),
        ColumnKind::Date => get(row, index, Value::Date),
        ColumnKind::Time => get(row, index, Value::Time),
        ColumnKind::Timestamp => get(row, index, Value::Timestamp),
        ColumnKind::TimestampTz => get(row, index, |at: DateTime<Utc>| Value::TimestampTz(at)),
        ColumnKind::Bytes => get(row, index, Value::Bytes),
        ColumnKind::Other => get(row, index, |Raw(_)| Value::Unsupported),
    }
}

fn get<'a, T: FromSql<'a>>(row: &'a Row, index: usize, wrap: fn(T) -> Value) -> Result<Value, tokio_postgres::Error> {
    Ok(row.try_get::<_, Option<T>>(index)?.map_or(Value::Null, wrap))
}

/// The bytes of any value as the server sent them.
struct Raw<'a>(&'a [u8]);

impl<'a> FromSql<'a> for Raw<'a> {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Self(raw))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

/// The decimal text of a binary `NUMERIC`: a count, a weight, a sign and a scale, then base-10000
/// digits, the first of which is worth 10000 to the power of the weight.
fn numeric_text(raw: &[u8]) -> Option<String> {
    let words: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair)).collect();
    let [count, weight, sign, scale, digits @ ..] = words.as_slice() else { return None };
    if usize::from(*count) != digits.len() {
        return None;
    }
    let special = match *sign {
        NUMERIC_NAN => Some("NaN"),
        NUMERIC_INFINITY => Some("Infinity"),
        NUMERIC_NEGATIVE_INFINITY => Some("-Infinity"),
        _ => None,
    };
    if let Some(special) = special {
        return Some(special.to_owned());
    }
    let weight = i32::from(*weight as i16);
    let digit = |at: i32| usize::try_from(at).ok().and_then(|at| digits.get(at)).copied().unwrap_or(0);
    let whole: String = if weight < 0 {
        "0".to_owned()
    } else {
        (0..=weight).map(|at| if at == 0 { digit(at).to_string() } else { format!("{:04}", digit(at)) }).collect()
    };
    let scale = usize::from(*scale);
    let fraction_words = scale.div_ceil(NUMERIC_DIGITS_PER_WORD) as i32;
    let mut fraction: String = (weight + 1..=weight + fraction_words).map(|at| format!("{:04}", digit(at))).collect();
    fraction.truncate(scale);
    let minus = if *sign == NUMERIC_NEGATIVE { "-" } else { "" };
    Some(if scale == 0 { format!("{minus}{whole}") } else { format!("{minus}{whole}.{fraction}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numeric(words: &[u16]) -> Option<String> {
        numeric_text(&words.iter().flat_map(|word| word.to_be_bytes()).collect::<Vec<u8>>())
    }

    #[test]
    fn a_numeric_keeps_every_digit_and_its_scale() {
        assert_eq!(numeric(&[3, 1, 0, 2, 1, 2345, 6700]).as_deref(), Some("12345.67"));
    }

    #[test]
    fn a_numeric_below_one_keeps_its_leading_zeros() {
        assert_eq!(numeric(&[1, 0xFFFE, 0, 6, 1200]).as_deref(), Some("0.000012"));
    }

    #[test]
    fn a_negative_whole_numeric_prints_its_sign() {
        assert_eq!(numeric(&[1, 0, NUMERIC_NEGATIVE, 0, 42]).as_deref(), Some("-42"));
        assert_eq!(numeric(&[0, 0, 0, 0]).as_deref(), Some("0"));
    }

    #[test]
    fn a_numeric_of_trailing_zero_words_keeps_them() {
        assert_eq!(numeric(&[1, 2, 0, 0, 3]).as_deref(), Some("300000000"));
    }

    #[test]
    fn nan_and_infinities_print_as_postgres_spells_them() {
        assert_eq!(numeric(&[0, 0, NUMERIC_NAN, 0]).as_deref(), Some("NaN"));
        assert_eq!(numeric(&[0, 0, NUMERIC_NEGATIVE_INFINITY, 0]).as_deref(), Some("-Infinity"));
    }

    #[test]
    fn a_truncated_numeric_is_none() {
        assert_eq!(numeric(&[2, 0, 0, 0, 1]), None);
    }

    #[test]
    fn enums_decode_as_text_and_unknown_types_as_other() {
        assert_eq!(kind(&Type::INET), ColumnKind::Other);
        assert_eq!(kind(&Type::new("mood".into(), 90001, Kind::Enum(vec!["calm".into()]), "shop".into())), ColumnKind::Text);
    }
}
