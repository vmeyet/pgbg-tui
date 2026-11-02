//! The folding of a tree drawn one node a line, the JSON tree's and the plan's: which nodes fold, which lines show, and
//! the cursor among them.
use std::collections::BTreeSet;

/// The mark before a line: whether it folds, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Mark {
    Open,
    Folded,
    Leaf,
}

/// Where a node sits among nodes written parents first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Span {
    pub(super) parent: Option<usize>,
    /// The first node past its own members, at any depth.
    pub(super) end: usize,
}

/// The folded nodes of a tree and the cursor's line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct Folds {
    spans: Vec<Span>,
    folded: BTreeSet<usize>,
    /// The node of each line shown, built again only when a node folds or unfolds.
    shown: Vec<usize>,
    pub(in crate::tui) at: usize,
}

impl Folds {
    pub(super) fn new(spans: Vec<Span>) -> Self {
        let folds = Self { spans, folded: BTreeSet::new(), shown: vec![], at: 0 };
        Self { shown: folds.lines(), ..folds }
    }

    /// The node of each line: every node in order, the members of a folded one left out.
    fn lines(&self) -> Vec<usize> {
        let mut shown = vec![];
        let mut index = 0;
        while index < self.spans.len() {
            shown.push(index);
            index = if self.mark(index) == Mark::Open { index + 1 } else { self.spans[index].end };
        }
        shown
    }

    pub(super) fn mark(&self, index: usize) -> Mark {
        if self.spans[index].end <= index + 1 {
            return Mark::Leaf;
        }
        if self.folded.contains(&index) { Mark::Folded } else { Mark::Open }
    }

    /// The nodes of the `count` lines from `first` on.
    pub(super) fn shown_in(&self, first: usize, count: usize) -> impl Iterator<Item = usize> {
        self.shown.iter().skip(first).take(count).copied()
    }

    /// The first line of a window of `height` lines that shows the cursor's.
    pub(in crate::tui) fn first_line_in(&self, height: usize) -> usize {
        self.at.saturating_sub(height.saturating_sub(1))
    }

    /// The node under the cursor.
    pub(super) fn node_at(&self) -> usize {
        self.shown.get(self.at).copied().unwrap_or(0)
    }

    pub(super) fn step(&mut self, delta: isize) {
        let last = self.shown.len().saturating_sub(1);
        self.at = self.at.saturating_add_signed(delta).min(last);
    }

    fn set_fold(&mut self, index: usize, is_folded: bool) {
        if self.mark(index) != Mark::Leaf {
            _ = if is_folded { self.folded.insert(index) } else { self.folded.remove(&index) };
            self.shown = self.lines();
        }
        self.at = self.shown.iter().position(|&shown| shown == index).unwrap_or(0);
    }

    /// Folds or unfolds the node under the cursor; on a node that does not fold, folds the one holding it.
    pub(super) fn toggle(&mut self) {
        let index = self.node_at();
        match (self.mark(index), self.spans[index].parent) {
            (Mark::Leaf, Some(parent)) => self.set_fold(parent, true),
            (mark, _) => self.set_fold(index, mark == Mark::Open),
        }
    }

    #[cfg(test)]
    pub(super) fn line_count(&self) -> usize {
        self.shown.len()
    }
}
