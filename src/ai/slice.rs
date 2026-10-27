//! The part of a catalog a prompt shows: names and types only, the tables most likely wanted first.
use crate::db::lex::{self, Token};
use crate::db::model::{Catalog, SchemaNode, Table, qualified, sql_name};

/// The shortest instruction word that may name a table or a column, so `a` or `of` pulls in nothing.
const MIN_WORD: usize = 3;

/// Why a table is in the slice, the most likely wanted first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reason {
    NamedByStatement,
    NamedByInstruction,
    ColumnNamedByInstruction,
}

/// One line per table of `catalog` the `statement` names or the `instruction`'s words point to, as
/// `schema.table(column type, ...)`: first the tables the statement names, then those the instruction names, then
/// those holding a column it names.
pub(super) fn slice(catalog: &Catalog, statement: &str, instruction: &str) -> Vec<String> {
    let named = statement_names(statement);
    let asked = instruction_words(instruction);
    let mut kept: Vec<(Reason, &SchemaNode, &Table)> =
        tables(catalog).filter_map(|(node, table)| Some((reason(table, &named, &asked)?, node, table))).collect();
    kept.sort_by_key(|(reason, _, _)| *reason);
    kept.into_iter().map(|(_, node, table)| line(node, table)).collect()
}

fn tables(catalog: &Catalog) -> impl Iterator<Item = (&SchemaNode, &Table)> {
    catalog.schema.schemas.iter().flat_map(|node| node.tables.iter().map(move |table| (node, table)))
}

fn reason(table: &Table, named: &[String], asked: &[String]) -> Option<Reason> {
    let name = table.name.to_lowercase();
    let is_asked = |word: &String| name.starts_with(word.as_str()) || word.starts_with(name.as_str());
    let has_asked_column = table.columns.iter().any(|column| asked.contains(&column.name.to_lowercase()));
    match () {
        () if named.contains(&name) => Some(Reason::NamedByStatement),
        () if asked.iter().any(is_asked) => Some(Reason::NamedByInstruction),
        () if has_asked_column => Some(Reason::ColumnNamedByInstruction),
        () => None,
    }
}

/// The words and quoted names of `statement`, lowercased and unquoted; what its literals hold is no name.
fn statement_names(statement: &str) -> Vec<String> {
    let tokens = lex::tokens(statement);
    let is_name = |text: &str, token: &Token| *token == Token::Word || text.starts_with('"');
    let texts = tokens.iter().map(|placed| (&statement[placed.range.clone()], &placed.token));
    texts.filter(|(text, token)| is_name(text, token)).map(|(text, _)| text.trim_matches('"').to_lowercase()).collect()
}

fn instruction_words(instruction: &str) -> Vec<String> {
    let words = instruction.split(|c: char| !(c.is_alphanumeric() || c == '_'));
    words.filter(|word| word.chars().count() >= MIN_WORD).map(str::to_lowercase).collect()
}

fn line(node: &SchemaNode, table: &Table) -> String {
    let columns: Vec<String> =
        table.columns.iter().map(|column| format!("{} {}", sql_name(&column.name), column.type_name.as_str())).collect();
    format!("{}({})", qualified(&node.name, &table.name), columns.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::shop_catalog;

    fn tables_in(statement: &str, instruction: &str) -> Vec<String> {
        let lines = slice(&shop_catalog(), statement, instruction);
        lines.iter().map(|line| line[..line.find('(').unwrap()].to_owned()).collect()
    }

    #[test]
    fn a_table_shows_its_columns_names_and_types_only() {
        assert_eq!(
            slice(&shop_catalog(), "SELECT * FROM orders", ""),
            ["public.orders(id integer, customer_id integer, total numeric(10,2))"]
        );
    }

    #[test]
    fn the_tables_the_statement_names_come_first() {
        assert_eq!(tables_in("SELECT * FROM billing.invoices", "join the customers"), ["billing.invoices", "public.customers"]);
    }

    #[test]
    fn an_instruction_word_names_a_table_in_the_singular_too() {
        assert_eq!(tables_in("", "the last order"), ["public.orders"]);
    }

    #[test]
    fn an_instruction_word_naming_a_column_brings_its_tables_after_the_named_ones() {
        assert_eq!(tables_in("", "customers by order_id"), ["public.customers", "billing.invoices"]);
    }

    #[test]
    fn a_word_inside_a_literal_names_no_table() {
        assert_eq!(tables_in("SELECT 'orders'", ""), Vec::<String>::new());
    }

    #[test]
    fn a_short_instruction_word_names_nothing() {
        assert_eq!(tables_in("", "an id of it"), Vec::<String>::new());
    }
}
