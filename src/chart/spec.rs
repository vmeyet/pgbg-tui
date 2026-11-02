use super::{ChartKind, Orientation};
use crate::db::model::ColumnName;
use std::fmt;
use std::str::FromStr;

/// The older word for `grouped horizontal`, which opens its bars on their side unless the header says otherwise.
const BAR: &str = "bar";

/// What to chart: the kind, which way its bars run, the column on x and the columns on y.
/// No orientation means upright.
/// A missing x or an empty y is filled from the result when the series is built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChartSpec {
    pub(crate) kind: ChartKind,
    pub(crate) orientation: Option<Orientation>,
    pub(crate) x: Option<ColumnName>,
    pub(crate) y: Vec<ColumnName>,
}

/// Why the text of a `-- chart:` header is not a chart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChartSpecError {
    NoKind,
    UnknownKind(String),
    /// A word that is not `x=`, `y=` or an orientation.
    UnknownPart(String),
    /// `x=` or `y=` given twice.
    Repeated(&'static str),
    /// `horizontal` or `vertical` given twice.
    RepeatedOrientation,
    /// `horizontal` or `vertical` on a kind that draws one way.
    NoOrientation(&'static str),
    /// `x=` with no name, or an empty name in the `y=` list.
    EmptyColumn,
}

impl fmt::Display for ChartSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kinds = std::iter::once(BAR).chain(ChartKind::ALL.map(ChartKind::name)).collect::<Vec<_>>().join(", ");
        match self {
            Self::NoKind => write!(f, "a chart starts with its kind: {kinds}"),
            Self::UnknownKind(kind) => write!(f, "`{kind}` is not a chart kind: {kinds}"),
            Self::UnknownPart(part) => {
                write!(f, "`{part}` is none of `x=<column>`, `y=<column>,<column>`, `horizontal`, `vertical`")
            }
            Self::Repeated(part) => write!(f, "`{part}=` is given twice"),
            Self::RepeatedOrientation => f.write_str("use one of `horizontal`, `vertical`, once"),
            Self::NoOrientation(kind) => write!(f, "`{kind}` draws one way: it takes no `horizontal` or `vertical`"),
            Self::EmptyColumn => f.write_str("a column name is empty"),
        }
    }
}

impl std::error::Error for ChartSpecError {}

/// `grouped x=day y=orders,refunds horizontal`: the kind first, then `x=`, `y=` and the orientation in any order, each optional.
impl FromStr for ChartSpec {
    type Err = ChartSpecError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut words = text.split_whitespace();
        let (kind, opening) = parse_kind(words.next().ok_or(ChartSpecError::NoKind)?)?;
        let spec = words.try_fold(Self { kind, orientation: None, x: None, y: Vec::new() }, with_part)?;
        Ok(Self { orientation: spec.orientation.or(opening), ..spec })
    }
}

/// The kind a word names, and the orientation that word opens with when the header gives none.
fn parse_kind(word: &str) -> Result<(ChartKind, Option<Orientation>), ChartSpecError> {
    if word == BAR {
        return Ok((ChartKind::Grouped, Some(Orientation::Horizontal)));
    }
    let kind = ChartKind::ALL.into_iter().find(|kind| kind.name() == word);
    kind.map(|kind| (kind, None)).ok_or_else(|| ChartSpecError::UnknownKind(word.to_owned()))
}

fn with_part(spec: ChartSpec, word: &str) -> Result<ChartSpec, ChartSpecError> {
    if let Some(orientation) = Orientation::named(word) {
        return with_orientation(spec, orientation);
    }
    match word.split_once('=') {
        Some(("x", _)) if spec.x.is_some() => Err(ChartSpecError::Repeated("x")),
        Some(("x", name)) => Ok(ChartSpec { x: Some(column_name(name)?), ..spec }),
        Some(("y", _)) if !spec.y.is_empty() => Err(ChartSpecError::Repeated("y")),
        Some(("y", names)) => Ok(ChartSpec { y: names.split(',').map(column_name).collect::<Result<_, _>>()?, ..spec }),
        _ => Err(ChartSpecError::UnknownPart(word.to_owned())),
    }
}

fn with_orientation(spec: ChartSpec, orientation: Orientation) -> Result<ChartSpec, ChartSpecError> {
    if !spec.kind.has_orientation() {
        return Err(ChartSpecError::NoOrientation(spec.kind.name()));
    }
    if spec.orientation.is_some() {
        return Err(ChartSpecError::RepeatedOrientation);
    }
    Ok(ChartSpec { orientation: Some(orientation), ..spec })
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
        let spec = ChartSpec {
            kind: ChartKind::Grouped,
            orientation: Some(Orientation::Horizontal),
            x: Some(ColumnName::new("day")),
            y: vec![ColumnName::new("orders")],
        };
        assert_eq!(parse("bar x=day y=orders"), Ok(spec));
    }

    #[test]
    fn y_takes_several_columns_separated_by_commas() {
        assert_eq!(parse("line y=orders,refunds").unwrap().y, [ColumnName::new("orders"), ColumnName::new("refunds")]);
    }

    #[test]
    fn a_kind_alone_leaves_x_and_y_to_the_result() {
        assert_eq!(parse("line"), Ok(ChartSpec { kind: ChartKind::Line, orientation: None, x: None, y: vec![] }));
    }

    #[test]
    fn x_and_y_come_in_any_order_and_spacing() {
        assert_eq!(parse("  bar   y=total x=customer "), parse("bar x=customer y=total"));
    }

    #[test]
    fn grouped_stands_upright_unless_told_otherwise() {
        let orientation = |text| parse(text).unwrap().orientation;
        assert_eq!(parse("grouped").unwrap().kind, ChartKind::Grouped);
        assert_eq!(orientation("grouped x=day y=orders,refunds"), None);
        assert_eq!(orientation("grouped horizontal x=day y=orders,refunds"), Some(Orientation::Horizontal));
    }

    #[test]
    fn bar_is_grouped_on_its_side_and_can_be_told_otherwise() {
        assert_eq!(parse("bar y=orders"), parse("grouped horizontal y=orders"));
        assert_eq!(parse("bar vertical y=orders"), parse("grouped vertical y=orders"));
    }

    #[test]
    fn a_line_takes_no_orientation() {
        assert_eq!(parse("line horizontal"), Err(ChartSpecError::NoOrientation("line")));
    }

    #[test]
    fn an_orientation_given_twice_is_refused() {
        assert_eq!(parse("grouped horizontal vertical"), Err(ChartSpecError::RepeatedOrientation));
    }

    #[test]
    fn a_word_that_is_no_part_says_what_a_part_can_be() {
        let refused = parse("grouped sideways").unwrap_err().to_string();
        assert_eq!(refused, "`sideways` is none of `x=<column>`, `y=<column>,<column>`, `horizontal`, `vertical`");
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
        assert_eq!(ChartSpecError::UnknownKind("pie".into()).to_string(), "`pie` is not a chart kind: bar, grouped, line");
    }
}
