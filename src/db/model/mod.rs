//! The neutral model of what a statement returns: code outside `db/` never sees a driver type.
pub(crate) use cancel::CancelToken;
pub(crate) use error::ServerError;
use serde::Serialize;
pub(crate) use value::Value;

mod cancel;
mod error;
mod value;

/// One SQL statement, as typed: the unit pgbg runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Statement(String);

impl Statement {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a column holds, grouped the way pgbg shows and charts it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ColumnKind {
    Bool,
    Int,
    Float,
    Numeric,
    Text,
    Json,
    Uuid,
    Date,
    Time,
    Timestamp,
    TimestampTz,
    Bytes,
    /// A type pgbg does not decode yet; its values show as `Value::Unsupported`, a `void` one as `Value::Null`.
    Other,
}

/// A column of a result: its name and the kind of its values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Column {
    pub(crate) name: String,
    pub(crate) kind: ColumnKind,
    /// The server's name for the type: `int4`, `timestamptz`, `inet`.
    #[serde(rename = "type")]
    pub(crate) type_name: String,
}

/// The columns and rows a statement returned; each row has one value per column.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct ResultSet {
    pub(crate) columns: Vec<Column>,
    pub(crate) rows: Vec<Vec<Value>>,
}
