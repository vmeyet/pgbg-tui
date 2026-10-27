//! A list narrowed as fzf does, for the `ctrl-p` finder and the `ctrl-o` picker: the line typed and what it matches,
//! ranked again only when the line or the list changes.
use crate::act::{self, Act, Pane};
use crate::connection::Connection;
use crate::history::Entry;
use crate::query::Query;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vtui::fuzzy;
use vtui::palette::{Mode, Palette};

/// The only mode of a search: what is typed is matched, never a command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct PlainSearch;

impl Mode for PlainSearch {
    fn switched_by(self, _first: char) -> Option<Self> {
        None
    }

    fn takes_commands(self) -> bool {
        false
    }
}

/// What a search lists, matched on its text.
pub(in crate::tui) trait Searched: Clone {
    fn search_text(&self) -> String;
}

impl Searched for Query {
    fn search_text(&self) -> String {
        self.path.to_string()
    }
}

impl Searched for Entry {
    fn search_text(&self) -> String {
        self.statement.clone()
    }
}

impl Searched for Connection {
    fn search_text(&self) -> String {
        self.name().to_string()
    }
}

/// The line typed, and what it matches, best first.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Search<T> {
    pub(in crate::tui) palette: Palette<PlainSearch>,
    pub(in crate::tui) shown: Vec<T>,
}

/// What a key did to a search.
pub(super) enum Searching<T> {
    Typed(Search<T>),
    Closed,
    Picked(T),
    Quit,
    /// An action of the pane the search leaves to its owner, such as removing the highlighted connection.
    Unhandled(Search<T>, Act),
}

impl<T: Searched> Search<T> {
    /// Every one of `all`, in its order, before anything is typed.
    pub(super) fn new(all: &[T]) -> Self {
        let mut search = Self { palette: Palette::default(), shown: vec![] };
        search.rank(all);
        search
    }

    pub(super) fn rank(&mut self, all: &[T]) {
        let ranked = fuzzy::rank(&self.palette.input, all.iter().map(|item| (item.search_text(), item)));
        self.shown = ranked.into_iter().map(|(_, item)| item.clone()).collect();
        self.palette.move_by(0, self.shown.len());
    }

    /// Types `text` on the line as vtui cleans a paste, without the blank around it a copy often carries, then ranks once.
    pub(super) fn paste(&mut self, text: &str, all: &[T]) {
        self.palette.type_str(text.trim());
        self.rank(all);
    }

    /// The index in `shown` of the first match in a view of `rows`, scrolled so the highlighted one shows.
    pub(in crate::tui) fn first_in_view(&self, rows: usize) -> usize {
        self.palette.selected.saturating_sub(rows.saturating_sub(1))
    }

    /// The matches in a view of `rows`.
    pub(in crate::tui) fn in_view(&self, rows: usize) -> &[T] {
        let first = self.first_in_view(rows);
        &self.shown[first..self.shown.len().min(first + rows)]
    }

    /// What `key` does where `pane` has the keys: its own keys move, pick, close and quit, the rest types over `all`.
    pub(super) fn after(mut self, key: KeyEvent, pane: Pane, all: &[T]) -> Searching<T> {
        match act::act_for(key, pane) {
            Some(Act::Down) => self.palette.move_by(1, self.shown.len()),
            Some(Act::Up) => self.palette.move_by(-1, self.shown.len()),
            Some(Act::Open) => return self.picked(),
            Some(Act::Close) => return Searching::Closed,
            Some(Act::Quit) => return Searching::Quit,
            Some(act) => return Searching::Unhandled(self, act),
            None => match key.code {
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.palette.type_char(c);
                    self.rank(all);
                }
                KeyCode::Backspace if !self.palette.backspace() => return Searching::Closed,
                KeyCode::Backspace => self.rank(all),
                _ => {}
            },
        }
        Searching::Typed(self)
    }

    /// The highlighted match, when one matches.
    pub(super) fn selected(&self) -> Option<&T> {
        self.shown.get(self.palette.selected)
    }

    /// The highlighted match; the search stays open when nothing matches.
    fn picked(self) -> Searching<T> {
        match self.selected().cloned() {
            Some(item) => Searching::Picked(item),
            None => Searching::Typed(self),
        }
    }
}
