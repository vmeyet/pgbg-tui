//! The words the guard reads as a sign of a write, which a name spelt so must hide in quotes.

/// Words that change data or create a table (`SELECT ... INTO`).
pub(crate) const WRITES: [&str; 5] = ["INSERT", "UPDATE", "DELETE", "MERGE", "INTO"];
/// Row locks a `SELECT` takes; `FOR UPDATE` is caught by `UPDATE` already.
pub(crate) const LOCKS: [[&str; 2]; 2] = [["FOR", "SHARE"], ["KEY", "SHARE"]];

/// Whether the guard reads `word`, bare, as a sign of a write: a name spelt so is safe from it only in quotes.
pub(crate) fn reads_as_write(word: &str) -> bool {
    let is_word = |known: &&str| known.eq_ignore_ascii_case(word);
    WRITES.iter().any(is_word) || LOCKS.iter().flatten().any(is_word)
}
