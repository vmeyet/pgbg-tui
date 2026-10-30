//! `o` on a JSON cell of the grid: its value as a tree in a popup, each object and array folding, the value or the path
//! under the cursor copied.
use super::{Action, App, Overlay, Run};
use crate::act::{self, Act, Pane};
use crate::db::model::{ColumnName, JsonText, Value};
use crate::syntax::json_string_end;
use crossterm::event::KeyEvent;
use std::collections::BTreeSet;
use std::ops::Range;

/// How a value is reached from the one holding it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    /// An object's key, as the JSON writes it, quotes and escapes kept.
    Key(String),
    Index(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Object,
    Array,
    String,
    Other,
}

/// One value of the JSON, in the order it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Node {
    parent: Option<usize>,
    depth: usize,
    step: Option<Step>,
    /// Its JSON in the compact text.
    bytes: Range<usize>,
    shape: Shape,
    /// The first node past its own members, at any depth.
    end: usize,
}

/// The mark before a line: whether it folds, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Mark {
    Open,
    Folded,
    Leaf,
}

/// A line as the popup draws it: its depth, its mark and its JSON text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct TreeLine {
    pub(in crate::tui) depth: usize,
    pub(in crate::tui) mark: Mark,
    pub(in crate::tui) text: String,
}

/// The JSON of one cell, its folded values and the cursor's line.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct JsonTree {
    pub(in crate::tui) column: ColumnName,
    /// The row's number, from 1.
    pub(in crate::tui) row: usize,
    json: JsonText,
    nodes: Vec<Node>,
    folded: BTreeSet<usize>,
    pub(in crate::tui) at: usize,
}

/// Reads compact JSON, known valid, into its nodes; one pass, no value decoded.
struct Reader<'t> {
    text: &'t str,
    at: usize,
    nodes: Vec<Node>,
}

impl Reader<'_> {
    fn value(&mut self, parent: Option<usize>, depth: usize, step: Option<Step>) {
        let index = self.nodes.len();
        let start = self.at;
        self.nodes.push(Node { parent, depth, step, bytes: start..start, shape: Shape::Other, end: index });
        let shape = match self.text.as_bytes().get(start) {
            Some(b'{') => self.members(index, depth, Shape::Object),
            Some(b'[') => self.members(index, depth, Shape::Array),
            Some(b'"') => {
                self.at = json_string_end(self.text.as_bytes(), start);
                Shape::String
            }
            _ => {
                self.at = self.scalar_end();
                Shape::Other
            }
        };
        let (end, read_to) = (self.nodes.len(), self.at);
        let node = &mut self.nodes[index];
        (node.bytes, node.shape, node.end) = (start..read_to, shape, end);
    }

    /// Reads the members of the object or array opened at the cursor; returns its shape.
    fn members(&mut self, parent: usize, depth: usize, shape: Shape) -> Shape {
        let (is_object, close) = (shape == Shape::Object, if shape == Shape::Object { b'}' } else { b']' });
        self.at += 1;
        let mut count = 0;
        while let Some(&byte) = self.text.as_bytes().get(self.at)
            && byte != close
        {
            if byte == b',' {
                self.at += 1;
            }
            let step = if is_object { Step::Key(self.key()) } else { Step::Index(count) };
            self.value(Some(parent), depth + 1, Some(step));
            count += 1;
        }
        self.at += 1;
        shape
    }

    /// The key at the cursor, the cursor left past its `:`.
    fn key(&mut self) -> String {
        let start = self.at;
        self.at = json_string_end(self.text.as_bytes(), start) + 1;
        self.text[start..self.at - 1].to_owned()
    }

    /// Past a number, `true`, `false` or `null`: the next `,`, `]` or `}`, at least one byte on.
    fn scalar_end(&self) -> usize {
        let rest = self.text.as_bytes().get(self.at + 1..).unwrap_or_default();
        rest.iter().position(|byte| b",]}".contains(byte)).map_or(self.text.len(), |length| self.at + 1 + length)
    }
}

fn nodes_of(json: &JsonText) -> Vec<Node> {
    let mut reader = Reader { text: json.as_str(), at: 0, nodes: vec![] };
    reader.value(None, 0, None);
    reader.nodes
}

/// A key's text, unescaped; the raw text inside its quotes should the JSON, read once already, not parse.
fn unquoted(key: &str) -> String {
    serde_json::from_str(key).unwrap_or_else(|_| key.trim_matches('"').to_owned())
}

