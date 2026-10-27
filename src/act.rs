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
    Delete,
    CloseSession,
    Add,
    Edit,
    Save,
    EditOutside,
    History,
    Detail,
    CopyCell,
    CopyRow,
    CopyResult,
    CopyCsv,
    CopyJson,
    CopyMarkdown,
}

impl Act {
    /// Whether this act works on the grid's cursor, so it does nothing while the result pane shows a chart.
    pub(crate) const fn is_grid(self) -> bool {
        matches!(
            self,
            Self::Down
                | Self::Up
                | Self::PageDown
                | Self::PageUp
                | Self::Top
                | Self::Bottom
                | Self::Left
                | Self::Right
                | Self::Detail
                | Self::CopyCell
                | Self::CopyRow
        )
    }
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
            Self::Chart => Some(Self::Delete),
            Self::Delete => Some(Self::CloseSession),
            Self::CloseSession => Some(Self::Add),
            Self::Add => Some(Self::Edit),
            Self::Edit => Some(Self::Save),
            Self::Save => Some(Self::EditOutside),
            Self::EditOutside => Some(Self::History),
            Self::History => Some(Self::Detail),
            Self::Detail => Some(Self::CopyCell),
            Self::CopyCell => Some(Self::CopyRow),
            Self::CopyRow => Some(Self::CopyResult),
            Self::CopyResult => Some(Self::CopyCsv),
            Self::CopyCsv => Some(Self::CopyJson),
            Self::CopyJson => Some(Self::CopyMarkdown),
            Self::CopyMarkdown => None,
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
    /// The form that adds or edits a connection, from the picker.
    Form,
    /// A question on the footer line, as the path of a new query: while open it takes every key.
    Prompt,
    /// The detail of the grid's row over the screen, a popup like the finder.
    Detail,
    /// A question on the footer line, the format to copy the result as: while open it takes every key.
    CopyAs,
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

/// The actions whose footer hint stays when the others no longer fit, the one kept longest first.
const FOOTER_KEPT_LONGEST: [Act; 2] = [Act::Quit, Act::Help];

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
    row(Act::Detail, Pane::Result, &[key(KeyCode::Enter), key(KeyCode::Char(' '))])
        .help("⏎ space", "the row under the cursor, one column a line")
        .footer("⏎", "row"),
    row(Act::CopyCell, Pane::Result, &[key(KeyCode::Char('y'))]).help("y", "copy the cell under the cursor").footer("y", "copy"),
    row(Act::CopyRow, Pane::Result, &[key(KeyCode::Char('Y'))]).help("Y", "copy the row as CSV"),
    row(Act::CopyResult, Pane::Result, &[key(KeyCode::Char('c'))]).help("c", "copy the result as CSV, JSON or Markdown"),
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
    row(Act::Delete, Pane::Picker, &[control('d')]).help("^d", "remove the connection from the config, once you say yes"),
    row(Act::CloseSession, Pane::Picker, &[control('x')]).help("^x", "close its session and its tunnel"),
    row(Act::Add, Pane::Picker, &[control('a')]).help("^a", "add a connection"),
    row(Act::Edit, Pane::Picker, &[control('e')]).help("^e", "edit the connection"),
    row(Act::Down, Pane::Form, &[key(KeyCode::Tab), key(KeyCode::Down)]).help("⇥ ↓", "next, previous field").footer("⇥", "next"),
    row(Act::Up, Pane::Form, &[key(KeyCode::BackTab), key(KeyCode::Up)]),
    row(Act::Save, Pane::Form, &[key(KeyCode::Enter)]).help("⏎", "save the connection").footer("⏎", "saves"),
    row(Act::Close, Pane::Form, &[key(KeyCode::Esc)]).help("esc", "leave without saving").footer("esc", "closes"),
    row(Act::Quit, Pane::Form, &[control('c')]).help("^c", "quit"),
    row(Act::Open, Pane::Prompt, &[key(KeyCode::Enter)]).help("⏎", "save, or open over the unsaved text").footer("⏎", "ok"),
    row(Act::Close, Pane::Prompt, &[key(KeyCode::Esc)]).help("esc", "keep editing").footer("esc", "cancel"),
    row(Act::Quit, Pane::Prompt, &[control('c')]).help("^c", "quit"),
    row(Act::Down, Pane::Detail, &[key(KeyCode::Char('j')), key(KeyCode::Down)]).help("j k", "scroll down, up").footer("j/k", "scroll"),
    row(Act::Up, Pane::Detail, &[key(KeyCode::Char('k')), key(KeyCode::Up)]),
    row(Act::Close, Pane::Detail, &[key(KeyCode::Esc)]).help("esc", "back to the grid").footer("esc", "closes"),
    row(Act::Quit, Pane::Detail, &[control('c')]).help("^c", "quit"),
    row(Act::CopyCsv, Pane::CopyAs, &[key(KeyCode::Char('c'))]).help("c", "copy the result as CSV").footer("c", "CSV"),
    row(Act::CopyJson, Pane::CopyAs, &[key(KeyCode::Char('j'))]).help("j", "copy it as JSON").footer("j", "JSON"),
    row(Act::CopyMarkdown, Pane::CopyAs, &[key(KeyCode::Char('m'))]).help("m", "copy it as a Markdown table").footer("m", "Markdown"),
    row(Act::Close, Pane::CopyAs, &[key(KeyCode::Esc)]).help("esc", "copy nothing").footer("esc", "cancel"),
    row(Act::Quit, Pane::CopyAs, &[control('c')]).help("^c", "quit"),
    row(Act::EditOutside, Pane::Editor, &[control('e')]).help("^e", "edit the saved query in $EDITOR"),
    row(Act::Save, Pane::Anywhere, &[control('s')]).help("^s", "save the query, naming a new one").footer("^s", "save"),
    row(Act::History, Pane::Anywhere, &[control('r')]).help("^r", "search the statements run on this connection"),
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
    row.pane == pane
        || (row.pane == Pane::Anywhere
            && !matches!(pane, Pane::Finder | Pane::Picker | Pane::Form | Pane::Prompt | Pane::Detail | Pane::CopyAs))
}

