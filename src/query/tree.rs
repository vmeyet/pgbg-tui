//! The queries as the sidebar shows them: folders first, then files, each level sorted, folders nested.
use super::{Query, Source};
use std::collections::{BTreeMap, BTreeSet};

/// One line of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeRow {
    /// `key` names the folder for `closed`: its source and its path, as `project/orders`.
    Folder {
        name: String,
        depth: usize,
        key: String,
        is_open: bool,
    },
    Query {
        name: String,
        depth: usize,
        query: Query,
    },
}

#[derive(Default)]
struct Folder<'a> {
    folders: BTreeMap<&'a str, Folder<'a>>,
    queries: BTreeMap<&'a str, &'a Query>,
}

impl<'a> Folder<'a> {
    fn insert(&mut self, query: &'a Query) {
        let mut parts: Vec<&str> = query.path.parts().collect();
        let file = parts.pop().unwrap_or_default();
        let folder = parts.into_iter().fold(self, |folder, part| folder.folders.entry(part).or_default());
        folder.queries.insert(file, query);
    }

    fn rows(&self, depth: usize, key: &str, closed: &BTreeSet<String>) -> Vec<TreeRow> {
        let mut rows = vec![];
        for (name, folder) in &self.folders {
            let key = format!("{key}/{name}");
            let is_open = !closed.contains(&key);
            rows.push(TreeRow::Folder { name: (*name).to_owned(), depth, key: key.clone(), is_open });
            if is_open {
                rows.extend(folder.rows(depth + 1, &key, closed));
            }
        }
        rows.extend(self.queries.iter().map(|(name, query)| TreeRow::Query { name: (*name).to_owned(), depth, query: (*query).clone() }));
        rows
    }
}

/// The rows of `queries` with the folders in `closed` folded; each source is a folder of its own when both have queries.
pub(crate) fn tree_rows(queries: &[Query], closed: &BTreeSet<String>) -> Vec<TreeRow> {
    let mut by_source: BTreeMap<Source, Folder<'_>> = BTreeMap::new();
    for query in queries {
        by_source.entry(query.source).or_default().insert(query);
    }
    if by_source.len() == 1 {
        return by_source.iter().flat_map(|(source, folder)| folder.rows(0, &source.to_string(), closed)).collect();
    }
    let source_folder = |source: &Source, folder: &Folder<'_>| {
        let key = source.to_string();
        let is_open = !closed.contains(&key);
        let rows = if is_open { folder.rows(1, &key, closed) } else { vec![] };
        std::iter::once(TreeRow::Folder { name: key.clone(), depth: 0, key, is_open }).chain(rows)
    };
    by_source.iter().flat_map(|(source, folder)| source_folder(source, folder)).collect()
}

#[cfg(test)]
mod tests {
    use super::super::QueryPath;
    use super::*;

    fn queries(source: Source, paths: &[&str]) -> Vec<Query> {
        paths.iter().map(|path| Query { source, path: QueryPath::parse(path).unwrap() }).collect()
    }

    fn drawn(rows: &[TreeRow]) -> Vec<String> {
        let line = |row: &TreeRow| match row {
            TreeRow::Folder { name, depth, is_open, .. } => format!("{}{} {name}/", "  ".repeat(*depth), if *is_open { "▾" } else { "▸" }),
            TreeRow::Query { name, depth, .. } => format!("{}{name}", "  ".repeat(*depth)),
        };
        rows.iter().map(line).collect()
    }

    #[test]
    fn folders_come_first_then_files_each_level_sorted() {
        let all = queries(Source::Project, &["zeta.sql", "orders/weekly.sql", "orders/daily/top.sql", "alpha.sql", "customers/new.sql"]);
        let rows = tree_rows(&all, &BTreeSet::new());
        assert_eq!(
            drawn(&rows),
            ["▾ customers/", "  new.sql", "▾ orders/", "  ▾ daily/", "    top.sql", "  weekly.sql", "alpha.sql", "zeta.sql"]
        );
    }

    #[test]
    fn a_closed_folder_hides_what_it_holds() {
        let all = queries(Source::Project, &["orders/daily.sql", "alpha.sql"]);
        let closed = BTreeSet::from(["project/orders".to_owned()]);
        assert_eq!(drawn(&tree_rows(&all, &closed)), ["▸ orders/", "alpha.sql"]);
    }

    #[test]
    fn each_source_is_a_folder_when_both_have_queries() {
        let mut all = queries(Source::Project, &["orders.sql"]);
        all.extend(queries(Source::User, &["mine/top.sql"]));
        assert_eq!(drawn(&tree_rows(&all, &BTreeSet::new())), ["▾ project/", "  orders.sql", "▾ user/", "  ▾ mine/", "    top.sql"]);
    }
}
