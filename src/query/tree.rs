//! The queries as the sidebar shows them: folders first, then files, each level sorted, folders nested.
use super::Query;
use crate::render;
use std::collections::{BTreeMap, BTreeSet};

/// One line of the tree; `name` is drawn as it is, its control characters escaped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeRow {
    /// `key` names the folder for `closed`: its path, as `orders/daily`.
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

impl TreeRow {
    /// Whether both rows stand for the same query or folder, however deep the tree now draws it.
    pub(crate) fn is_at(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Folder { key, .. }, Self::Folder { key: other, .. }) => key == other,
            (Self::Query { query, .. }, Self::Query { query: other, .. }) => query == other,
            _ => false,
        }
    }
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

    /// The rows of this folder at `depth`, whose path from the top is `prefix`, as `orders/`.
    fn rows(&self, depth: usize, prefix: &str, closed: &BTreeSet<String>) -> Vec<TreeRow> {
        let mut rows = vec![];
        for (name, folder) in &self.folders {
            let key = format!("{prefix}{name}");
            let is_open = !closed.contains(&key);
            rows.push(TreeRow::Folder { name: render::printable(name).into_owned(), depth, key: key.clone(), is_open });
            if is_open {
                rows.extend(folder.rows(depth + 1, &format!("{key}/"), closed));
            }
        }
        rows.extend(self.queries.iter().map(|(name, query)| TreeRow::Query {
            name: render::printable(name).into_owned(),
            depth,
            query: (*query).clone(),
        }));
        rows
    }
}

/// The rows of `queries` with the folders in `closed` folded.
pub(crate) fn tree_rows(queries: &[Query], closed: &BTreeSet<String>) -> Vec<TreeRow> {
    let mut top = Folder::default();
    for query in queries {
        top.insert(query);
    }
    top.rows(0, "", closed)
}

#[cfg(test)]
mod tests {
    use super::super::QueryPath;
    use super::*;

    fn queries(paths: &[&str]) -> Vec<Query> {
        paths.iter().map(|path| Query { folder: "shop".parse().unwrap(), path: QueryPath::parse(path).unwrap() }).collect()
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
        let all = queries(&["zeta.sql", "orders/weekly.sql", "orders/daily/top.sql", "alpha.sql", "customers/new.sql"]);
        let rows = tree_rows(&all, &BTreeSet::new());
        assert_eq!(
            drawn(&rows),
            ["▾ customers/", "  new.sql", "▾ orders/", "  ▾ daily/", "    top.sql", "  weekly.sql", "alpha.sql", "zeta.sql"]
        );
    }

    #[test]
    fn a_closed_folder_hides_what_it_holds() {
        let all = queries(&["orders/daily.sql", "alpha.sql"]);
        let closed = BTreeSet::from(["orders".to_owned()]);
        assert_eq!(drawn(&tree_rows(&all, &closed)), ["▸ orders/", "alpha.sql"]);
    }

    #[test]
    fn a_control_character_in_a_folder_or_file_name_is_drawn_escaped() {
        let all = queries(&["orders\u{1b}[2J/daily\u{202e}.sql"]);
        assert_eq!(drawn(&tree_rows(&all, &BTreeSet::new())), ["▾ orders\\u{1b}[2J/", "  daily\\u{202e}.sql"]);
    }
}
