//! The candidates for an ask, best first: a prefix before a fuzzy match, a column of a table the statement names
//! before any other name, then shorter first.
use super::context::{Ask, Want};
use super::index::{Index, Name, starting};
use std::cmp::Ordering;
use std::collections::HashSet;

/// How many candidates the list keeps.
const MAX_LISTED: usize = 50;

/// What a candidate names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Table,
    Column,
    Keyword,
}

/// A name offered at the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// As shown: the catalog's name, or a keyword in the case typed.
    pub(crate) name: String,
    /// As put in the editor: quoted when the name needs it.
    pub(crate) insert: String,
    pub(crate) kind: Kind,
}

/// Names of one kind an ask draws from, and whether they are columns of a table the statement names.
struct Source<'a> {
    names: &'a [Name],
    kind: Kind,
    is_in_scope: bool,
}

/// A name that matches what was typed, and what ranks it.
struct Match<'a> {
    is_fuzzy: bool,
    is_in_scope: bool,
    kind: Kind,
    name: &'a Name,
}

impl Candidate {
    /// What the ghost shows after the cursor: the name past the characters of `typed`.
    pub(crate) fn rest_after(&self, typed: &str) -> &str {
        let at = self.name.char_indices().nth(typed.chars().count()).map_or(self.name.len(), |(at, _)| at);
        &self.name[at..]
    }
}

impl Index {
    /// The best name that starts with what `ask` typed, for the ghost; none when nothing was typed, unless after a `.`.
    pub(crate) fn ghost(&self, ask: &Ask) -> Option<Candidate> {
        let is_after_dot = matches!(ask.want, Want::ColumnOf { .. });
        if ask.prefix.is_empty() && !is_after_dot {
            return None;
        }
        let key = ask.prefix.to_lowercase();
        let sources = self.sources(&ask.want);
        let best = |is_in_scope: bool| {
            let tier = sources.iter().filter(|source| source.is_in_scope == is_in_scope);
            tier.flat_map(|source| starting(source.names, &key).iter().map(|name| source.matched(name, false))).min_by(Match::compare)
        };
        best(true).or_else(|| best(false)).map(|best| best.candidate(&ask.prefix))
    }

    /// The best names for `ask`, prefix and fuzzy matches together, at most 50.
    pub(crate) fn candidates(&self, ask: &Ask) -> Vec<Candidate> {
        let key = ask.prefix.to_lowercase();
        let sources = self.sources(&ask.want);
        let mut seen = HashSet::new();
        let matches = sources.iter().flat_map(|source| source.matching(&key));
        let matches = matches.filter(|found| seen.insert((found.kind, found.name.key.as_str()))).collect();
        best_of(matches).iter().map(|found| found.candidate(&ask.prefix)).collect()
    }

    fn sources(&self, want: &Want) -> Vec<Source<'_>> {
        let source = |names, kind, is_in_scope| Source { names, kind, is_in_scope };
        match want {
            Want::Keyword => vec![source(&self.keywords, Kind::Keyword, false)],
            Want::Table => vec![source(&self.tables, Kind::Table, false)],
            Want::ColumnOf { table } => self.columns_of(table).map(|names| source(names, Kind::Column, true)).into_iter().collect(),
            Want::Column { tables } => {
                let named = tables.iter().filter_map(|table| self.columns_of(table)).map(|names| source(names, Kind::Column, true));
                let others = [source(&self.keywords, Kind::Keyword, false), source(&self.columns, Kind::Column, false)];
                named.chain(others).collect()
            }
        }
    }
}

impl<'a> Source<'a> {
    fn matched(&self, name: &'a Name, is_fuzzy: bool) -> Match<'a> {
        Match { is_fuzzy, is_in_scope: self.is_in_scope, kind: self.kind, name }
    }

    fn matching(&self, key: &str) -> impl Iterator<Item = Match<'a>> {
        self.names.iter().filter_map(move |name| match (name.key.starts_with(key), is_subsequence(key, &name.key)) {
            (true, _) => Some(self.matched(name, false)),
            (false, true) => Some(self.matched(name, true)),
            (false, false) => None,
        })
    }
}

