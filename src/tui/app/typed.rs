//! A command typed in the palette, as `:set theme=nord`, with the words revu and slack use for the same things.
//! Pure: it reads a line against what the app holds, so what parses can run.
use super::Command;
use crate::act::{self, Act, Listing, Pane};
use crate::connection::Connection;
use crate::query::Query;
use vtui::palette::typed::{Setting, TypedLine, Typing, unknown_verb};
use vtui::theme::Theme as Palette;

/// What a line is read against.
#[derive(Clone, Copy, Debug)]
pub(super) struct Known<'a> {
    /// The pane the palette opened on.
    pub(super) pane: Pane,
    pub(super) connections: &'a [Connection],
    pub(super) queries: &'a [Query],
}

/// What the token at the end of the line can complete to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Slot {
    Verb,
    Connection,
    Query,
    Setting,
    Theme,
    Free,
}

pub(super) const VERBS: [&str; 8] = ["go", "open", "run", "save", "copy", "set", "help", "quit"];

/// The command `line` says, or why it says none.
/// A word that takes no argument refuses one, so `copy the row` stays text to filter the list with.
pub(super) fn parse(line: &str, known: Known<'_>) -> Result<Command, String> {
    let typed = TypedLine::read(line);
    let TypedLine { verb, rest, .. } = typed;
    let act = match verb {
        "" => return Err("type a command, tab completes".into()),
        "go" | "g" => return connection_named(rest, known).map(Command::Go),
        "open" | "o" => return query_at(rest, known).map(Command::Open),
        "set" => return theme_set(typed).map(Command::Theme),
        "run" => Act::Run,
        "save" => Act::Save,
        "copy" if known.pane == Pane::Result => Act::CopyCell,
        "copy" => Act::CopyStatement,
        "help" | "h" | "?" => Act::Help,
        "quit" | "q" | "exit" => Act::Quit,
        unknown => return Err(unknown_verb(unknown, VERBS)),
    };
    if !rest.is_empty() {
        return Err(format!(":{verb} takes no argument"));
    }
    listing_of(act, known.pane).map(Command::Act)
}

fn connection_named(name: &str, known: Known<'_>) -> Result<Connection, String> {
    if name.is_empty() {
        return Err(":go needs a connection".into());
    }
    let found = known.connections.iter().find(|connection| connection.name().to_string() == name);
    found.cloned().ok_or_else(|| format!("no connection `{name}`"))
}

fn query_at(path: &str, known: Known<'_>) -> Result<Query, String> {
    if path.is_empty() {
        return Err(":open needs a saved query".into());
    }
    known.queries.iter().find(|query| query.path.as_str() == path).cloned().ok_or_else(|| format!("no saved query `{path}`"))
}

fn theme_set(typed: TypedLine<'_>) -> Result<Palette, String> {
    let Some(Setting { key, value }) = typed.setting() else { return Err(":set needs key=value, e.g. theme=nord".into()) };
    if key != "theme" {
        return Err(format!("unknown setting `{key}`, try :set theme=nord"));
    }
    Palette::named(value).ok_or_else(|| format!("unknown theme `{value}`, try {}", Palette::NAMES.join(", ")))
}

fn listing_of(act: Act, pane: Pane) -> Result<Listing, String> {
    act::listings_from(pane).find(|listing| listing.act == act).ok_or_else(|| format!("no key does {}", act.name()))
}

pub(super) fn slot(line: &str) -> Slot {
    let TypedLine { verb, rest, typing } = TypedLine::read(line);
    let Typing::Argument(argument) = typing else { return Slot::Verb };
    match (verb, argument) {
        ("go" | "g", 0) => Slot::Connection,
        ("open" | "o", 0) => Slot::Query,
        ("set", 0) if rest.contains("theme=") => Slot::Theme,
        ("set", 0) => Slot::Setting,
        _ => Slot::Free,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::connection;
    use super::*;
    use crate::fixtures::nord;
    use crate::query::QueryPath;

    fn read_on(pane: Pane, line: &str) -> Result<Command, String> {
        let connections = [connection("local", "read-only")];
        let queries = [Query { folder: connections[0].name().clone(), path: QueryPath::parse("orders/daily.sql").unwrap() }];
        parse(line, Known { pane, connections: &connections, queries: &queries })
    }

    fn read(line: &str) -> Result<Command, String> {
        read_on(Pane::Editor, line)
    }

    fn act_of(line: &str) -> Option<Act> {
        match read(line) {
            Ok(Command::Act(listing)) => Some(listing.act),
            _ => None,
        }
    }

    #[test]
    fn set_theme_reads_the_palette_it_names() {
        assert_eq!(read(":set theme=nord"), Ok(Command::Theme(nord())));
    }

    #[test]
    fn set_without_key_and_value_says_how_to_write_it() {
        assert_eq!(read("set theme"), Err(":set needs key=value, e.g. theme=nord".to_owned()));
    }

    #[test]
    fn set_of_another_key_names_the_one_there_is() {
        assert_eq!(read("set size=2"), Err("unknown setting `size`, try :set theme=nord".to_owned()));
    }

    #[test]
    fn set_of_an_unknown_theme_names_those_there_are() {
        assert!(read("set theme=solarized").unwrap_err().contains("nord"));
    }

    #[test]
    fn go_finds_the_connection_it_names() {
        assert!(matches!(read("go shop"), Ok(Command::Go(connection)) if connection.name().to_string() == "shop"));
    }

    #[test]
    fn go_to_a_connection_there_is_not_says_so() {
        assert_eq!(read("g nowhere"), Err("no connection `nowhere`".to_owned()));
    }

    #[test]
    fn go_alone_says_what_it_needs() {
        assert_eq!(read("go"), Err(":go needs a connection".to_owned()));
    }

    #[test]
    fn open_finds_the_query_at_its_path() {
        assert!(matches!(read("o orders/daily.sql"), Ok(Command::Open(query)) if query.path.as_str() == "orders/daily.sql"));
    }

    #[test]
    fn open_of_a_query_there_is_not_says_so() {
        assert_eq!(read("open it"), Err("no saved query `it`".to_owned()));
    }

    #[test]
    fn words_and_their_aliases_say_their_act() {
        let words = [("run", Act::Run), ("save", Act::Save), ("?", Act::Help), ("help", Act::Help), ("q", Act::Quit), ("exit", Act::Quit)];
        assert_eq!(words.map(|(word, _)| act_of(word)), words.map(|(_, act)| Some(act)));
    }

    #[test]
    fn copy_is_the_cell_on_the_result_and_the_statement_elsewhere() {
        assert_eq!(act_of("copy"), Some(Act::CopyStatement));
        assert!(matches!(read_on(Pane::Result, "copy"), Ok(Command::Act(listing)) if listing.act == Act::CopyCell));
    }

    #[test]
    fn a_word_that_takes_no_argument_refuses_one() {
        assert_eq!(read("copy the row"), Err(":copy takes no argument".to_owned()));
    }

    #[test]
    fn an_unknown_word_names_those_it_comes_close_to() {
        assert!(read("sav").unwrap_err().contains("did you mean :save"));
    }

    #[test]
    fn the_token_at_the_end_of_the_line_picks_the_slot() {
        let slots = [
            ("se", Slot::Verb),
            ("go ", Slot::Connection),
            ("open or", Slot::Query),
            ("set ", Slot::Setting),
            ("set theme=no", Slot::Theme),
            ("go shop now", Slot::Free),
        ];
        assert_eq!(slots.map(|(line, _)| slot(line)), slots.map(|(_, slot)| slot));
    }
}
