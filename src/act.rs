//! Every action of the TUI in one table: its default keys, the pane they work in, its line in the `?` help and
//! its hint in the footer. The key handling, the help and the footer all read it from here.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vtui::help::{Row as HelpRow, Section};

/// One action; its row in [`ROWS`] says everything else about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    Run,
    NewLine,
    Cancel,
    SwitchPane,
    Help,
    Connections,
    Quit,
    Down,
    Up,
    PageDown,
    PageUp,
    Top,
    Bottom,
    Left,
    Right,
    Find,
    Open,
    Close,
    Chart,
}

#[cfg(test)]
impl Act {
    /// Every action, in the order [`Act::after`] chains them.
    pub(crate) const ALL: [Self; Self::COUNT] = Self::chained();
    const COUNT: usize = Self::chain_length();

    /// The action listed after this one: the match makes a new action name its place before the crate builds.
    const fn after(self) -> Option<Self> {
        match self {
            Self::Run => Some(Self::NewLine),
            Self::NewLine => Some(Self::Cancel),
            Self::Cancel => Some(Self::SwitchPane),
            Self::SwitchPane => Some(Self::Help),
            Self::Help => Some(Self::Connections),
            Self::Connections => Some(Self::Quit),
            Self::Quit => Some(Self::Down),
            Self::Down => Some(Self::Up),
            Self::Up => Some(Self::PageDown),
            Self::PageDown => Some(Self::PageUp),
            Self::PageUp => Some(Self::Top),
            Self::Top => Some(Self::Bottom),
            Self::Bottom => Some(Self::Left),
            Self::Left => Some(Self::Right),
            Self::Right => Some(Self::Find),
            Self::Find => Some(Self::Open),
            Self::Open => Some(Self::Close),
            Self::Close => Some(Self::Chart),
            Self::Chart => None,
        }
    }

    const fn chain_length() -> usize {
        let mut count = 1;
        let mut act = Self::Run;
        while let Some(next) = act.after() {
            act = next;
            count += 1;
        }
        count
    }

    const fn chained() -> [Self; Self::COUNT] {
        let mut all = [Self::Run; Self::COUNT];
        let mut at = 1;
        while at < Self::COUNT {
            all[at] = match all[at - 1].after() {
                Some(next) => next,
                None => Self::Run,
            };
            at += 1;
        }
        all
    }
}

/// Where an action's keys work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pane {
    /// Whichever pane has the keys, before that pane's own keys.
    Anywhere,
    Editor,
    Result,
    /// The saved queries in the sidebar.
    Queries,
    /// The `ctrl-p` finder over every saved query: while open it takes every key, and only its own work.
    Finder,
    /// The `ctrl-o` connection picker, a popup like the finder.
    Picker,
}

/// Keys as the help or the footer writes them, and what they do in a few words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hint {
    pub(crate) keys: &'static str,
    pub(crate) does: &'static str,
}

/// What pgbg knows about one action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) act: Act,
    pub(crate) pane: Pane,
    pub(crate) keys: &'static [KeyEvent],
    /// Its line in the `?` help; an action listed with another one has none.
    pub(crate) help: Option<Hint>,
    /// Its hint in the footer while its pane has the keys.
    pub(crate) footer: Option<Hint>,
}

const fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

