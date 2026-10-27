//! The header of a saved query: its leading `--` lines, `-- name:`, `-- connection:`, `-- chart:` and `-- format:`.
use crate::chart::ChartSpec;
use crate::connection::ConnectionName;
use crate::format::{self, ColumnFormat};
use crate::render;

/// What the header says. A query may come from a repository nobody checked, so a line it cannot read is a warning,
/// never a failure, and every text kept is printable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) name: Option<String>,
    /// The connection the query was written for; only a suggestion, the query runs on the session on screen.
    pub(crate) connection: Option<ConnectionName>,
    pub(crate) chart: Option<ChartSpec>,
    /// The format each column it names opens in.
    pub(crate) formats: Vec<ColumnFormat>,
    pub(crate) warnings: Vec<String>,
}

impl Header {
    /// The header of `text`: every `--` line before the first other one; a comment that is no `key:` line is left alone.
    pub(crate) fn parse(text: &str) -> Self {
        let comments = text.lines().map_while(|line| line.trim_start().strip_prefix("--"));
        comments.filter_map(key_line).fold(Self::default(), Self::with)
    }

    fn with(self, (key, value): (&str, &str)) -> Self {
        match key {
            "name" => Self { name: Some(render::printable(value).into_owned()), ..self },
            "connection" => match value.parse() {
                Ok(connection) => Self { connection: Some(connection), ..self },
                Err(err) => self.warned(&format!("{err:#}")),
            },
            "chart" => match value.parse() {
                Ok(chart) => Self { chart: Some(chart), ..self },
                Err(err) => self.warned(&format!("chart: {err}")),
            },
            "format" => match format::parse_column_formats(value) {
                Ok(formats) => Self { formats, ..self },
                Err(err) => self.warned(&format!("format: {err}")),
            },
            unknown => self.warned(&format!("unknown header key `{unknown}`")),
        }
    }

    fn warned(mut self, warning: &str) -> Self {
        self.warnings.push(render::printable(warning).into_owned());
        self
    }
}

/// `name: daily orders` as its key and value, when the key is one word.
fn key_line(comment: &str) -> Option<(&str, &str)> {
    let (key, value) = comment.split_once(':')?;
    let key = key.trim();
    let is_word = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    is_word.then(|| (key, value.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::ChartKind;
    use crate::format::Format;

    #[test]
    fn a_header_sets_the_name_the_connection_and_the_chart() {
        let header = Header::parse("-- name: daily orders\n-- connection: shop\n-- chart: bar x=day y=orders\nSELECT 1");
        assert_eq!(header.name.as_deref(), Some("daily orders"));
        assert_eq!(header.connection.map(|name| name.to_string()).as_deref(), Some("shop"));
        assert_eq!(header.chart.map(|chart| chart.kind), Some(ChartKind::Bar));
        assert_eq!(header.warnings, Vec::<String>::new());
    }

    #[test]
    fn a_header_names_the_format_of_its_columns() {
        let header = Header::parse("-- format: created_at=relative size=bytes\nSELECT 1");
        let formats: Vec<(&str, Format)> = header.formats.iter().map(|named| (named.column.as_str(), named.format)).collect();
        assert_eq!(formats, [("created_at", Format::Relative), ("size", Format::Bytes)]);
    }

    #[test]
    fn a_format_header_that_does_not_parse_is_a_warning() {
        let header = Header::parse("-- format: created_at=pretty\nSELECT 1");
        assert_eq!((header.formats, header.warnings.len()), (vec![], 1));
    }

    #[test]
    fn a_file_without_a_header_has_an_empty_one() {
        assert_eq!(Header::parse("SELECT * FROM orders"), Header::default());
    }

    #[test]
    fn the_header_ends_at_the_first_line_that_is_no_comment() {
        assert_eq!(Header::parse("SELECT 1;\n-- name: late").name, None);
    }

    #[test]
    fn an_unknown_key_or_a_bad_value_is_a_warning() {
        let header = Header::parse("-- owner: nina\n-- chart: pie\n-- connection: shop prod\n-- name: kept");
        assert_eq!(header.name.as_deref(), Some("kept"));
        assert_eq!(header.warnings.len(), 3);
        assert!(header.warnings[0].contains("`owner`"));
    }

    #[test]
    fn a_comment_in_plain_words_is_no_key() {
        assert_eq!(Header::parse("-- orders per day, see: the dashboard\nSELECT 1"), Header::default());
    }

    #[test]
    fn a_name_cannot_send_the_terminal_a_command() {
        assert_eq!(Header::parse("-- name: daily\u{1b}[2J").name.as_deref(), Some("daily\\u{1b}[2J"));
    }
}