impl Match<'_> {
    fn compare(&self, other: &Self) -> Ordering {
        let rank = |found: &Self| (found.is_fuzzy, !found.is_in_scope, found.name.key.len(), found.name.key.as_str());
        rank(self).cmp(&rank(other))
    }

    fn candidate(&self, typed: &str) -> Candidate {
        if self.kind == Kind::Keyword {
            let name = cased_as(&self.name.text, typed);
            return Candidate { insert: name.clone(), name, kind: self.kind };
        }
        let insert = self.name.quoted.as_deref().unwrap_or(&self.name.text).to_owned();
        Candidate { name: self.name.text.clone(), insert, kind: self.kind }
    }
}

/// The first 50 of `matches` in rank order, without sorting the rest.
fn best_of(mut matches: Vec<Match<'_>>) -> Vec<Match<'_>> {
    if matches.len() > MAX_LISTED {
        matches.select_nth_unstable_by(MAX_LISTED - 1, Match::compare);
        matches.truncate(MAX_LISTED);
    }
    matches.sort_unstable_by(Match::compare);
    matches
}

/// A keyword in lower case once what was typed of it holds a lower-case letter.
fn cased_as(keyword: &str, typed: &str) -> String {
    if typed.chars().any(char::is_lowercase) { keyword.to_lowercase() } else { keyword.to_owned() }
}

/// Whether every character of `query` comes in `key`, in order.
fn is_subsequence(query: &str, key: &str) -> bool {
    let mut chars = key.chars();
    query.chars().all(|wanted| chars.any(|c| c == wanted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::Catalog;
    use crate::fixtures::{shop_catalog, shop_schema};

    fn index() -> Index {
        Index::new(&shop_catalog())
    }

    fn ask(marked: &str) -> Ask {
        let cursor = marked.find('|').unwrap();
        Ask::at(&marked.replace('|', ""), cursor).unwrap()
    }

    fn ghost(marked: &str) -> Option<String> {
        index().ghost(&ask(marked)).map(|candidate| candidate.name)
    }

    fn listed(marked: &str) -> Vec<String> {
        index().candidates(&ask(marked)).into_iter().map(|candidate| candidate.name).collect()
    }

    #[test]
    fn the_ghost_is_the_shortest_name_the_typed_text_starts() {
        assert_eq!(ghost("SELECT * FROM c|").as_deref(), Some("customers"));
        assert_eq!(ghost("SELECT o|").as_deref(), Some("on"));
    }

    #[test]
    fn a_column_of_a_named_table_beats_a_shorter_name() {
        assert_eq!(ghost("SELECT c| FROM orders").as_deref(), Some("customer_id"));
    }

    #[test]
    fn after_a_dot_the_ghost_offers_a_column_of_that_table_with_nothing_typed() {
        assert_eq!(ghost("SELECT o.| FROM orders o").as_deref(), Some("id"));
        assert_eq!(ghost("SELECT * FROM orders WHERE |"), None);
    }

    #[test]
    fn the_ghost_never_offers_a_fuzzy_match() {
        assert_eq!(ghost("SELECT * FROM cstm|"), None);
    }

    #[test]
    fn a_keyword_takes_the_case_it_is_typed_in() {
        assert_eq!(ghost("SEL|").as_deref(), Some("SELECT"));
        assert_eq!(ghost("sel|").as_deref(), Some("select"));
    }

    #[test]
    fn the_ghost_shows_the_rest_of_its_name_after_what_was_typed() {
        let ghost = index().ghost(&ask("SELECT * FROM CUS|")).unwrap();
        assert_eq!(ghost.rest_after("CUS"), "tomers");
    }

    #[test]
    fn the_list_puts_prefix_matches_before_fuzzy_ones() {
        assert_eq!(listed("SELECT * FROM o|"), ["orders", "customers", "billing.invoices"]);
    }

    #[test]
    fn the_list_puts_columns_of_named_tables_first_and_each_name_once() {
        assert_eq!(listed("SELECT i| FROM customers")[..3], ["id", "in", "is"]);
    }

    #[test]
    fn the_list_keeps_fifty_candidates_at_most() {
        assert_eq!(listed("SELECT |").len(), MAX_LISTED);
    }

    #[test]
    fn a_name_that_needs_quotes_goes_in_quoted() {
        let mut schema = shop_schema();
        schema.schemas[1].tables[0].name = "Orders".to_owned();
        let index = Index::new(&Catalog { schema, search_path: vec!["public".to_owned()], is_cut: false });
        assert_eq!(index.ghost(&ask("SELECT * FROM or|")).map(|candidate| candidate.insert).as_deref(), Some("\"Orders\""));
    }
}
