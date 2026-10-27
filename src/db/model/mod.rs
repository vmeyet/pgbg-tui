//! The neutral model of what a statement returns: code outside `db/` never sees a driver type.
pub(crate) use cancel::CancelToken;
pub(crate) use error::ServerError;
pub(crate) use result::{Extent, Page, ResultSet};
pub(crate) use route::{LocalPort, Route};
pub(crate) use schema::{Catalog, Index, Schema, SchemaColumn, SchemaNode, Table, TableKind};
use serde::Serialize;
pub(crate) use server::{Named, RoleFlag, RolePowers, RoleRights, ServerFacts};
pub(crate) use value::Value;

mod cancel;
mod error;
mod result;
mod route;
#[cfg_attr(not(test), expect(dead_code, reason = "`Schema::matching` serves only tests since the read filters in its SQL"))]
mod schema;
mod server;
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

/// A type as a statement's header names it (`date`, `timestamp with time zone`, `shop.status`): the server looks it
/// up, the name bound, never put in SQL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TypeName(String);

impl TypeName {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The values bound to a statement's `$1`, `$2`…, in order: each sent as text, for the server to read with the type it
/// gives the parameter, unless its type is named; `None` binds NULL.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Params {
    values: Vec<Option<String>>,
    /// The type of each `$n`, `None` for the server to infer; shorter than `values` when the last ones have none.
    types: Vec<Option<TypeName>>,
}

impl Params {
    /// `values`, each `$n` of the type at its place in `types`.
    pub(crate) fn new(values: Vec<Option<String>>, types: Vec<Option<TypeName>>) -> Self {
        Self { values, types }
    }

    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(crate) fn values(&self) -> &[Option<String>] {
        &self.values
    }

    pub(crate) fn types(&self) -> &[Option<TypeName>] {
        &self.types
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