const fn control(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

const fn row(act: Act, pane: Pane, keys: &'static [KeyEvent]) -> Row {
    Row { act, pane, keys, help: None, footer: None }
}

impl Row {
    const fn help(self, keys: &'static str, does: &'static str) -> Self {
        Self { help: Some(Hint { keys, does }), ..self }
    }

    const fn footer(self, keys: &'static str, does: &'static str) -> Self {
        Self { footer: Some(Hint { keys, does }), ..self }
    }
}

/// Every action, in the order the help and the footer list them; where two rows share a key in a pane, the first wins.
pub(crate) const ROWS: &[Row] = &[
    row(Act::Run, Pane::Editor, &[key(KeyCode::Enter)]).help("⏎", "run the statement under the cursor").footer("⏎", "run"),
    row(
        Act::NewLine,
        Pane::Editor,
        &[KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)],
    )
    .help("⌥⏎ ⇧⏎", "break the line")
    .footer("⌥⏎", "new line"),
    row(Act::Down, Pane::Result, &[key(KeyCode::Char('j')), key(KeyCode::Down)]).help("j k", "next, previous row").footer("j/k", "rows"),
    row(Act::Up, Pane::Result, &[key(KeyCode::Char('k')), key(KeyCode::Up)]),
    row(Act::PageDown, Pane::Result, &[key(KeyCode::PageDown)]).help("PgDn PgUp", "a page down, up"),
    row(Act::PageUp, Pane::Result, &[key(KeyCode::PageUp)]),
    row(Act::Top, Pane::Result, &[key(KeyCode::Char('g'))]).help("g G", "first, last row"),
    row(Act::Bottom, Pane::Result, &[key(KeyCode::Char('G'))]),
    row(Act::Left, Pane::Result, &[key(KeyCode::Char('h')), key(KeyCode::Left)]).help("h l", "column left, right").footer("h/l", "columns"),
    row(Act::Right, Pane::Result, &[key(KeyCode::Char('l')), key(KeyCode::Right)]),
    row(Act::Chart, Pane::Result, &[key(KeyCode::Char('v'))]).help("v", "the result as a grid or a chart").footer("v", "chart"),
    row(Act::Help, Pane::Result, &[key(KeyCode::Char('?'))]).help("?", "these keys; again for every key"),
    row(Act::Down, Pane::Queries, &[key(KeyCode::Char('j')), key(KeyCode::Down)]).help("j k", "next, previous query").footer("j/k", "move"),
    row(Act::Up, Pane::Queries, &[key(KeyCode::Char('k')), key(KeyCode::Up)]),
    row(Act::Open, Pane::Queries, &[key(KeyCode::Enter)]).help("⏎", "open the query, fold the folder").footer("⏎", "open"),
    row(Act::Down, Pane::Finder, &[key(KeyCode::Down), control('n')]).help("↓ ^n", "next, previous query").footer("↑↓", "pick"),
    row(Act::Up, Pane::Finder, &[key(KeyCode::Up), control('p')]),
    row(Act::Open, Pane::Finder, &[key(KeyCode::Enter)]).help("⏎", "open it in the editor").footer("⏎", "opens"),
    row(Act::Quit, Pane::Finder, &[control('c')]).help("^c", "quit"),
    row(Act::Close, Pane::Finder, &[key(KeyCode::Esc)]).help("esc", "close the finder").footer("esc", "closes"),
    row(Act::Find, Pane::Anywhere, &[control('p')]).help("^p", "find a saved query").footer("^p", "find"),
    row(Act::Cancel, Pane::Anywhere, &[key(KeyCode::Esc)]).help("esc", "cancel the running statement"),
    row(Act::SwitchPane, Pane::Anywhere, &[key(KeyCode::Tab)]).help("tab", "next pane: editor, result, queries").footer("⇥", "pane"),
    row(Act::Help, Pane::Anywhere, &[key(KeyCode::F(1))]).help("F1", "these keys, from the editor too").footer("F1", "keys"),
    row(Act::Down, Pane::Picker, &[key(KeyCode::Down), control('n')]).help("↓ ^n", "next, previous connection").footer("↑↓", "pick"),
    row(Act::Up, Pane::Picker, &[key(KeyCode::Up), control('p')]),
    row(Act::Open, Pane::Picker, &[key(KeyCode::Enter)]).help("⏎", "open it, or switch back to it").footer("⏎", "opens"),
    row(Act::Close, Pane::Picker, &[key(KeyCode::Esc), control('o')]).help("esc ^o", "close the picker").footer("esc", "closes"),
    row(Act::Quit, Pane::Picker, &[control('c')]).help("^c", "quit"),
    row(Act::Connections, Pane::Anywhere, &[control('o')]).help("^o", "pick a connection").footer("^o", "connections"),
    row(Act::Quit, Pane::Anywhere, &[control('c')]).help("^c", "quit").footer("^c", "quit"),
];

/// Whether `pressed` is `own`; a typed character is its own shift, so `G` matches with or without it.
fn is_the_key(own: &KeyEvent, pressed: KeyEvent) -> bool {
    let is_char = matches!(pressed.code, KeyCode::Char(_));
    let modifiers = if is_char { pressed.modifiers - KeyModifiers::SHIFT } else { pressed.modifiers };
    own.code == pressed.code && own.modifiers == modifiers
}

