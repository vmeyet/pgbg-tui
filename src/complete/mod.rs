//! Completion, pure: what the cursor wants from the statement under it, an `Index` of one catalog's names, and the
//! candidates ranked for the ghost and the list.
pub(crate) use context::Ask;
pub(crate) use index::Index;
pub(crate) use rank::{Candidate, Kind};

mod context;
mod index;
mod keyword;
mod rank;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::{Catalog, Schema, SchemaColumn, SchemaNode, Table, TableKind, TypeName};
    use std::time::{Duration, Instant};

    const TABLE_COUNT: usize = 20_000;
    const COLUMNS_PER_TABLE: usize = 10;
    const WORDS: [&str; 10] = ["orders", "customers", "invoices", "payments", "refunds", "carts", "items", "stocks", "shipments", "notes"];
    const COLUMN_WORDS: [&str; 10] =
        ["id", "created_at", "updated_at", "customer_id", "status", "total", "currency", "note", "code", "count"];
    /// A release build takes 0.2 ms at worst (`SELECT c|`, 40 000 columns to weigh); a debug build, which CI runs, ten times that.
    const BUDGET: Duration = if cfg!(debug_assertions) { Duration::from_millis(20) } else { Duration::from_millis(1) };

    fn large_catalog() -> Catalog {
        let column = |name: String| SchemaColumn { name, type_name: TypeName::new("integer"), is_nullable: false, default: None };
        let table = |at: usize| Table {
            name: format!("{}_{at}", WORDS[at % WORDS.len()]),
            kind: TableKind::Table,
            estimated_rows: None,
            columns: COLUMN_WORDS.iter().take(COLUMNS_PER_TABLE).map(|word| column(format!("{word}_{at}"))).collect(),
            indexes: vec![],
        };
        let public = SchemaNode { name: "public".to_owned(), tables: (0..TABLE_COUNT).map(table).collect() };
        Catalog { schema: Schema { schemas: vec![public] }, search_path: vec!["public".to_owned()], is_cut: false }
    }

    fn ghost_took(index: &Index, marked: &str) -> Duration {
        let cursor = marked.find('|').unwrap();
        let text = marked.replace('|', "");
        let started = Instant::now();
        let ghost: Option<Candidate> = Ask::at(&text, cursor).and_then(|ask| index.ghost(&ask));
        let took = started.elapsed();
        assert!(ghost.is_some(), "{marked} offers a ghost");
        took
    }

    fn list_took(index: &Index, marked: &str) -> Duration {
        let cursor = marked.find('|').unwrap();
        let text = marked.replace('|', "");
        let started = Instant::now();
        let listed = Ask::at(&text, cursor).map(|ask| index.candidates(&ask)).unwrap_or_default();
        let took = started.elapsed();
        assert!(!listed.is_empty(), "{marked} lists a name");
        took
    }

    const KEYS: [&str; 5] =
        ["SELECT * FROM o|", "SELECT o.| FROM orders_40 o", "SELECT st| FROM orders_40", "SELECT c|", "SELECT cr| FROM missing"];

    #[test]
    fn one_key_on_twenty_thousand_tables_finds_its_ghost_within_the_budget() {
        let index = Index::new(&large_catalog());
        for marked in KEYS {
            let took = ghost_took(&index, marked);
            assert!(took < BUDGET, "{marked} took {took:?}");
        }
    }

    #[test]
    fn one_key_on_twenty_thousand_tables_fills_its_list_within_the_budget() {
        let index = Index::new(&large_catalog());
        for marked in KEYS {
            let took = list_took(&index, marked);
            assert!(took < BUDGET, "{marked} took {took:?}");
        }
    }
}
