//! A statement's values as the driver binds them: each in text, so the server reads it with its parameter's own type.
use super::guard::Admitted;
use super::model::{ParamValue, Params, TypeName, UnknownType};
use super::server_error;
use anyhow::Result;
use bytes::BytesMut;
use std::error::Error;
use tokio_postgres::types::{Format, IsNull, Kind, ToSql, Type, to_sql_checked};
use tokio_postgres::{GenericClient, Statement as Prepared};

/// The type's id, NULL when the server knows no such type; the name is bound, so it cannot change the query.
const TYPE_ID: &str = "SELECT to_regtype($1)::oid";

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
    params.values().iter().map(|value| AsText(value.as_ref().map(ParamValue::as_str))).collect()
}

/// `values` as the driver's `bind` takes them.
pub(super) fn borrowed<'v>(values: &'v [AsText<'_>]) -> Vec<&'v (dyn ToSql + Sync)> {
    values.iter().map(|value| value as &(dyn ToSql + Sync)).collect()
}

/// `admitted` prepared, each `$n` of the type its header names, the others of no type (id 0) for the server to infer.
pub(super) async fn prepare(client: &impl GenericClient, admitted: &Admitted) -> Result<Prepared> {
    let mut types = Vec::with_capacity(admitted.params.types().len());
    for type_name in admitted.params.types() {
        types.push(match type_name {
            Some(name) => looked_up(client, name).await?,
            None => inferred(),
        });
    }
    client.prepare_typed(admitted.statement.as_str(), &types).await.map_err(server_error)
}

/// The server's type named `name`; one it does not know stops the run.
async fn looked_up(client: &impl GenericClient, name: &TypeName) -> Result<Type> {
    let row = client.query_one(TYPE_ID, &[&AsText(Some(name.as_str()))]).await.map_err(server_error)?;
    let oid: Option<u32> = row.try_get(0).map_err(server_error)?;
    let oid = oid.ok_or_else(|| UnknownType(name.clone()))?;
    Ok(Type::from_oid(oid).unwrap_or_else(|| Type::new(name.as_str().to_owned(), oid, Kind::Simple, String::new())))
}

/// No type: the server infers it from where the parameter stands.
fn inferred() -> Type {
    Type::new(String::new(), 0, Kind::Simple, String::new())
}