fn works_in(row: &Row, pane: Pane) -> bool {
    row.pane == pane || (row.pane == Pane::Anywhere && !matches!(pane, Pane::Finder | Pane::Picker))
}

/// What `key` does while `pane` has the keys; `None` leaves it to the pane, as text for the editor.
pub(crate) fn act_for(key: KeyEvent, pane: Pane) -> Option<Act> {
    ROWS.iter().find(|row| works_in(row, pane) && row.keys.iter().any(|own| is_the_key(own, key))).map(|row| row.act)
}

/// The footer's hints while `pane` has the keys: its own, then those that work anywhere.
pub(crate) fn footer(pane: Pane) -> impl Iterator<Item = Hint> {
    let own = ROWS.iter().filter(move |row| row.pane == pane);
    let anywhere = ROWS.iter().filter(move |row| row.pane == Pane::Anywhere && works_in(row, pane));
    own.chain(anywhere).filter_map(|row| row.footer)
}

/// The `?` help: the keys of `pane` and those that work anywhere, or every pane's with `every_key`.
pub(crate) fn sections(pane: Pane, every_key: bool) -> Vec<Section> {
    let panes = [
        (Pane::Editor, "editor"),
        (Pane::Result, "result"),
        (Pane::Queries, "queries"),
        (Pane::Finder, "finder"),
        (Pane::Picker, "picker"),
        (Pane::Anywhere, "anywhere"),
    ];
    let shown = panes.into_iter().filter(|&(listed, _)| every_key || listed == pane || listed == Pane::Anywhere);
    shown.map(|(listed, title)| Section { title, rows: help_rows(listed) }).collect()
}

fn help_rows(pane: Pane) -> Vec<HelpRow> {
    let hints = ROWS.iter().filter(|row| row.pane == pane).filter_map(|row| row.help);
    hints.map(|hint| HelpRow { keys: hint.keys.to_owned(), does: hint.does }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_is_listed_once() {
        for (at, act) in Act::ALL.iter().enumerate() {
            assert!(!Act::ALL[at + 1..].contains(act), "{act:?}");
        }
    }

    #[test]
    fn every_action_has_a_key() {
        for act in Act::ALL {
            assert!(ROWS.iter().any(|row| row.act == act && !row.keys.is_empty()), "{act:?}");
        }
    }

    #[test]
    fn no_key_does_two_actions_in_one_pane() {
        for pane in [Pane::Editor, Pane::Result, Pane::Queries, Pane::Finder, Pane::Picker] {
            let keys: Vec<&KeyEvent> = ROWS.iter().filter(|row| works_in(row, pane)).flat_map(|row| row.keys).collect();
            for (at, key) in keys.iter().enumerate() {
                assert!(!keys[at + 1..].contains(key), "{key:?} in {pane:?}");
            }
        }
    }

    #[test]
    fn a_letter_moves_the_result_and_types_in_the_editor() {
        assert_eq!(act_for(key(KeyCode::Char('j')), Pane::Result), Some(Act::Down));
        assert_eq!(act_for(key(KeyCode::Char('j')), Pane::Editor), None);
    }

    #[test]
    fn a_shifted_letter_is_the_capital_whether_the_terminal_says_shift_or_not() {
        assert_eq!(act_for(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT), Pane::Result), Some(Act::Bottom));
        assert_eq!(act_for(key(KeyCode::Char('G')), Pane::Result), Some(Act::Bottom));
    }

    #[test]
    fn a_modifier_makes_another_key() {
        assert_eq!(act_for(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), Pane::Editor), Some(Act::NewLine));
        assert_eq!(act_for(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL), Pane::Result), None);
    }

    #[test]
    fn the_finder_takes_its_own_keys_only() {
        assert_eq!(act_for(key(KeyCode::Esc), Pane::Finder), Some(Act::Close));
        assert_eq!(act_for(key(KeyCode::Tab), Pane::Finder), None);
        assert_eq!(act_for(control('p'), Pane::Editor), Some(Act::Find));
    }

    #[test]
    fn the_first_help_lists_the_focused_pane_and_what_works_anywhere() {
        let titles: Vec<&str> = sections(Pane::Result, false).iter().map(|section| section.title).collect();
        assert_eq!(titles, ["result", "anywhere"]);
        assert_eq!(sections(Pane::Result, true).len(), 6);
    }
}