/// What `key` does while `pane` has the keys; `None` leaves it to the pane, as text for the editor.
pub(crate) fn act_for(key: KeyEvent, pane: Pane) -> Option<Act> {
    ROWS.iter().find(|row| works_in(row, pane) && row.keys.iter().any(|own| is_the_key(own, key))).map(|row| row.act)
}

/// The footer's hints while `pane` has the keys: its own, then those that work anywhere.
pub(crate) fn footer(pane: Pane) -> impl Iterator<Item = Hint> {
    footer_rows(pane).filter_map(|row| row.footer)
}

fn footer_rows(pane: Pane) -> impl Iterator<Item = &'static Row> {
    let own = ROWS.iter().filter(move |row| row.pane == pane);
    let anywhere = ROWS.iter().filter(move |row| row.pane == Pane::Anywhere && works_in(row, pane));
    own.chain(anywhere).filter(|row| row.footer.is_some())
}

/// The footer's hints for `pane` that fit in `columns`, `width` saying what each takes, in the footer's order: the
/// way out and the way to every other key are the last to go, then the others go from the end.
pub(crate) fn footer_within(pane: Pane, columns: usize, width: impl Fn(Hint) -> usize) -> Vec<Hint> {
    let rows: Vec<&Row> = footer_rows(pane).collect();
    let mut by_priority: Vec<usize> = (0..rows.len()).collect();
    by_priority
        .sort_by_key(|&index| FOOTER_KEPT_LONGEST.iter().position(|&act| act == rows[index].act).unwrap_or(FOOTER_KEPT_LONGEST.len()));
    let mut left = columns;
    let mut kept = vec![false; rows.len()];
    for index in by_priority {
        let taken = rows[index].footer.map_or(0, &width);
        if taken > left {
            break;
        }
        left -= taken;
        kept[index] = true;
    }
    rows.iter().zip(kept).filter(|(_, is_kept)| *is_kept).filter_map(|(row, _)| row.footer).collect()
}

/// The `?` help: the keys of `pane` and those that work anywhere, or every pane's with `every_key`.
pub(crate) fn sections(pane: Pane, every_key: bool) -> Vec<Section> {
    let panes = [
        (Pane::Editor, "editor"),
        (Pane::Result, "result"),
        (Pane::Queries, "queries"),
        (Pane::Finder, "finder"),
        (Pane::Picker, "picker"),
        (Pane::Form, "form"),
        (Pane::Prompt, "prompt"),
        (Pane::Detail, "row"),
        (Pane::CopyAs, "copy as"),
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

    fn hint_width(hint: Hint) -> usize {
        hint.keys.chars().count() + hint.does.chars().count() + 2
    }

    #[test]
    fn a_narrow_footer_keeps_quit_and_the_keys_first() {
        let kept: Vec<&str> = footer_within(Pane::Editor, 16, hint_width).iter().map(|hint| hint.does).collect();
        assert_eq!(kept, ["keys", "quit"]);
        let kept: Vec<&str> = footer_within(Pane::Editor, 8, hint_width).iter().map(|hint| hint.does).collect();
        assert_eq!(kept, ["quit"]);
    }

    #[test]
    fn a_wide_footer_keeps_every_hint_in_order() {
        assert_eq!(footer_within(Pane::Editor, 500, hint_width), footer(Pane::Editor).collect::<Vec<_>>());
    }

    #[test]
    fn copying_the_whole_result_works_on_a_chart_too() {
        assert!(Act::CopyCell.is_grid() && !Act::CopyResult.is_grid());
    }

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
        for pane in
            [Pane::Editor, Pane::Result, Pane::Queries, Pane::Finder, Pane::Picker, Pane::Form, Pane::Prompt, Pane::Detail, Pane::CopyAs]
        {
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
        assert_eq!(sections(Pane::Result, true).len(), 10);
    }
}
