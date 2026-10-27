//! Parameters: the `{{name}}` of a statement turned into `$1`, `$2`, and the values given by name bound in that order.
use crate::connection::WritePolicy;
use crate::db::guard::{self, Admitted};
use crate::db::lex::{self, Token};
use crate::db::model::{Params, Statement};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;
use std::str::FromStr;

/// Says nothing of the text itself, which may be a value typed in the wrong place.
const NO_EQUAL_SIGN: &str = "a value is given as `name=value`, and this has no `=`";

/// The name in a `{{name}}`: a letter or `_`, then letters, digits or `_`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ParamName(String);

/// One `name=value`, from `--param` or a line of `--params-file`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    name: ParamName,
    value: String,
}

/// The values given for a run, by name.
#[derive(Debug, Default)]
pub(crate) struct Given {
    values: BTreeMap<ParamName, String>,
    /// The names a params file gave: one file may serve several statements, so a name this one lacks is no typo.
    from_file: BTreeSet<ParamName>,
}

/// A statement as the server gets it, each `{{name}}` replaced by its `$n`, and the values bound to them.
#[derive(Debug)]
pub(crate) struct Filled {
    pub(crate) statement: Statement,
    pub(crate) params: Params,
}

/// Why a statement's parameters cannot be filled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParamError {
    /// A `{{` that opens no `{{name}}`.
    Unreadable,
    /// The statement numbers its own `$n` and names `{{name}}` too.
    Mixed,
    /// A name the statement holds, given no value or an empty one.
    Missing(ParamName),
    /// A value given for a name the statement does not hold, other than by a params file.
    Unknown(ParamName),
    Twice(ParamName),
}

impl Given {
    /// No values: a statement without parameters runs with them, one with parameters is refused.
    pub(crate) fn none() -> Self {
        Self::default()
    }

    /// The values of `flags` and of a params file's `file`, refused when one name comes twice.
    pub(crate) fn new(flags: Vec<Assignment>, file: Vec<Assignment>) -> Result<Self, ParamError> {
        let from_file = file.iter().map(|assignment| assignment.name.clone()).collect();
        let mut values = BTreeMap::new();
        for Assignment { name, value } in flags.into_iter().chain(file) {
            if values.contains_key(&name) {
                return Err(ParamError::Twice(name));
            }
            values.insert(name, value);
        }
        Ok(Self { values, from_file })
    }
}

/// The `name=value` of each line of `text`, blank lines left out; a line that is none fails with its number.
pub(crate) fn assignments(text: &str) -> Result<Vec<Assignment>, String> {
    let lines = text.lines().enumerate().filter(|(_, line)| !line.trim().is_empty());
    lines.map(|(at, line)| line.parse().map_err(|err| format!("line {}: {err}", at + 1))).collect()
}

/// `template` with each `{{name}}` replaced by `$n`, the same name the same number, and the value given for each.
pub(crate) fn fill(template: &Statement, given: &Given) -> Result<Filled, ParamError> {
    let text = template.as_str();
    let placed = named_params(text)?;
    let names = distinct(&placed);
    check_known(given, &names)?;
    let values = names.iter().map(|name| value_of(given, name)).collect::<Result<_, _>>()?;
    Ok(Filled { statement: Statement::new(numbered(text, &placed, &names)), params: Params::new(values) })
}

/// `template` filled with `given`, then cleared for a connection of policy `writes`, or why it is not.
pub(crate) fn admit(template: &Statement, given: &Given, writes: WritePolicy) -> anyhow::Result<Admitted> {
    let Filled { statement, params } = fill(template, given)?;
    Ok(guard::admit(statement, params, writes)?)
}

/// A `{{name}}` and the bytes it spans.
struct NamedParam {
    name: ParamName,
    range: Range<usize>,
}

fn named_params(text: &str) -> Result<Vec<NamedParam>, ParamError> {
    let tokens = lex::tokens(text);
    if tokens.iter().any(|placed| placed.token == Token::Unreadable) {
        return Err(ParamError::Unreadable);
    }
    let is_numbered = tokens.iter().any(|placed| matches!(placed.token, Token::Numbered(_)));
    let named: Vec<NamedParam> = tokens.into_iter().filter_map(named_param).collect();
    if is_numbered && !named.is_empty() {
        return Err(ParamError::Mixed);
    }
    Ok(named)
}

fn named_param(placed: lex::Placed) -> Option<NamedParam> {
    match placed.token {
        Token::Param(name) => Some(NamedParam { name: ParamName(name), range: placed.range }),
        _ => None,
    }
}

fn distinct(placed: &[NamedParam]) -> Vec<ParamName> {
    let mut names: Vec<ParamName> = vec![];
    for param in placed {
        if !names.contains(&param.name) {
            names.push(param.name.clone());
        }
    }
    names
}

fn check_known(given: &Given, names: &[ParamName]) -> Result<(), ParamError> {
    let is_unknown = |name: &&ParamName| !names.contains(name) && !given.from_file.contains(*name);
    match given.values.keys().find(is_unknown) {
        Some(unknown) => Err(ParamError::Unknown(unknown.clone())),
        None => Ok(()),
    }
}

fn value_of(given: &Given, name: &ParamName) -> Result<Option<String>, ParamError> {
    match given.values.get(name) {
        Some(value) if !value.is_empty() => Ok(Some(value.clone())),
        _ => Err(ParamError::Missing(name.clone())),
    }
}

/// `text` with each of `placed` replaced by `$n`, `n` its name's place in `names` from 1.
fn numbered(text: &str, placed: &[NamedParam], names: &[ParamName]) -> String {
    let mut sql = String::with_capacity(text.len());
    let mut from = 0;
    for param in placed {
        let number = names.iter().position(|name| *name == param.name).map_or(0, |at| at + 1);
        sql.push_str(&text[from..param.range.start]);
        sql.push_str(&format!("${number}"));
        from = param.range.end;
    }
    sql + &text[from..]
}

