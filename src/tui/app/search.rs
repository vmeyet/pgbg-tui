//! What pgbg's lists search on, and what each key does in them; the line and its matches are vtui's `Search`.
use super::versions::Listed;
use crate::act::{self, Act, Pane};
use crate::connection::Connection;
use crate::history::Entry;
use crate::query::Query;
use crate::secret;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vtui::palette::Mode;
use vtui::palette::search::PlainSearch;
pub(in crate::tui) use vtui::palette::search::{Search, Searched};
use vtui::toward::Toward;

impl Searched for Query {
    fn search_text(&self) -> String {
        self.path.to_string()
    }
}

impl Searched for Entry {
    fn search_text(&self) -> String {
        self.statement.as_str().to_owned()
    }
}

impl Searched for Listed {
    fn search_text(&self) -> String {
        self.version.text.as_deref().unwrap_or(secret::NOT_KEPT).to_owned()
    }
}

impl Searched for Connection {
    fn search_text(&self) -> String {
        self.name().to_string()
    }
}

/// What a key did to a search.
pub(super) enum Searching<T, M = PlainSearch> {
    Typed(Search<T, M>),
    Closed,
    Picked(T),
    Quit,
    /// An action of the pane that the search leaves to its owner, such as removing the highlighted connection.
    Unhandled(Search<T, M>, Act),
}

/// What `key` does to `search` in `pane`: its own keys move, pick, close and quit; other keys type and search `all` again.
pub(super) fn after<T: Searched, M: Mode>(mut search: Search<T, M>, key: KeyEvent, pane: Pane, all: &[T]) -> Searching<T, M> {
    match act::act_for(key, pane) {
        Some(Act::Down) => search.arrow(Toward::Next, all),
        Some(Act::Up) => search.arrow(Toward::Previous, all),
        Some(Act::Open) => return picked(search),
        Some(Act::Close) => return Searching::Closed,
        Some(Act::Quit) => return Searching::Quit,
        Some(act) => return Searching::Unhandled(search, act),
        None => match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                search.palette.type_char(c);
                search.rank(all);
            }
            KeyCode::Backspace if !search.palette.backspace() => return Searching::Closed,
            KeyCode::Backspace => search.rank(all),
            _ => {}
        },
    }
    Searching::Typed(search)
}

/// The highlighted match; the search stays open when nothing matches.
fn picked<T: Searched, M: Mode>(search: Search<T, M>) -> Searching<T, M> {
    match search.selected().cloned() {
        Some(item) => Searching::Picked(item),
        None => Searching::Typed(search),
    }
}
