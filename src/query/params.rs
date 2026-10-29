//! Parameters: the `{{name}}` of a statement turned into `$1`, `$2`, and the values given by name bound in that order.
use crate::connection::WritePolicy;
use crate::db::guard::{self, Admitted};
use crate::db::lex::{self, Placed, Token};
use crate::db::model::{ParamValue, Params, Statement, TypeName};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;
use std::str::FromStr;

/// The message never quotes the text, which may be a value typed in the wrong place.
const NO_EQUAL_SIGN: &str = "a value is given as `name=value`, and this has no `=`";
/// The message never quotes the line: what a mistyped line holds before its `=` may be a secret.
const NO_ASSIGNMENT_LINE: &str = "a line is `name=value`, the name a letter or `_`, then letters, digits or `_`";

/// The name in a `{{name}}`: a letter or `_`, then letters, digits or `_`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ParamName(String);

/// One `name=value`, from `--param` or a line of `--params-file`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    name: ParamName,
    value: ParamValue,
}

/// What a header's `-- param: since date = 2024-01-01` says of one parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParamSpec {
    pub(crate) name: ParamName,
    /// The type the server looks up, else it infers one from where the parameter stands.
    pub(crate) type_name: Option<TypeName>,
    pub(crate) fallback: Fallback,
}

/// What a parameter left empty takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fallback {
    /// Nothing: the run stops.
    Required,
    /// The value after `=`, as in `= 2024-01-01`.
    Default(ParamValue),
    /// `optional`: NULL.
    Optional,
}

/// The values given for a run, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Given {
    values: BTreeMap<ParamName, ParamValue>,
    /// The names a params file gave: one file may serve several statements, so a name this one lacks is no typo.
    from_file: BTreeSet<ParamName>,
}

/// What fills a statement's parameters: the values given for them, and what its header says of each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ParamInput {
    pub(crate) given: Given,
    pub(crate) specs: Vec<ParamSpec>,
}

/// A statement as the server gets it, each `{{name}}` replaced by its `$n`, and the values bound to them.
#[derive(Debug)]
pub(crate) struct Filled {
    pub(crate) statement: Statement,
    pub(crate) params: Params,
}

/// Where each `{{name}}` of a template stands and the `$n` it becomes, to read a place in the filled text back in the template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Numbering {
    placed: Vec<Numbered>,
}

