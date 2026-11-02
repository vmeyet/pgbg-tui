//! The header of a saved query: its leading `--` lines, `-- name:`, `-- connection:`, `-- chart:`, `-- format:` and
//! `-- param:`; a statement further down the file may lead with a `-- chart:` of its own.
use super::params::ParamSpec;
use crate::chart::ChartSpec;
use crate::connection::ConnectionName;
use crate::format::{self, ColumnFormat};
use crate::render;

const CHART_KEY: &str = "chart";

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
    /// One per `-- param:` line, in order; a later one for a name wins.
    pub(crate) params: Vec<ParamSpec>,
    pub(crate) warnings: Vec<String>,
}

impl Header {
    /// The header of `text`: every `--` line before the first other one; a comment that is no `key:` line is left alone.
    pub(crate) fn parse(text: &str) -> Self {
        key_lines(text).fold(Self::default(), Self::with)
    }

    /// `answer`, which takes the place of `replaced`, under the `--` lines leading `replaced`, but for the `key:` lines its
    /// own lines set: a prompt holds no comment, so the answer cannot keep them, and a key never ends up twice.
    pub(crate) fn kept_above(replaced: &str, answer: &str) -> String {
        let set: Vec<&str> = key_lines(answer).map(|(key, _)| key).collect();
        let is_replaced = |comment: &str| key_line(comment).is_some_and(|(key, _)| set.contains(&key));
        let leading = replaced.lines().map_while(|line| Some((line, Self::comment_in(line)?)));
        let kept = leading.filter(|(_, comment)| !is_replaced(comment));
        kept.map(|(line, _)| format!("{line}\n")).chain([answer.to_owned()]).collect()
    }

    /// Whether a `key:` line leading `answer` also leads `text`.
    pub(crate) fn shares_a_key(text: &str, answer: &str) -> bool {
        key_lines(answer).any(|(key, _)| key_lines(text).any(|(own, _)| own == key))
    }

    /// What follows the `--` of `line`, when the line is a `--` comment.
    pub(crate) fn comment_in(line: &str) -> Option<&str> {
        line.trim_start().strip_prefix("--")
    }

    /// Whether `comment`, the text of a header line after its `--`, is a `key:` line the header reads, as `name: daily`.
    pub(crate) fn is_key_line(comment: &str) -> bool {
        key_line(comment).is_some()
    }

    /// Whether `comment`, the text of a line after its `--`, is a `chart:` line, which is pgbg's wherever it sits.
    pub(crate) fn is_chart_line(comment: &str) -> bool {
        key_line(comment).is_some_and(|(key, _)| key == CHART_KEY)
    }

    fn with(self, (key, value): (&str, &str)) -> Self {
        match key {
            "name" => Self { name: Some(render::printable(value).into_owned()), ..self },
            "connection" => match value.parse() {
                Ok(connection) => Self { connection: Some(connection), ..self },
                Err(err) => self.warned(&format!("{err:#}")),
            },
            CHART_KEY => match value.parse() {
                Ok(chart) => Self { chart: Some(chart), ..self },
                Err(err) => self.warned(&format!("chart: {err}")),
            },
            "format" => match format::parse_column_formats(value) {
                Ok(formats) => Self { formats, ..self },
                Err(err) => self.warned(&format!("format: {err}")),
            },
            "param" => match value.parse() {
                Ok(param) => Self { params: [self.params, vec![param]].concat(), ..self },
                Err(err) => self.warned(&format!("param: {err}")),
            },
            unknown => self.warned(&format!("unknown header key `{unknown}`")),
        }
    }

    fn warned(mut self, warning: &str) -> Self {
        self.warnings.push(render::printable(warning).into_owned());
        self
    }
}

/// The `-- chart:` leading one statement: below the top of a file, its other comments are the user's words.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StatementChart {
    pub(crate) spec: Option<ChartSpec>,
    /// Why its line cannot be read.
    pub(crate) warning: Option<String>,
}

impl StatementChart {
    pub(crate) fn of(statement: &str) -> Self {
        let read = key_lines(statement).filter(|(key, _)| *key == CHART_KEY).fold(Header::default(), Header::with);
        Self { spec: read.chart, warning: read.warnings.into_iter().next() }
    }
}

/// The key and value of each `key:` line among the leading `--` lines of `text`.
fn key_lines(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().map_while(Header::comment_in).filter_map(key_line)
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
        assert_eq!(header.chart.map(|chart| chart.kind), Some(ChartKind::Grouped));
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
    fn each_param_line_declares_a_parameter() {
        let header = Header::parse("-- param: since date = 2024-01-01\n-- param: owner text optional\n-- param: 1st\nSELECT 1");
        let names: Vec<String> = header.params.iter().map(|param| param.name.to_string()).collect();
        assert_eq!((names, header.warnings.len()), (vec!["since".to_owned(), "owner".to_owned()], 1));
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
    fn a_comment_naming_a_key_is_a_key_line_and_one_in_plain_words_is_not() {
        assert_eq!([" param: since date", " owner: nina", " the open orders"].map(Header::is_key_line), [true, true, false]);
    }

    #[test]
    fn the_chart_of_a_statement_reads_its_chart_and_warns_of_nothing_else() {
        let chart = StatementChart::of("-- see: the docs\n-- chart: line x=week y=total\nSELECT 1");
        assert_eq!((chart.spec.map(|spec| spec.kind), chart.warning), (Some(ChartKind::Line), None));
    }

    #[test]
    fn the_chart_of_a_statement_warns_when_it_cannot_be_read() {
        let chart = StatementChart::of("-- chart: pie\nSELECT 1");
        assert!(chart.spec.is_none() && chart.warning.is_some());
    }

    #[test]
    fn an_answer_keeps_the_comments_above_and_replaces_only_the_key_it_sets() {
        let kept = Header::kept_above("-- name: daily\n-- chart: bar\n-- plain words\nSELECT 1", "-- chart: line\nSELECT 2");
        assert_eq!(kept, "-- name: daily\n-- plain words\n-- chart: line\nSELECT 2");
    }

    #[test]
    fn an_answer_with_no_chart_keeps_the_chart_above_it() {
        assert_eq!(Header::kept_above("-- chart: bar\nSELECT 1", "SELECT 2"), "-- chart: bar\nSELECT 2");
    }

    #[test]
    fn two_texts_share_a_key_when_both_lead_with_it() {
        assert!(Header::shares_a_key("-- name: a\n-- chart: bar\n", "-- chart: line\nSELECT 1"));
        assert!(!Header::shares_a_key("-- name: a\n", "-- chart: line\nSELECT 1"));
    }

    #[test]
    fn a_name_cannot_send_the_terminal_a_command() {
        assert_eq!(Header::parse("-- name: daily\u{1b}[2J").name.as_deref(), Some("daily\\u{1b}[2J"));
    }
}
