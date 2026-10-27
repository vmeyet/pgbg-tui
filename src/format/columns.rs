//! The format each column of a result opens in: the one a `-- format:` header names for it, else the config's default.
use super::{Format, UnknownFormat, allowed, readable};
use crate::db::model::{Column, ColumnKind, ColumnName};
use serde::Deserialize;
use std::fmt;
use std::str::FromStr;

/// How every column reads at once: `[display] format` in the config, and what `F` switches between.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    #[default]
    Raw,
    /// Each column in its kind's readable format.
    Readable,
}

impl Mode {
    /// The format a column of `kind` reads in under this mode.
    pub(crate) fn format_of(self, kind: &ColumnKind) -> Format {
        match self {
            Self::Raw => Format::Raw,
            Self::Readable => readable(kind),
        }
    }
}

/// One `column=format` of a `-- format:` header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ColumnFormat {
    pub(crate) column: ColumnName,
    pub(crate) format: Format,
}

/// Why a word of a `-- format:` header is not a `column=format`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ColumnFormatError {
    /// A word without `=`, or with nothing before it.
    NoColumn(String),
    Unknown(UnknownFormat),
}

impl fmt::Display for ColumnFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoColumn(word) => write!(f, "`{word}` is not `<column>=<format>`"),
            Self::Unknown(unknown) => unknown.fmt(f),
        }
    }
}

impl std::error::Error for ColumnFormatError {}

impl FromStr for ColumnFormat {
    type Err = ColumnFormatError;

    fn from_str(word: &str) -> Result<Self, Self::Err> {
        let Some((column, format)) = word.split_once('=').filter(|(column, _)| !column.is_empty()) else {
            return Err(ColumnFormatError::NoColumn(word.to_owned()));
        };
        let format = format.parse().map_err(ColumnFormatError::Unknown)?;
        Ok(Self { column: ColumnName::new(column), format })
    }
}

/// `created_at=relative size=bytes`: each word one column's format.
pub(crate) fn parse_column_formats(text: &str) -> Result<Vec<ColumnFormat>, ColumnFormatError> {
    text.split_whitespace().map(str::parse).collect()
}

/// The format each of `columns` opens in: the last of `named` for its name, whatever its case, when its kind allows it,
/// else the one `mode` gives its kind.
pub(crate) fn opening(columns: &[Column], named: &[ColumnFormat], mode: Mode) -> Vec<Format> {
    columns.iter().map(|column| named_for(column, named).unwrap_or_else(|| mode.format_of(&column.kind))).collect()
}

fn named_for(column: &Column, named: &[ColumnFormat]) -> Option<Format> {
    let mut formats =
        named.iter().filter(|named| named.column.as_str().eq_ignore_ascii_case(column.name.as_str())).map(|named| named.format);
    formats.next_back().filter(|format| allowed(&column.kind).contains(format))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::column;

    fn orders() -> Vec<Column> {
        vec![column("id", ColumnKind::Int), column("created_at", ColumnKind::TimestampTz), column("customer", ColumnKind::Text)]
    }

    fn named(text: &str) -> Vec<ColumnFormat> {
        parse_column_formats(text).unwrap()
    }

    #[test]
    fn columns_open_raw_by_default() {
        assert_eq!(opening(&orders(), &[], Mode::Raw), [Format::Raw, Format::Raw, Format::Raw]);
    }

    #[test]
    fn columns_open_readable_when_the_mode_says_so() {
        assert_eq!(opening(&orders(), &[], Mode::Readable), [Format::Grouped, Format::Local, Format::Raw]);
    }

    #[test]
    fn a_named_format_wins_over_the_mode_whatever_the_case_of_the_name() {
        assert_eq!(opening(&orders(), &named("Created_At=relative"), Mode::Raw), [Format::Raw, Format::Relative, Format::Raw]);
    }

    #[test]
    fn a_named_format_the_kind_does_not_allow_leaves_the_mode() {
        assert_eq!(opening(&orders(), &named("customer=bytes id=short"), Mode::Readable), [Format::Grouped, Format::Local, Format::Raw]);
    }

    #[test]
    fn the_last_format_named_for_a_column_wins() {
        assert_eq!(opening(&orders(), &named("id=bytes id=raw"), Mode::Readable)[0], Format::Raw);
    }

    #[test]
    fn a_word_without_a_column_is_refused() {
        assert_eq!(parse_column_formats("id=raw =bytes"), Err(ColumnFormatError::NoColumn("=bytes".to_owned())));
        assert_eq!(parse_column_formats("relative"), Err(ColumnFormatError::NoColumn("relative".to_owned())));
    }

    #[test]
    fn an_unknown_format_is_refused_saying_which_ones_exist() {
        let error = parse_column_formats("id=pretty").unwrap_err();
        assert!(error.to_string().starts_with("`pretty` is not a format: raw,"), "{error}");
    }
}
