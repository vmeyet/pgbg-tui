//! A statement's values as the driver binds them: each in text, so the server reads it with its parameter's own type.
use super::model::Params;
use bytes::BytesMut;
use std::error::Error;
use tokio_postgres::types::{Format, IsNull, ToSql, Type, to_sql_checked};

/// One value in text, whatever the type of its parameter: the server parses it with that type's input, and a value it
/// cannot read fails with its words; `None` is NULL.
#[derive(Debug)]
pub(super) struct AsText<'p>(Option<&'p str>);

impl ToSql for AsText<'_> {
    fn to_sql(&self, _: &Type, out: &mut BytesMut) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        let Some(text) = self.0 else { return Ok(IsNull::Yes) };
        out.extend_from_slice(text.as_bytes());
        Ok(IsNull::No)
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    fn encode_format(&self, _: &Type) -> Format {
        Format::Text
    }

    to_sql_checked!();
}

/// `params` in the order of their `$n`.
pub(super) fn as_text(params: &Params) -> Vec<AsText<'_>> {
    params.values().iter().map(|value| AsText(value.as_deref())).collect()
}

/// `values` as the driver's `bind` takes them.
pub(super) fn borrowed<'v>(values: &'v [AsText<'_>]) -> Vec<&'v (dyn ToSql + Sync)> {
    values.iter().map(|value| value as &(dyn ToSql + Sync)).collect()
}