impl FromStr for ParamName {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, String> {
        if lex::is_param_name(name) {
            Ok(Self(name.to_owned()))
        } else {
            Err(format!("`{name}` is no parameter name: a letter or `_`, then letters, digits or `_`"))
        }
    }
}

impl fmt::Display for ParamName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Assignment {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let (name, value) = text.split_once('=').ok_or_else(|| NO_EQUAL_SIGN.to_owned())?;
        Ok(Self { name: name.trim().parse()?, value: value.to_owned() })
    }
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable => f.write_str("a `{{` here opens no `{{name}}`: a name is a letter or `_`, then letters, digits or `_`"),
            Self::Mixed => f.write_str("this statement numbers its own `$n`: use `{{name}}` or `$n`, not both"),
            Self::Missing(name) => write!(f, "`{{{{{name}}}}}` has no value"),
            Self::Unknown(name) => write!(f, "a value was given for `{name}`, and the statement holds no `{{{{{name}}}}}`"),
            Self::Twice(name) => write!(f, "`{name}` was given twice"),
        }
    }
}

impl std::error::Error for ParamError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(pairs: &[&str]) -> Vec<Assignment> {
        pairs.iter().map(|pair| pair.parse().unwrap()).collect()
    }

    fn given(pairs: &[&str]) -> Given {
        Given::new(parsed(pairs), vec![]).unwrap()
    }

    fn filled(sql: &str, pairs: &[&str]) -> Result<(String, Vec<Option<String>>), ParamError> {
        let Filled { statement, params } = fill(&Statement::new(sql), &given(pairs))?;
        Ok((statement.as_str().to_owned(), params.values().to_vec()))
    }

    fn name(name: &str) -> ParamName {
        name.parse().unwrap()
    }

    #[test]
    fn each_name_becomes_a_numbered_parameter_with_its_value() {
        let (sql, values) =
            filled("SELECT * FROM orders WHERE day > {{since}} AND owner = {{owner}}", &["owner=nina", "since=2024-01-01"]).unwrap();
        assert_eq!(sql, "SELECT * FROM orders WHERE day > $1 AND owner = $2");
        assert_eq!(values, [Some("2024-01-01".to_owned()), Some("nina".to_owned())]);
    }

    #[test]
    fn the_same_name_gets_the_same_number() {
        let (sql, values) = filled("SELECT {{day}}, {{owner}}, {{day}}", &["day=1", "owner=nina"]).unwrap();
        assert_eq!((sql.as_str(), values.len()), ("SELECT $1, $2, $1", 2));
    }

    #[test]
    fn a_statement_without_parameters_stays_as_typed() {
        assert_eq!(filled("SELECT '{{day}}' -- {{day}}", &[]), Ok(("SELECT '{{day}}' -- {{day}}".to_owned(), vec![])));
    }

    #[test]
    fn a_value_keeps_its_spaces_and_equal_signs() {
        assert_eq!(filled("SELECT {{note}}", &["note= a=b "]).unwrap().1, [Some(" a=b ".to_owned())]);
    }

    #[test]
    fn a_name_given_no_value_or_an_empty_one_is_refused() {
        assert_eq!(filled("SELECT {{since}}", &[]), Err(ParamError::Missing(name("since"))));
        assert_eq!(filled("SELECT {{since}}", &["since="]), Err(ParamError::Missing(name("since"))));
    }

    #[test]
    fn a_value_for_a_name_the_statement_lacks_is_refused() {
        assert_eq!(filled("SELECT {{since}}", &["since=1", "sinse=2"]), Err(ParamError::Unknown(name("sinse"))));
    }

    #[test]
    fn a_statement_mixing_its_own_numbers_and_names_is_refused() {
        assert_eq!(filled("SELECT $1, {{since}}", &["since=1"]), Err(ParamError::Mixed));
    }

    #[test]
    fn an_unreadable_brace_pair_is_refused() {
        assert_eq!(filled("SELECT {{since}}, {{ }}", &["since=1"]), Err(ParamError::Unreadable));
    }

    #[test]
    fn a_name_given_twice_is_refused() {
        assert_eq!(Given::new(parsed(&["day=1", "day=2"]), vec![]).unwrap_err(), ParamError::Twice(name("day")));
        assert_eq!(Given::new(parsed(&["day=1"]), parsed(&["day=2"])).unwrap_err(), ParamError::Twice(name("day")));
    }

    #[test]
    fn a_params_file_may_hold_names_the_statement_lacks() {
        let given = Given::new(parsed(&["since=1"]), parsed(&["owner=nina", "region=eu"])).unwrap();
        let Filled { statement, params } = fill(&Statement::new("SELECT {{since}}, {{owner}}"), &given).unwrap();
        assert_eq!((statement.as_str(), params.len()), ("SELECT $1, $2", 2));
    }

    #[test]
    fn an_assignment_needs_an_equal_sign_and_a_name() {
        assert!("since".parse::<Assignment>().is_err());
        assert!("1st=2".parse::<Assignment>().is_err());
    }

    #[test]
    fn a_params_file_takes_one_assignment_a_line_and_skips_blank_ones() {
        let read = assignments("since=2024-01-01\n\nowner=nina\r\n").unwrap();
        assert_eq!(read, ["since=2024-01-01".parse().unwrap(), "owner=nina".parse().unwrap()]);
    }

    #[test]
    fn a_params_file_line_that_is_no_assignment_says_its_number() {
        assert!(assignments("since=1\nowner\n").unwrap_err().starts_with("line 2:"));
    }
}
