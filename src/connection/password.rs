use crate::secret::SecretSource;
use serde::Deserialize;
use serde::de::{Deserializer, Error, MapAccess, Visitor, value::MapAccessDeserializer};
use std::fmt;

/// The `password` key as written: a source, or a plain password, which is refused once parsed so
/// that no parse error ever quotes it.
pub(super) enum PasswordEntry {
    Plain,
    Source(SecretSource),
}

impl<'de> Deserialize<'de> for PasswordEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(PasswordVisitor)
    }
}

struct PasswordVisitor;

impl<'de> Visitor<'de> for PasswordVisitor {
    type Value = PasswordEntry;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("{ keychain = \"...\" }, { env = \"...\" } or { command = [...] }")
    }

    fn visit_str<E: Error>(self, _plain: &str) -> Result<Self::Value, E> {
        Ok(PasswordEntry::Plain)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        let source = SecretSource::deserialize(MapAccessDeserializer::new(map))?;
        match source.problem() {
            Some(problem) => Err(A::Error::custom(problem)),
            None => Ok(PasswordEntry::Source(source)),
        }
    }
}
