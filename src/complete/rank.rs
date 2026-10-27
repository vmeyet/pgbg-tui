//! The candidates for an ask, best first: vtui ranks each list of names, and this merges the lists, a prefix before a
//! fuzzy match, a column of a table the statement names before any other name, and among prefixes the shorter first.
use super::context::{Ask, Want};
use super::index::{Index, ranked};
use std::collections::HashSet;
use vtui::complete;

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

/// The name the ghost offers: what shows after the cursor, and what taking it puts in place of what was typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ghosted {
    pub(crate) rest: String,
    pub(crate) insert: String,
}

/// Names of one kind an ask draws from, and whether they are columns of a table the statement names.
#[derive(Clone, Copy)]
struct Source<'a> {
    names: &'a complete::Index,
    kind: Kind,
    is_in_scope: bool,
}

/// A name of one source that matches what was typed.
struct Match {
    name: String,
    kind: Kind,
    is_in_scope: bool,
    is_fuzzy: bool,
}

impl Index {
    /// The best name that starts with what `ask` typed, for the ghost; none when nothing was typed, unless after a `.`,
    /// where it is the table's first column.
    pub(crate) fn ghost(&self, ask: &Ask) -> Option<Ghosted> {
        let is_after_dot = matches!(ask.want, Want::ColumnOf { .. });
        if ask.prefix.is_empty() && !is_after_dot {
            return None;
        }
        let sources = self.sources(&ask.want);
        let best = |is_in_scope: bool| {
            let tier = sources.iter().filter(|source| source.is_in_scope == is_in_scope);
            let rests = tier.filter_map(|source| Some((source.kind, source.rest_after(&ask.prefix)?)));
            rests.min_by_key(|(_, rest)| rest.chars().count())
        };
        let (kind, rest) = best(true).or_else(|| best(false))?;
        let insert = self.candidate(kind, &format!("{}{rest}", ask.prefix), &ask.prefix).insert;
        let rest = if kind == Kind::Keyword { cased_as(&rest, &ask.prefix) } else { rest };
        Some(Ghosted { rest, insert })
    }

    /// The best names for `ask`, at most `MAX_LISTED`, each name once: a column of a table the statement names wins over the
    /// same name among every column.
    pub(crate) fn candidates(&self, ask: &Ask) -> Vec<Candidate> {
        let mut seen = HashSet::new();
        let found = self.sources(&ask.want).into_iter().flat_map(|source| source.matches(&ask.prefix));
        let mut found: Vec<Match> = found.filter(|found| seen.insert((found.kind, found.name.to_lowercase()))).collect();
        found.sort_by_key(Match::rank);
        found.into_iter().take(MAX_LISTED).map(|found| self.candidate(found.kind, &found.name, &ask.prefix)).collect()
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

    fn candidate(&self, kind: Kind, name: &str, typed: &str) -> Candidate {
        if kind == Kind::Keyword {
            let name = cased_as(name, typed);
            return Candidate { insert: name.clone(), name, kind };
        }
        Candidate { name: name.to_owned(), insert: self.insert(name), kind }
    }
}

impl Source<'_> {
    /// What the ghost shows after `typed`: the rest of the best name it starts, or the first name when nothing was typed.
    fn rest_after(&self, typed: &str) -> Option<String> {
        if typed.is_empty() { self.names.cycle("").map(|cycle| cycle.current().to_owned()) } else { self.names.ghost(typed) }
    }

    fn matches(self, typed: &str) -> impl Iterator<Item = Match> {
        let key = typed.to_lowercase();
        ranked(self.names, typed).into_iter().map(move |name| Match {
            is_fuzzy: !name.to_lowercase().starts_with(&key),
            name,
            kind: self.kind,
            is_in_scope: self.is_in_scope,
        })
    }
}

impl Match {
    /// Its place across the lists: each list keeps vtui's order, and a prefix match of one list takes the shorter
    /// name's place among the prefix matches of the others.
    fn rank(&self) -> (bool, bool, usize) {
        let prefix_length = if self.is_fuzzy { 0 } else { self.name.chars().count() };
        (self.is_fuzzy, !self.is_in_scope, prefix_length)
    }
}

/// A keyword in lower case once what was typed of it holds a lower-case letter.
fn cased_as(keyword: &str, typed: &str) -> String {
    if typed.chars().any(char::is_lowercase) { keyword.to_lowercase() } else { keyword.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::lex::Lexed;
    use crate::db::model::Catalog;
    use crate::fixtures::{shop_catalog, shop_schema};

    fn index() -> Index {
        Index::new(&shop_catalog())
    }

    fn ask(marked: &str) -> Ask {
        let cursor = marked.find('|').unwrap();
        let text = marked.replace('|', "");
        Ask::at(&text, &Lexed::of(&text), cursor).unwrap()
    }

    fn ghost(marked: &str) -> Option<String> {
        index().ghost(&ask(marked)).map(|ghosted| ghosted.rest)
    }

    fn listed(marked: &str) -> Vec<String> {
        index().candidates(&ask(marked)).into_iter().map(|candidate| candidate.name).collect()
    }

    #[test]
    fn the_ghost_is_the_shortest_name_the_typed_text_starts() {
        assert_eq!(ghost("SELECT * FROM c|").as_deref(), Some("ustomers"));
        assert_eq!(ghost("SELECT o|").as_deref(), Some("n"));
    }

    #[test]
    fn a_column_of_a_named_table_beats_a_shorter_name() {
        assert_eq!(ghost("SELECT c| FROM orders").as_deref(), Some("ustomer_id"));
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
        assert_eq!(ghost("SEL|").as_deref(), Some("ECT"));
        assert_eq!(ghost("sel|").as_deref(), Some("ect"));
    }

    #[test]
    fn the_ghost_shows_the_rest_of_its_name_after_what_was_typed() {
        assert_eq!(ghost("SELECT * FROM CUS|").as_deref(), Some("tomers"));
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
        assert_eq!(index.ghost(&ask("SELECT * FROM or|")).map(|ghosted| ghosted.insert).as_deref(), Some("\"Orders\""));
    }
}
