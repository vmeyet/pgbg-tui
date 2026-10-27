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
    use super::rank::Ghosted;
    use super::*;
    use crate::db::lex::Lexed;
    use crate::db::model::{Catalog, Schema, SchemaColumn, SchemaNode, Table, TableKind, TypeName};

    const TABLE_COUNT: usize = 20_000;
    const COLUMNS_PER_TABLE: usize = 10;
    const WORDS: [&str; 10] = ["orders", "customers", "invoices", "payments", "refunds", "carts", "items", "stocks", "shipments", "notes"];
    const COLUMN_WORDS: [&str; 10] =
        ["id", "created_at", "updated_at", "customer_id", "status", "total", "currency", "note", "code", "count"];
    const KEYS: [&str; 5] =
        ["SELECT * FROM o|", "SELECT o.| FROM orders_40 o", "SELECT st| FROM orders_40", "SELECT c|", "SELECT cr| FROM missing"];

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

    fn ask(marked: &str) -> Option<Ask> {
        let cursor = marked.find('|').unwrap();
        let text = marked.replace('|', "");
        Ask::at(&text, &Lexed::of(&text), cursor)
    }

    fn ghost(index: &Index, marked: &str) -> Option<Ghosted> {
        ask(marked).and_then(|ask| index.ghost(&ask))
    }

    fn listed(index: &Index, marked: &str) -> Vec<Candidate> {
        ask(marked).map(|ask| index.candidates(&ask)).unwrap_or_default()
    }

    #[test]
    fn one_key_on_twenty_thousand_tables_finds_its_ghost() {
        let index = Index::new(&large_catalog());
        for marked in KEYS {
            assert!(ghost(&index, marked).is_some(), "{marked} offers a ghost");
        }
    }

    #[test]
    fn one_key_on_twenty_thousand_tables_fills_its_list() {
        let index = Index::new(&large_catalog());
        for marked in KEYS {
            assert!(!listed(&index, marked).is_empty(), "{marked} lists a name");
        }
    }

    /// The wall clock, read in a release build only, by CI's own step: `cargo test --release --lib -- --ignored budget::`.
    #[cfg(not(debug_assertions))]
    mod budget {
        use super::*;
        use crate::fixtures::best_of;
        use std::time::Duration;

        /// The worst key takes 0.1 ms on a laptop (`SELECT c|`, 40 000 columns to weigh), CI about four times that: ten times slower fails.
        const BUDGET: Duration = Duration::from_micros(2_500);

        #[test]
        #[ignore = "reads the wall clock: CI runs it in release"]
        fn one_key_on_twenty_thousand_tables_finds_its_ghost_within_the_budget() {
            let index = Index::new(&large_catalog());
            for marked in KEYS {
                let took = best_of(|| ghost(&index, marked));
                assert!(took < BUDGET, "{marked} took {took:?}");
            }
        }

        #[test]
        #[ignore = "reads the wall clock: CI runs it in release"]
        fn one_key_on_twenty_thousand_tables_fills_its_list_within_the_budget() {
            let index = Index::new(&large_catalog());
            for marked in KEYS {
                let took = best_of(|| listed(&index, marked));
                assert!(took < BUDGET, "{marked} took {took:?}");
            }
        }
    }
}
