//! A name as a statement must write it: bare when PostgreSQL reads it back the same, else in double quotes.
use std::borrow::Cow;

/// The keywords PostgreSQL reserves, which a table or a schema can only be named by in quotes.
const RESERVED: &str = "all analyse analyze and any array as asc asymmetric authorization binary both case cast check collate \
    collation column concurrently constraint create cross current_catalog current_date current_role current_schema \
    current_time current_timestamp current_user default deferrable desc distinct do else end except false fetch \
    for foreign freeze from full grant group having ilike in initially inner intersect into is isnull join lateral \
    leading left like limit localtime localtimestamp natural not notnull null offset on only or order outer \
    overlaps placing primary references returning right select session_user similar some symmetric system_user \
    table tablesample then to trailing true union unique user using";

fn is_reserved(name: &str) -> bool {
    RESERVED.split_whitespace().any(|word| word == name)
}

/// Whether PostgreSQL reads `name` back as written only between double quotes: an upper-case letter, a sign, a space
/// or a reserved keyword.
pub(crate) fn needs_quotes(name: &str) -> bool {
    let is_plain = name.chars().next().is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$');
    !is_plain || is_reserved(name)
}

/// `name` bare when PostgreSQL reads it back the same, else quoted with its quotes doubled.
pub(crate) fn sql_name(name: &str) -> Cow<'_, str> {
    if needs_quotes(name) { Cow::Owned(format!("\"{}\"", name.replace('"', "\"\""))) } else { Cow::Borrowed(name) }
}

/// `schema.table`, each part as a statement must write it.
pub(crate) fn qualified(schema: &str, table: &str) -> String {
    format!("{}.{}", sql_name(schema), sql_name(table))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lowercase_name_stays_bare() {
        assert_eq!(qualified("public", "order_items2"), "public.order_items2");
    }

    #[test]
    fn a_name_with_capitals_or_spaces_is_quoted() {
        assert_eq!(qualified("Billing", "late invoices"), "\"Billing\".\"late invoices\"");
    }

    #[test]
    fn a_reserved_keyword_is_quoted() {
        assert_eq!(qualified("public", "order"), "public.\"order\"");
    }

    #[test]
    fn a_keyword_postgresql_takes_as_a_table_name_stays_bare() {
        assert_eq!((sql_name("values"), sql_name("user")), ("values".into(), "\"user\"".into()));
    }

    #[test]
    fn a_quote_in_a_name_is_doubled() {
        assert_eq!(sql_name("nina\"s"), "\"nina\"\"s\"");
    }
}