/// One `{{name}}`: the template's bytes it spans and its `$n`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Numbered {
    range: Range<usize>,
    number: usize,
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
    /// The values typed in the parameter bar, one per name.
    pub(crate) fn typed(values: BTreeMap<ParamName, ParamValue>) -> Self {
        Self { values, from_file: BTreeSet::new() }
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

/// The `name=value` of each line of `text`, blank lines left out; a line that is none fails with its number only.
pub(crate) fn assignments(text: &str) -> Result<Vec<Assignment>, String> {
    let lines = text.lines().enumerate().filter(|(_, line)| !line.trim().is_empty());
    lines.map(|(at, line)| line.parse().map_err(|_quotes_the_line: String| format!("line {}: {NO_ASSIGNMENT_LINE}", at + 1))).collect()
}

/// The names of the `{{name}}` among a statement's `tokens`, each once, in the order they first come.
pub(crate) fn names(tokens: &[Placed]) -> Result<Vec<ParamName>, ParamError> {
    Ok(distinct(&named_in(tokens)?))
}

/// `template` with each `{{name}}` replaced by `$n`, one number per name, and the values to bind.
/// Each value is the one given for its name, else its fallback in `specs`; each `$n` takes the type `specs` names.
pub(crate) fn fill(template: &Statement, given: &Given, specs: &[ParamSpec]) -> Result<Filled, ParamError> {
    let text = template.as_str();
    let placed = named_params(text)?;
    let names = distinct(&placed);
    check_known(given, &names)?;
    let specs: Vec<Option<&ParamSpec>> = names.iter().map(|name| specs.iter().rev().find(|spec| spec.name == *name)).collect();
    let values = names.iter().zip(&specs).map(|(name, spec)| value_of(given, name, *spec)).collect::<Result<_, _>>()?;
    let types = specs.iter().map(|spec| spec.and_then(|spec| spec.type_name.clone())).collect();
    Ok(Filled { statement: Statement::new(Numbering::of(&placed, &names).apply(text)), params: Params::new(values, types) })
}

/// How `fill` numbers `template`'s `{{name}}`; an empty numbering when it cannot read them.
pub(crate) fn numbering(template: &Statement) -> Numbering {
    let placed = named_params(template.as_str()).unwrap_or_default();
    Numbering::of(&placed, &distinct(&placed))
}

/// `template` filled with `given` and `specs`, then cleared for a connection of policy `writes`, or why it is not.
pub(crate) fn admit(template: &Statement, given: &Given, specs: &[ParamSpec], writes: WritePolicy) -> anyhow::Result<Admitted> {
    let Filled { statement, params } = fill(template, given, specs)?;
    Ok(guard::admit(statement, params, writes)?)
}

/// A `{{name}}` and the bytes it spans.
struct NamedParam {
    name: ParamName,
    range: Range<usize>,
}

fn named_params(text: &str) -> Result<Vec<NamedParam>, ParamError> {
    named_in(&lex::tokens(text))
}

fn named_in(tokens: &[Placed]) -> Result<Vec<NamedParam>, ParamError> {
    if tokens.iter().any(|placed| placed.token == Token::Unreadable) {
        return Err(ParamError::Unreadable);
    }
    let is_numbered = tokens.iter().any(|placed| matches!(placed.token, Token::Numbered(_)));
    let named: Vec<NamedParam> = tokens.iter().filter_map(named_param).collect();
    if is_numbered && !named.is_empty() {
        return Err(ParamError::Mixed);
    }
    Ok(named)
}

fn named_param(placed: &Placed) -> Option<NamedParam> {
    match &placed.token {
        Token::Param(name) => Some(NamedParam { name: ParamName(name.clone()), range: placed.range.clone() }),
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

fn value_of(given: &Given, name: &ParamName, spec: Option<&ParamSpec>) -> Result<Option<ParamValue>, ParamError> {
    if let Some(value) = given.values.get(name).filter(|value| !value.as_str().is_empty()) {
        return Ok(Some(value.clone()));
    }
    match spec.map(|spec| &spec.fallback) {
        Some(Fallback::Default(default)) => Ok(Some(default.clone())),
        Some(Fallback::Optional) => Ok(None),
        Some(Fallback::Required) | None => Err(ParamError::Missing(name.clone())),
    }
}

impl Numbering {
    fn of(placed: &[NamedParam], names: &[ParamName]) -> Self {
        let number = |param: &NamedParam| names.iter().position(|name| *name == param.name).map_or(0, |at| at + 1);
        Self { placed: placed.iter().map(|param| Numbered { range: param.range.clone(), number: number(param) }).collect() }
    }

    /// `template` with each `{{name}}` replaced by its `$n`.
    fn apply(&self, template: &str) -> String {
        let mut sql = String::with_capacity(template.len());
        let mut from = 0;
        for param in &self.placed {
            sql.push_str(&template[from..param.range.start]);
            sql.push_str(&param.dollar());
            from = param.range.end;
        }
        sql + &template[from..]
    }

    /// The byte of `template` at character `filled` of its filled text, from 0: a `$n` points at its `{{name}}`, the end
    /// of the filled text at the end of the template.
    pub(crate) fn template_byte(&self, template: &str, filled: usize) -> Option<usize> {
        let mut left = filled;
        let mut from = 0;
        for param in &self.placed {
            let between = &template[from..param.range.start];
            let between_chars = between.chars().count();
            if left < between_chars {
                return byte_of_char(between, left).map(|at| from + at);
            }
            left -= between_chars;
            let dollar_chars = param.dollar().len();
            if left < dollar_chars {
                return Some(param.range.start);
            }
            left -= dollar_chars;
            from = param.range.end;
        }
        byte_of_char(&template[from..], left).map(|at| from + at)
    }
}

impl Numbered {
    fn dollar(&self) -> String {
        format!("${}", self.number)
    }
}

/// Where character `nth` of `text` starts, in bytes, counting from 0; the length of `text` just past the last one.
fn byte_of_char(text: &str, nth: usize) -> Option<usize> {
    text.char_indices().map(|(at, _)| at).chain(std::iter::once(text.len())).nth(nth)
}

impl ParamName {
    /// A name for a value of the column `column`: its letters and digits in lower case, the rest `_`, then `_2`, `_3`
    /// past the names `taken`.
    pub(crate) fn for_column(column: &str, taken: &[Self]) -> Self {
        let lowered: String = column.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
        let base = match lowered.trim_matches('_') {
            "" => "value".to_owned(),
            digit_first if digit_first.starts_with(|c: char| c.is_ascii_digit()) => format!("_{digit_first}"),
            plain => plain.to_owned(),
        };
        let numbered = (2..).map(|number| Self(format!("{base}_{number}")));
        std::iter::once(Self(base.clone())).chain(numbered).find(|name| !taken.contains(name)).unwrap_or(Self(base))
    }
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

impl FromStr for ParamSpec {
    type Err = String;

    /// `since date = 2024-01-01`, `owner text optional`, `limit`: a name, then a type, then a default or `optional`.
    fn from_str(text: &str) -> Result<Self, String> {
        let (declared, fallback) = match text.split_once('=') {
            Some((declared, default)) => (declared, default_of(default)?),
            None => match text.trim_end().strip_suffix(OPTIONAL) {
                Some(declared) if declared.ends_with(char::is_whitespace) || declared.is_empty() => (declared, Fallback::Optional),
                _ => (text, Fallback::Required),
            },
        };
        let (name, type_name) = declared.trim().split_once(char::is_whitespace).unwrap_or((declared.trim(), ""));
        let type_name = Some(type_name.trim()).filter(|type_name| !type_name.is_empty()).map(TypeName::new);
        Ok(Self { name: name.parse()?, type_name, fallback })
    }
}

const OPTIONAL: &str = "optional";

fn default_of(text: &str) -> Result<Fallback, String> {
    let default = text.trim();
    if default.is_empty() { Err("a default goes after `=`".to_owned()) } else { Ok(Fallback::Default(ParamValue::new(default))) }
}

impl FromStr for Assignment {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let (name, value) = text.split_once('=').ok_or_else(|| NO_EQUAL_SIGN.to_owned())?;
        Ok(Self { name: name.trim().parse()?, value: ParamValue::new(value) })
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
        let Filled { statement, params } = fill(&Statement::new(sql), &given(pairs), &[])?;
        Ok((
            statement.as_str().to_owned(),
            params.values().iter().map(|value| value.as_ref().map(|value| value.as_str().to_owned())).collect(),
        ))
    }

    fn name(name: &str) -> ParamName {
        name.parse().unwrap()
    }

    #[test]
    fn a_columns_parameter_reads_its_name_in_lower_case_without_what_a_name_cannot_hold() {
        assert_eq!(
            ["Total Amount", "?column?", "2024", "Σ"].map(|column| ParamName::for_column(column, &[])),
            [name("total_amount"), name("column"), name("_2024"), name("value")]
        );
    }

    #[test]
    fn a_columns_parameter_takes_a_number_past_the_names_taken() {
        assert_eq!(ParamName::for_column("customer", &[name("customer"), name("customer_2")]), name("customer_3"));
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
    fn names_come_once_each_in_the_order_they_first_come() {
        let names = names(&lex::tokens("SELECT {{owner}}, {{since}}, {{owner}}")).unwrap();
        assert_eq!(names, [name("owner"), name("since")]);
    }

    #[test]
    fn a_place_on_a_dollar_number_points_at_its_name_in_the_template() {
        let template = "SELECT {{since}}, {{owner}}";
        assert_eq!(numbering(&Statement::new(template)).template_byte(template, 12), Some(18));
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
        let Filled { statement, params } = fill(&Statement::new("SELECT {{since}}, {{owner}}"), &given, &[]).unwrap();
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

    #[test]
    fn a_params_file_line_with_a_bad_name_says_nothing_of_its_text() {
        let err = assignments("since=1\nhunter2 secret=x\n").unwrap_err();
        assert!(err.starts_with("line 2:") && !err.contains("hunter2"), "{err}");
    }

    fn spec(text: &str) -> ParamSpec {
        text.parse().unwrap()
    }

    #[test]
    fn a_spec_names_a_type_and_a_default() {
        let since = spec("since date = 2024-01-01");
        assert_eq!((since.type_name, since.fallback), (Some(TypeName::new("date")), Fallback::Default(ParamValue::new("2024-01-01"))));
    }

    #[test]
    fn a_spec_type_may_take_several_words() {
        assert_eq!(spec("at timestamp with time zone optional").type_name, Some(TypeName::new("timestamp with time zone")));
    }

    #[test]
    fn a_spec_may_say_only_its_name_or_that_it_is_optional() {
        assert_eq!(spec("limit"), ParamSpec { name: name("limit"), type_name: None, fallback: Fallback::Required });
        assert_eq!(spec("owner optional").fallback, Fallback::Optional);
        assert_eq!(spec("owner text").fallback, Fallback::Required);
    }

    #[test]
    fn a_spec_with_an_empty_default_or_a_bad_name_is_refused() {
        assert!("since date =".parse::<ParamSpec>().is_err());
        assert!("1st date".parse::<ParamSpec>().is_err());
    }

    fn filled_with(sql: &str, pairs: &[&str], specs: &[&str]) -> Result<Params, ParamError> {
        let specs: Vec<ParamSpec> = specs.iter().map(|text| spec(text)).collect();
        fill(&Statement::new(sql), &given(pairs), &specs).map(|filled| filled.params)
    }

    #[test]
    fn a_name_left_empty_takes_its_default_and_an_optional_one_binds_null() {
        let params = filled_with("SELECT {{since}}, {{owner}}", &["since="], &["since date = 2024-01-01", "owner text optional"]).unwrap();
        assert_eq!(params.values(), [Some(ParamValue::new("2024-01-01")), None]);
    }

    #[test]
    fn a_value_given_wins_over_the_default() {
        let params = filled_with("SELECT {{since}}", &["since=2025-06-01"], &["since date = 2024-01-01"]).unwrap();
        assert_eq!(params.values(), [Some(ParamValue::new("2025-06-01"))]);
    }

    #[test]
    fn each_parameter_takes_the_type_its_spec_names() {
        let params = filled_with("SELECT {{owner}}, {{since}}", &["owner=nina", "since=1"], &["since date"]).unwrap();
        assert_eq!(params.types(), [None, Some(TypeName::new("date"))]);
    }

    #[test]
    fn a_required_spec_still_needs_a_value() {
        assert_eq!(filled_with("SELECT {{since}}", &[], &["since date"]), Err(ParamError::Missing(name("since"))));
    }
}