fn is_identifier(key: &str) -> bool {
    key.chars().next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl JsonTree {
    pub(in crate::tui) fn new(column: ColumnName, row: usize, json: JsonText) -> Self {
        let nodes = nodes_of(&json);
        Self { column, row, json, nodes, folded: BTreeSet::new(), at: 0 }
    }

    /// The node of each line shown: every value in order, the members of a folded one left out.
    fn shown(&self) -> Vec<usize> {
        let mut shown = vec![];
        let mut index = 0;
        while index < self.nodes.len() {
            shown.push(index);
            index = if self.mark(index) == Mark::Open { index + 1 } else { self.nodes[index].end };
        }
        shown
    }

    fn mark(&self, index: usize) -> Mark {
        let node = &self.nodes[index];
        let has_members = node.end > index + 1;
        match node.shape {
            Shape::Object | Shape::Array if has_members && self.folded.contains(&index) => Mark::Folded,
            Shape::Object | Shape::Array if has_members => Mark::Open,
            _ => Mark::Leaf,
        }
    }

    pub(in crate::tui) fn lines(&self) -> Vec<TreeLine> {
        self.shown().into_iter().map(|shown| self.line(shown)).collect()
    }

    fn line(&self, index: usize) -> TreeLine {
        let node = &self.nodes[index];
        let mark = self.mark(index);
        let value = match (mark, node.shape) {
            (Mark::Leaf, _) => &self.json.as_str()[node.bytes.clone()],
            (Mark::Open, Shape::Object) => "{",
            (Mark::Open, _) => "[",
            (Mark::Folded, Shape::Object) => "{…}",
            (Mark::Folded, _) => "[…]",
        };
        let text = match &node.step {
            Some(Step::Key(key)) => format!("{key}: {value}"),
            _ => value.to_owned(),
        };
        TreeLine { depth: node.depth, mark, text }
    }

    fn node_at(&self) -> usize {
        self.shown().get(self.at).copied().unwrap_or(0)
    }

    fn step(&mut self, delta: isize) {
        let last = self.shown().len().saturating_sub(1);
        self.at = self.at.saturating_add_signed(delta).min(last);
    }

    fn go_to(&mut self, index: usize) {
        self.at = self.shown().iter().position(|&shown| shown == index).unwrap_or(0);
    }

    fn set_fold(&mut self, index: usize, is_folded: bool) {
        if self.mark(index) != Mark::Leaf {
            _ = if is_folded { self.folded.insert(index) } else { self.folded.remove(&index) };
        }
        self.go_to(index);
    }

    /// Folds or unfolds the value under the cursor; on a value that does not fold, folds the one holding it.
    fn toggle(&mut self) {
        let index = self.node_at();
        match (self.mark(index), self.nodes[index].parent) {
            (Mark::Leaf, Some(parent)) => self.set_fold(parent, true),
            (mark, _) => self.set_fold(index, mark == Mark::Open),
        }
    }

    /// The value under the cursor to paste: a string's text unescaped, any other value as compact JSON.
    fn value_text(&self) -> String {
        let node = &self.nodes[self.node_at()];
        let json = &self.json.as_str()[node.bytes.clone()];
        if node.shape == Shape::String { unquoted(json) } else { json.to_owned() }
    }

    /// The steps from the cell's value to the one under the cursor, the outermost first.
    fn steps(&self) -> Vec<&Step> {
        let ancestors = std::iter::successors(Some(self.node_at()), |&index| self.nodes[index].parent);
        let mut steps: Vec<&Step> = ancestors.filter_map(|index| self.nodes[index].step.as_ref()).collect();
        steps.reverse();
        steps
    }

    /// The path under the cursor as jq writes it: `.items[0].sku`, a key that is no plain word in quotes, `.` alone at the
    /// top.
    pub(in crate::tui) fn path(&self) -> String {
        let written: String = self
            .steps()
            .into_iter()
            .map(|step| match step {
                Step::Key(key) if is_identifier(&unquoted(key)) => format!(".{}", unquoted(key)),
                Step::Key(key) => format!(".{key}"),
                Step::Index(index) => format!("[{index}]"),
            })
            .collect();
        if written.is_empty() { ".".to_owned() } else { written }
    }
}

impl App {
    /// Opens the tree of the grid's cell, or says on the status line that it holds no JSON.
    pub(super) fn open_json_tree(&mut self) {
        let Run::Done { result, grid, .. } = &self.sheet().run else { return };
        let Some(value) = result.rows().get(grid.row).and_then(|values| values.get(grid.column)) else { return };
        let column = result.columns()[grid.column].name.clone();
        match value {
            Value::Json(json) => self.overlay = Some(Overlay::JsonTree(Box::new(JsonTree::new(column, grid.row + 1, json.clone())))),
            _ => self.note = Some(format!("{column} holds no JSON here")),
        }
    }

    /// The tree after `key`: it moves, folds and unfolds, `y` and `Y` copy the value and the path, `esc` closes it.
    pub(super) fn json_tree_key(&mut self, mut tree: Box<JsonTree>, key: KeyEvent) -> Vec<Action> {
        let act = act::act_for(key, Pane::JsonTree);
        match act {
            Some(Act::Quit) => return self.quit(),
            Some(Act::Close) => return vec![],
            Some(Act::Down) => tree.step(1),
            Some(Act::Up) => tree.step(-1),
            Some(Act::Open) => tree.toggle(),
            _ => {}
        }
        let copied = act.and_then(|act| copied(&tree, act));
        self.overlay = Some(Overlay::JsonTree(tree));
        copied.into_iter().collect()
    }
}

/// What `act` copies from `tree`: the value under the cursor, or its path.
fn copied(tree: &JsonTree, act: Act) -> Option<Action> {
    match act {
        Act::CopyCell => Some(Action::Copy { text: tree.value_text(), done: format!("copied the value at {}", tree.path()) }),
        Act::CopyPath => Some(Action::Copy { text: tree.path(), done: format!("copied {}", tree.path()) }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{on_the_result, press};
    use super::*;
    use crate::db::model::{ColumnKind, ResultSet};
    use crate::fixtures::column;
    use crossterm::event::KeyCode;

    const ORDER: &str = r#"{"id":7,"items":[{"sku":"TEA-01","count":2},{"sku":"say \"hi\", [x]","count":1}],"gift note":null,"tags":{}}"#;

    fn tree_of(json: &str) -> JsonTree {
        JsonTree::new(ColumnName::new("data"), 1, JsonText::compacted(json).unwrap())
    }

    fn texts(tree: &JsonTree) -> Vec<String> {
        tree.lines().into_iter().map(|line| format!("{}{}", "  ".repeat(line.depth), line.text)).collect()
    }

    fn on_line(json: &str, line: usize) -> JsonTree {
        JsonTree { at: line, ..tree_of(json) }
    }

    #[test]
    fn the_tree_shows_each_value_on_its_line_in_the_order_written() {
        assert_eq!(
            texts(&tree_of(ORDER)),
            [
                "{",
                r#"  "id": 7"#,
                r#"  "items": ["#,
                "    {",
                r#"      "sku": "TEA-01""#,
                r#"      "count": 2"#,
                "    {",
                r#"      "sku": "say \"hi\", [x]""#,
                r#"      "count": 1"#,
                r#"  "gift note": null"#,
                r#"  "tags": {}"#,
            ]
        );
    }

    #[test]
    fn toggling_an_array_folds_its_members_away_and_again_brings_them_back() {
        let mut tree = on_line(ORDER, 2);
        tree.toggle();
        let line = &tree.lines()[2];
        assert_eq!((line.text.as_str(), line.mark, tree.lines().len()), (r#""items": […]"#, Mark::Folded, 5));
        tree.toggle();
        assert_eq!(tree.lines().len(), 11);
    }

    #[test]
    fn toggling_a_scalar_folds_the_value_holding_it_and_moves_there() {
        let mut tree = on_line(ORDER, 4);
        tree.toggle();
        assert_eq!((tree.at, tree.lines()[3].mark), (3, Mark::Folded));
    }

    #[test]
    fn the_path_under_the_cursor_reads_as_jq() {
        assert_eq!(on_line(ORDER, 7).path(), ".items[1].sku");
        assert_eq!(on_line(ORDER, 9).path(), r#"."gift note""#);
        assert_eq!(on_line(ORDER, 0).path(), ".");
    }

    #[test]
    fn the_value_of_a_string_is_its_text_and_of_an_object_its_json() {
        assert_eq!(on_line(ORDER, 7).value_text(), "say \"hi\", [x]");
        assert_eq!(on_line(ORDER, 3).value_text(), r#"{"sku":"TEA-01","count":2}"#);
    }

    fn on_a_json_cell() -> App {
        let data = Value::Json(JsonText::compacted(ORDER).unwrap());
        let result =
            ResultSet::whole(vec![column("id", ColumnKind::Int), column("data", ColumnKind::Json)], vec![vec![Value::Int(7), data]]);
        on_the_result(result)
    }

    #[test]
    fn o_on_a_json_cell_opens_its_tree() {
        let mut app = on_a_json_cell();
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('o'));
        assert!(matches!(&app.overlay, Some(Overlay::JsonTree(tree)) if tree.column.as_str() == "data"));
    }

    #[test]
    fn o_on_a_cell_without_json_says_so() {
        let mut app = on_a_json_cell();
        press(&mut app, KeyCode::Char('o'));
        assert_eq!((app.overlay.is_none(), app.note.as_deref()), (true, Some("id holds no JSON here")));
    }

    fn in_the_tree() -> App {
        let mut app = on_a_json_cell();
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('o'));
        (0..7).for_each(|_| press(&mut app, KeyCode::Char('j')));
        app
    }

    #[test]
    fn y_copies_the_value_and_capital_y_the_path_and_the_tree_stays_open() {
        let mut app = in_the_tree();
        let copied = |text: &str, done: &str| vec![Action::Copy { text: text.to_owned(), done: done.to_owned() }];
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Char('y'))), copied("say \"hi\", [x]", "copied the value at .items[1].sku"));
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Char('Y'))), copied(".items[1].sku", "copied .items[1].sku"));
        assert!(matches!(app.overlay, Some(Overlay::JsonTree(_))));
    }
}
