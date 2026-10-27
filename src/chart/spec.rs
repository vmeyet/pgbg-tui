use super::ChartKind;
use crate::db::model::ColumnName;
use std::fmt;
use std::str::FromStr;

/// What to chart: the kind, the column on x and the columns on y.
/// A missing x or an empty y is filled from the result when the series is built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChartSpec {
    pub(crate) kind: ChartKind,
    pub(crate) x: Option<ColumnName>,
    pub(crate) y: Vec<ColumnName>,
}

/// Why the text of a `-- chart:` header is not a chart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChartSpecError {
    NoKind,
    UnknownKind(String),
    /// A word that is neither `x=` nor `y=`.
    UnknownPart(String),
    /// `x=` or `y=` given twice.
    Repeated(&'static str),
    /// `x=` with no name, or an empty name in the `y=` list.
    EmptyColumn,
}

impl fmt::Display for ChartSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kinds = ChartKind::ALL.map(ChartKind::name).join(", ");
        match self {
            Self::NoKind => write!(f, "a chart starts with its kind: {kinds}"),
            Self::UnknownKind(kind) => write!(f, "`{kind}` is not a chart kind: {kinds}"),
            Self::UnknownPart(part) => write!(f, "`{part}` is neither `x=<column>` nor `y=<column>,<column>`"),
            Self::Repeated(part) => write!(f, "`{part}=` is given twice"),
            Self::EmptyColumn => f.write_str("a column name is empty"),
        }
    }
}

impl std::error::Error for ChartSpecError {}

/// `bar x=day y=orders,refunds`: the kind first, then `x=` and `y=` in any order, each optional.
impl FromStr for ChartSpec {
    type Err = ChartSpecError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut words = text.split_whitespace();
        let kind = parse_kind(words.next().ok_or(ChartSpecError::NoKind)?)?;
        let mut spec = Self { kind, x: None, y: Vec::new() };
        for word in words {
            spec = with_part(spec, word)?;
        }
        Ok(spec)
    }
}

fn parse_kind(word: &str) -> Result<ChartKind, ChartSpecError> {
    ChartKind::ALL.into_iter().find(|kind| kind.name() == word).ok_or_else(|| ChartSpecError::UnknownKind(word.to_owned()))
}

fn with_part(spec: ChartSpec, word: &str) -> Result<ChartSpec, ChartSpecError> {
    match word.split_once('=') {
        Some(("x", _)) if spec.x.is_some() => Err(ChartSpecError::Repeated("x")),
        Some(("x", name)) => Ok(ChartSpec { x: Some(column_name(name)?), ..spec }),
        Some(("y", _)) if !spec.y.is_empty() => Err(ChartSpecError::Repeated("y")),
        Some(("y", names)) => Ok(ChartSpec { y: names.split(',').map(column_name).collect::<Result<_, _>>()?, ..spec }),
        _ => Err(ChartSpecError::UnknownPart(word.to_owned())),
    }
}

fn column_name(name: &str) -> Result<ColumnName, ChartSpecError> {
    if name.is_empty() { Err(ChartSpecError::EmptyColumn) } else { Ok(ColumnName::new(name)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<ChartSpec, ChartSpecError> {
        text.parse()
    }

    #[test]
    fn a_header_names_the_kind_x_and_y() {
        let spec = ChartSpec { kind: ChartKind::Bar, x: Some(ColumnName::new("day")), y: vec![ColumnName::new("orders")] };
        assert_eq!(parse("bar x=day y=orders"), Ok(spec));
    }

    #[test]
    fn y_takes_several_columns_separated_by_commas() {
        assert_eq!(parse("line y=orders,refunds").unwrap().y, [ColumnName::new("orders"), ColumnName::new("refunds")]);
    }

    #[test]
    fn a_kind_alone_leaves_x_and_y_to_the_result() {
        assert_eq!(parse("line"), Ok(ChartSpec { kind: ChartKind::Line, x: None, y: vec![] }));
    }

    #[test]
    fn x_and_y_come_in_any_order_and_spacing() {
        assert_eq!(parse("  bar   y=total x=customer "), parse("bar x=customer y=total"));
    }

    #[test]
    fn an_empty_header_has_no_kind() {
        assert_eq!(parse("  "), Err(ChartSpecError::NoKind));
    }

    #[test]
    fn an_unknown_kind_is_refused() {
        assert_eq!(parse("pie x=customer"), Err(ChartSpecError::UnknownKind("pie".into())));
    }

    #[test]
    fn a_part_that_is_not_x_or_y_is_refused() {
        assert_eq!(parse("bar z=day"), Err(ChartSpecError::UnknownPart("z=day".into())));
        assert_eq!(parse("bar day"), Err(ChartSpecError::UnknownPart("day".into())));
    }

    #[test]
    fn a_part_given_twice_is_refused() {
        assert_eq!(parse("bar x=day x=week"), Err(ChartSpecError::Repeated("x")));
        assert_eq!(parse("bar y=orders y=refunds"), Err(ChartSpecError::Repeated("y")));
    }

    #[test]
    fn an_empty_column_name_is_refused() {
        assert_eq!(parse("bar x="), Err(ChartSpecError::EmptyColumn));
        assert_eq!(parse("bar y=orders,,refunds"), Err(ChartSpecError::EmptyColumn));
    }

    #[test]
    fn an_unknown_kind_says_which_kinds_exist() {
        assert_eq!(ChartSpecError::UnknownKind("pie".into()).to_string(), "`pie` is not a chart kind: bar, line");
    }
}
