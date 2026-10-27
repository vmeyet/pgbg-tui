//! Two texts line by line, as a reader compares a statement with the answer that would replace it.

/// The most lines of each side compared line by line: the table that compares them grows with both sides, on the loop.
const MAX_LINES_COMPARED: usize = 2_000;

/// What became of one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Change {
    Kept,
    Removed,
    Added,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct DiffLine {
    pub(in crate::tui) change: Change,
    pub(in crate::tui) text: String,
}

/// The lines of `old` and `new` in order: the longest run of lines they share kept, the others removed from `old` or
/// added in `new`, a removed line before the line added in its place; past `MAX_LINES_COMPARED` on either side, every
/// line of `old` removed, then every line of `new` added.
pub(super) fn lines(old: &str, new: &str) -> Vec<DiffLine> {
    let (old, new): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    if old.len() > MAX_LINES_COMPARED || new.len() > MAX_LINES_COMPARED {
        return replaced(&old, &new);
    }
    compared(&old, &new)
}

fn line(change: Change, text: &str) -> DiffLine {
    DiffLine { change, text: text.to_owned() }
}

/// Every line of `old` removed, then every line of `new` added.
fn replaced(old: &[&str], new: &[&str]) -> Vec<DiffLine> {
    let removed = old.iter().map(|text| line(Change::Removed, text));
    removed.chain(new.iter().map(|text| line(Change::Added, text))).collect()
}

/// `old` against `new` line by line, the longest run of lines they share kept.
fn compared(old: &[&str], new: &[&str]) -> Vec<DiffLine> {
    let shared = shared_after(old, new);
    let (mut at_old, mut at_new) = (0, 0);
    let mut lines = vec![];
    while at_old < old.len() || at_new < new.len() {
        if at_old < old.len() && at_new < new.len() && old[at_old] == new[at_new] {
            lines.push(line(Change::Kept, old[at_old]));
            (at_old, at_new) = (at_old + 1, at_new + 1);
        } else if at_old < old.len() && (at_new == new.len() || shared[at_old + 1][at_new] >= shared[at_old][at_new + 1]) {
            lines.push(line(Change::Removed, old[at_old]));
            at_old += 1;
        } else {
            lines.push(line(Change::Added, new[at_new]));
            at_new += 1;
        }
    }
    lines
}

/// `shared[i][j]`: how many lines `old[i..]` and `new[j..]` share, in order.
fn shared_after(old: &[&str], new: &[&str]) -> Vec<Vec<usize>> {
    let mut shared = vec![vec![0; new.len() + 1]; old.len() + 1];
    for at_old in (0..old.len()).rev() {
        for at_new in (0..new.len()).rev() {
            shared[at_old][at_new] = if old[at_old] == new[at_new] {
                shared[at_old + 1][at_new + 1] + 1
            } else {
                shared[at_old + 1][at_new].max(shared[at_old][at_new + 1])
            };
        }
    }
    shared
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(old: &str, new: &str) -> Vec<String> {
        let mark = |change| match change {
            Change::Kept => ' ',
            Change::Removed => '-',
            Change::Added => '+',
        };
        lines(old, new).into_iter().map(|line| format!("{}{}", mark(line.change), line.text)).collect()
    }

    #[test]
    fn a_changed_line_is_removed_then_added_between_the_lines_kept() {
        let old = "SELECT *\nFROM orders\nLIMIT 10";
        let new = "SELECT id\nFROM orders\nLIMIT 10";
        assert_eq!(marked(old, new), ["-SELECT *", "+SELECT id", " FROM orders", " LIMIT 10"]);
    }

    #[test]
    fn past_the_cap_every_old_line_is_removed_then_every_new_one_added() {
        let text = vec!["SELECT 1"; MAX_LINES_COMPARED + 1].join("\n");
        let changes: Vec<Change> = lines(&text, &text).into_iter().map(|line| line.change).collect();
        let expected = [vec![Change::Removed; MAX_LINES_COMPARED + 1], vec![Change::Added; MAX_LINES_COMPARED + 1]].concat();
        assert_eq!(changes, expected);
    }

    #[test]
    fn a_new_statement_is_all_added() {
        assert_eq!(marked("", "SELECT id\nFROM orders"), ["+SELECT id", "+FROM orders"]);
    }

    #[test]
    fn lines_added_and_dropped_keep_the_lines_between() {
        let old = "SELECT id\nFROM orders\nWHERE total > 10";
        let new = "SELECT id, total\nFROM orders\nJOIN customers ON true";
        assert_eq!(marked(old, new), ["-SELECT id", "+SELECT id, total", " FROM orders", "-WHERE total > 10", "+JOIN customers ON true"]);
    }
}
