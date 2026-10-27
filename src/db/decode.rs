//! Wire values to `Value`, by column type; a type pgbg does not know decodes as `Value::Unsupported`.
use super::model::{Column, ColumnKind, ColumnName, JsonText, TypeName, Value};
use chrono::{DateTime, Utc};
use serde_json::value::RawValue;
use tokio_postgres::Row;
use tokio_postgres::types::{FromSql, Json, Kind, Type};

const NUMERIC_DIGITS_PER_WORD: usize = 4;
const NUMERIC_NEGATIVE: u16 = 0x4000;
const NUMERIC_NAN: u16 = 0xC000;
const NUMERIC_INFINITY: u16 = 0xD000;
const NUMERIC_NEGATIVE_INFINITY: u16 = 0xF000;

/// A column as `ResultSet` holds it.
pub(super) fn column(column: &tokio_postgres::Column) -> Column {
    Column { name: ColumnName::new(column.name()), kind: column_kind(column.type_()), type_name: TypeName::new(column.type_().name()) }
}

/// Every value of `row`, in column order; a value pgbg cannot decode is `Value::Unsupported`, never an error.
pub(super) fn row(row: &Row) -> Result<Vec<Value>, tokio_postgres::Error> {
    row.columns().iter().enumerate().map(|(index, column)| value(row, index, column.type_())).collect()
}

fn column_kind(ty: &Type) -> ColumnKind {
    kind(base_type(ty))
}

/// The type a domain is built on, through every level; any other type as itself.
fn base_type(ty: &Type) -> &Type {
    match ty.kind() {
        Kind::Domain(base) => base_type(base),
        _ => ty,
    }
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
    let raw: Option<Raw<'_>> = row.try_get(index)?;
    Ok(raw.map_or(Value::Null, |Raw(bytes)| cell(ty, bytes)))
}

/// The value of a non-null `raw` of type `ty`.
fn cell(ty: &Type, raw: &[u8]) -> Value {
    let ty = base_type(ty);
    match kind(ty) {
        ColumnKind::Bool => typed(ty, raw, Value::Bool),
        ColumnKind::Int if *ty == Type::INT2 => typed(ty, raw, |int: i16| Value::Int(int.into())),
        ColumnKind::Int if *ty == Type::INT4 => typed(ty, raw, |int: i32| Value::Int(int.into())),
        ColumnKind::Int if *ty == Type::OID => typed(ty, raw, |oid: u32| Value::Int(oid.into())),
        ColumnKind::Int => typed(ty, raw, Value::Int),
        ColumnKind::Float if *ty == Type::FLOAT4 => typed(ty, raw, real),
        ColumnKind::Float => typed(ty, raw, Value::Float),
        ColumnKind::Numeric => numeric_text(raw).map_or(Value::Unsupported, |text| Value::Numeric(text.into_boxed_str())),
        ColumnKind::Text => Value::Text(String::from_utf8_lossy(raw).into()),
        ColumnKind::Json => typed(ty, raw, json),
        ColumnKind::Uuid => typed(ty, raw, Value::Uuid),
        ColumnKind::Date => date(raw),
        ColumnKind::Time => typed(ty, raw, Value::Time),
        ColumnKind::Timestamp => timestamp(ty, raw, Value::Timestamp),
        ColumnKind::TimestampTz => timestamp(ty, raw, |at: DateTime<Utc>| Value::TimestampTz(at)),
        ColumnKind::Bytes => typed(ty, raw, |bytes: &[u8]| Value::Bytes(bytes.into())),
        ColumnKind::Other if *ty == Type::VOID => Value::Null,
        ColumnKind::Other => Value::Unsupported,
    }
}

/// JSON as the server sent it, compacted; never parsed into a tree.
fn json(Json(sent): Json<&RawValue>) -> Value {
    JsonText::compacted(sent.get()).map_or(Value::Unsupported, Value::Json)
}

/// A `real` widened through its shortest text, so `0.1` stays `0.1` and does not become `0.10000000149011612`.
fn real(float: f32) -> Value {
    Value::Float(float.to_string().parse().unwrap_or(f64::from(float)))
}

fn typed<'a, T: FromSql<'a>>(ty: &Type, raw: &'a [u8], wrap: fn(T) -> Value) -> Value {
    T::from_sql(ty, raw).map_or(Value::Unsupported, wrap)
}

/// The server sends `infinity` and `-infinity` as the largest and smallest day count, which no `NaiveDate` holds.
fn date(raw: &[u8]) -> Value {
    match raw.try_into().map(i32::from_be_bytes) {
        Ok(i32::MAX) => Value::Infinity,
        Ok(i32::MIN) => Value::NegativeInfinity,
        _ => typed(&Type::DATE, raw, Value::Date),
    }
}

/// The server sends `infinity` and `-infinity` as the largest and smallest microsecond count.
fn timestamp<'a, T: FromSql<'a>>(ty: &Type, raw: &'a [u8], wrap: fn(T) -> Value) -> Value {
    match raw.try_into().map(i64::from_be_bytes) {
        Ok(i64::MAX) => Value::Infinity,
        Ok(i64::MIN) => Value::NegativeInfinity,
        _ => typed(ty, raw, wrap),
    }
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

    fn domain(base: Type) -> Type {
        Type::new("amount".into(), 90002, Kind::Domain(base), "shop".into())
    }

    #[test]
    fn infinite_dates_and_timestamps_decode_as_infinity() {
        assert_eq!(cell(&Type::DATE, &i32::MAX.to_be_bytes()), Value::Infinity);
        assert_eq!(cell(&Type::TIMESTAMP, &i64::MAX.to_be_bytes()), Value::Infinity);
        assert_eq!(cell(&Type::TIMESTAMPTZ, &i64::MIN.to_be_bytes()), Value::NegativeInfinity);
    }

    #[test]
    fn a_timestamp_out_of_range_is_unsupported_and_the_row_still_decodes() {
        assert_eq!(cell(&Type::TIMESTAMP, &(i64::MAX - 1).to_be_bytes()), Value::Unsupported);
    }

    #[test]
    fn a_real_keeps_the_digits_the_server_shows() {
        assert_eq!(cell(&Type::FLOAT4, &0.1_f32.to_be_bytes()).to_string(), "0.1");
    }

    #[test]
    fn a_void_value_is_null() {
        assert_eq!(cell(&Type::VOID, &[]), Value::Null);
    }

    #[test]
    fn a_domain_decodes_as_its_base_type() {
        assert_eq!(cell(&domain(Type::INT4), &7_i32.to_be_bytes()), Value::Int(7));
        assert_eq!(column_kind(&domain(Type::INT4)), ColumnKind::Int);
    }

    #[test]
    fn enums_decode_as_text_and_unknown_types_as_other() {
        assert_eq!(kind(&Type::INET), ColumnKind::Other);
        assert_eq!(kind(&Type::new("mood".into(), 90001, Kind::Enum(vec!["calm".into()]), "shop".into())), ColumnKind::Text);
    }
}
