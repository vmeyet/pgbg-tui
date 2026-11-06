//! Every action of the TUI in one table: its default keys, the pane they work in, its line in the `?` help and its
//! hint in the footer.
//! The key handling, the help and the footer all read it from here.
use crate::chart::ChartNext;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vtui::help::{Row as HelpRow, Section};
use vtui::usage::Group as UsageGroup;

/// One action; its row in [`ROWS`] says everything else about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(strum::EnumIter))]
pub(crate) enum Act {
    Run,
    Explain,
    ExplainAnalyze,
    NewLine,
    Cancel,
    SwitchPane,
    SwitchPaneBack,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
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
    ChartKind,
    ChartFlip,
    StepFormat,
    SwitchFormats,
    Delete,
    DeleteQuery,
    CloseSession,
    Add,
    Edit,
    Save,
    SaveAs,
    NewQuery,
    EditOutside,
    History,
    Versions,
    Detail,
    WhereEqual,
    WhereNotEqual,
    ColumnStats,
    JsonTree,
    CopyCell,
    CopyRow,
    CopyResult,
    CopyCsv,
    CopyJson,
    CopyMarkdown,
    CopyStatement,
    CopySelection,
    CopyPath,
    Paste,
    SwitchTab,
    Filter,
    Sort,
    SwitchFreeze,
    FreezeThrough,
    HideColumn,
    ShowColumns,
    ReadAgain,
    TakeGhost,
    DropGhost,
    Complete,
    Undo,
    Redo,
    Preview,
    Insert,
    FindTable,
    Ask,
    Back,
    Commands,
}

impl Act {
    /// What `pgbg usage` and the usage file call it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Explain => "explain",
            Self::ExplainAnalyze => "explain_analyze",
            Self::NewLine => "new_line",
            Self::Cancel => "cancel",
            Self::SwitchPane => "switch_pane",
            Self::SwitchPaneBack => "switch_pane_back",
            Self::FocusLeft => "focus_left",
            Self::FocusRight => "focus_right",
            Self::FocusUp => "focus_up",
            Self::FocusDown => "focus_down",
            Self::Help => "help",
            Self::Connections => "connections",
            Self::Quit => "quit",
            Self::Down => "down",
            Self::Up => "up",
            Self::PageDown => "page_down",
            Self::PageUp => "page_up",
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Right => "right",
            Self::Find => "find",
            Self::Open => "open",
            Self::Close => "close",
            Self::Chart => "chart",
            Self::ChartKind => "chart_kind",
            Self::ChartFlip => "chart_flip",
            Self::StepFormat => "step_format",
            Self::SwitchFormats => "switch_formats",
            Self::Delete => "delete",
            Self::DeleteQuery => "delete_query",
            Self::CloseSession => "close_session",
            Self::Add => "add",
            Self::Edit => "edit",
            Self::Save => "save",
            Self::SaveAs => "save_as",
            Self::NewQuery => "new_query",
            Self::EditOutside => "edit_outside",
            Self::History => "history",
            Self::Versions => "versions",
            Self::Detail => "detail",
            Self::WhereEqual => "where_equal",
            Self::WhereNotEqual => "where_not_equal",
            Self::ColumnStats => "column_stats",
            Self::JsonTree => "json_tree",
            Self::CopyCell => "copy_cell",
            Self::CopyRow => "copy_row",
            Self::CopyResult => "copy_result",
            Self::CopyCsv => "copy_csv",
            Self::CopyJson => "copy_json",
            Self::CopyMarkdown => "copy_markdown",
            Self::CopyStatement => "copy_statement",
            Self::CopySelection => "copy_selection",
            Self::CopyPath => "copy_path",
            Self::Paste => "paste",
            Self::SwitchTab => "switch_tab",
            Self::Filter => "filter",
            Self::Sort => "sort",
            Self::SwitchFreeze => "switch_freeze",
            Self::FreezeThrough => "freeze_through",
            Self::HideColumn => "hide_column",
            Self::ShowColumns => "show_columns",
            Self::ReadAgain => "read_again",
            Self::TakeGhost => "take_ghost",
            Self::DropGhost => "drop_ghost",
            Self::Complete => "complete",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Preview => "preview",
            Self::Insert => "insert",
            Self::FindTable => "find_table",
            Self::Ask => "ask",
            Self::Back => "back",
            Self::Commands => "commands",
        }
    }

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
                | Self::WhereEqual
                | Self::WhereNotEqual
                | Self::ColumnStats
                | Self::JsonTree
                | Self::CopyCell
                | Self::CopyRow
                | Self::StepFormat
                | Self::SwitchFormats
                | Self::Filter
                | Self::Sort
                | Self::SwitchFreeze
                | Self::FreezeThrough
                | Self::HideColumn
                | Self::ShowColumns
        )
    }

    /// Whether this act works on the chart, so its hint shows only while the result pane shows one.
    const fn is_chart(self) -> bool {
        matches!(self, Self::ChartKind | Self::ChartFlip)
    }

    /// Whether this act works on the ghost, so its key does what it does around the editor while no ghost shows.
    pub(crate) const fn is_ghost(self) -> bool {
        matches!(self, Self::TakeGhost | Self::DropGhost)
    }
}

/// Where an action's keys work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pane {
    /// Whichever pane has the keys, before that pane's own keys.
    Anywhere,
    Editor,
    /// The parameter bar above the editor.
    Params,
    Result,
    /// The result pane while it shows a plan: its nodes fold, and the grid's keys have no cell to work on.
    Plan,
    /// The saved queries in the sidebar.
    Queries,
    /// The schema in the sidebar, its other tab.
    Schema,
    /// The line that filters the schema's tables: while open it takes every key.
    SchemaFilter,
    /// The footer line that filters the grid's rows: while open it takes every key.
    RowFilter,
    /// The `ctrl-t` finder over every table of the schema, a popup like the `ctrl-p` one.
    TableFinder,
    /// The `ctrl-p` finder over every saved query: while open it takes every key, and only its own work.
    Finder,
    /// The `ctrl-l` command palette, a popup like the finder.
    Commands,
    /// The `ctrl-o` connection picker, a popup like the finder.
    Picker,
    /// The form that adds or edits a connection, from the picker.
    Form,
    /// A question on the footer line, as the path of a new query: while open it takes every key.
    Prompt,
    /// A question on the footer line, whether to send a write: `y` sends it, any other key drops it.
    Confirm,
    /// A question on the footer line, whether to delete a saved query: `y` deletes it, any other key keeps it.
    ConfirmDelete,
    /// The detail of the grid's row over the screen, a popup like the finder.
    Detail,
    /// The figures of the grid's column over the screen, a popup like the row's detail.
    ColumnStats,
    /// The JSON of the grid's cell as a tree that folds, a popup like the row's detail.
    JsonTree,
    /// A question on the footer line, the format to copy the result as: while open it takes every key.
    CopyAs,
    /// The `ctrl-space` list of names at the cursor: while open it takes every key.
    Completion,
    /// The `ctrl-g` instruction on the footer line: while open it takes every key.
    Instruction,
    /// An ask waiting on the `[ai] command`: while it waits it takes every key.
    Asking,
    /// The `[ai] command`'s answer: while shown it takes every key.
    Answer,
}

impl Pane {
    /// The order the `?` help lists the panes in.
    const IN_HELP: [Self; 25] = [
        Self::Editor,
        Self::Params,
        Self::Result,
        Self::Plan,
        Self::Queries,
        Self::Schema,
        Self::SchemaFilter,
        Self::RowFilter,
        Self::Finder,
        Self::TableFinder,
        Self::Commands,
        Self::Picker,
        Self::Form,
        Self::Prompt,
        Self::Confirm,
        Self::ConfirmDelete,
        Self::Detail,
        Self::ColumnStats,
        Self::JsonTree,
        Self::CopyAs,
        Self::Completion,
        Self::Instruction,
        Self::Asking,
        Self::Answer,
        Self::Anywhere,
    ];

    /// Its heading in the `?` help, and its name in `pgbg usage`.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Editor => "editor",
            Self::Params => "parameters",
            Self::Result => "result",
            Self::Plan => "plan",
            Self::Queries => "queries",
            Self::Schema => "schema",
            Self::SchemaFilter => "schema filter",
            Self::RowFilter => "row filter",
            Self::Finder => "finder",
            Self::TableFinder => "table finder",
            Self::Commands => "commands",
            Self::Picker => "picker",
            Self::Form => "form",
            Self::Prompt => "prompt",
            Self::Confirm => "write",
            Self::ConfirmDelete => "delete",
            Self::Detail => "row",
            Self::ColumnStats => "column",
            Self::JsonTree => "json",
            Self::CopyAs => "copy as",
            Self::Completion => "names",
            Self::Instruction => "ask",
            Self::Asking => "asking",
            Self::Answer => "answer",
            Self::Anywhere => "anywhere",
        }
    }
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

/// `⌥` and a letter, which a macOS terminal sends only with its option key set as meta.
const fn alt(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
}

/// `⌘` and a letter, which only a terminal that speaks the kitty keyboard protocol reports.
const fn command(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::SUPER)
}

const fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
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

/// The `?` help's line in the panes where `?` types nothing.
const HELP_KEYS: &str = "these keys; again for every key";

/// The command palette's line in the panes where `:` types nothing.
const COMMANDS_KEY: &str = "every command, as ^l";
const FIND_KEY: &str = "find a saved query, as ^p";

/// The keys that switch the sidebar's tab: `←` `→` for keyboards where `[` `]` take a modifier, as AZERTY.
const SWITCH_TAB: &[KeyEvent] = &[key(KeyCode::Left), key(KeyCode::Right), key(KeyCode::Char('[')), key(KeyCode::Char(']'))];

/// The actions whose footer hint stays when the others no longer fit, the one kept longest first.
const FOOTER_KEPT_LONGEST: [Act; 5] = [Act::Quit, Act::Help, Act::Close, Act::ChartKind, Act::ChartFlip];

/// Every action, in the order the help and the footer list them; where two rows share a key in a pane, the first wins.
pub(crate) const ROWS: &[Row] = &[
    row(Act::Run, Pane::Editor, &[key(KeyCode::Enter)]).help("⏎", "run the statement under the cursor").footer("⏎", "run"),
    row(Act::Explain, Pane::Editor, &[control('b'), alt('e')]).help("^b ⌥e", "the plan, without running the statement"),
    row(Act::ExplainAnalyze, Pane::Editor, &[control('f'), alt('a')]).help("^f ⌥a", "the plan as it ran: a read runs, read only and rolled back"),
    row(
        Act::NewLine,
        Pane::Editor,
        &[KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)],
    )
    .help("⌥⏎ ⇧⏎", "break the line")
    .footer("⌥⏎", "new line"),
    row(Act::TakeGhost, Pane::Editor, &[key(KeyCode::Right), key(KeyCode::Tab)]).help("→ ⇥", "take the faded rest after the cursor"),
    row(Act::DropGhost, Pane::Editor, &[key(KeyCode::Esc)]).help("esc", "drop the faded rest"),
    row(Act::Complete, Pane::Editor, &[control(' '), control('n')]).help("^space ^n", "list the names that fit here"),
    row(Act::Ask, Pane::Editor, &[control('g')]).help("^g", "ask the [ai] command for SQL"),
    row(Act::Undo, Pane::Editor, &[control('z')]).help("^z", "undo the last change"),
    row(Act::Redo, Pane::Editor, &[control('Z'), control('y')]).help("⇧^z ^y", "redo the change undone"),
    row(Act::CopyStatement, Pane::Editor, &[key(KeyCode::F(2)), alt('c')]).help("F2 ⌥c", "copy the statement under the cursor"),
    row(Act::CopySelection, Pane::Editor, &[command('c')]).help("⌘c", "copy the selection, else the statement"),
    row(Act::Paste, Pane::Editor, &[command('v')]).help("⌘v", "paste the clipboard"),
    row(Act::Run, Pane::Params, &[key(KeyCode::Enter)]).help("⏎", "run the statement with these values").footer("⏎", "run"),
    row(Act::Down, Pane::Params, &[key(KeyCode::Tab)]).help("⇥ ⇧⇥", "next, previous parameter").footer("⇥", "next"),
    row(Act::Up, Pane::Params, &[key(KeyCode::BackTab)]),
    row(Act::Cancel, Pane::Params, &[key(KeyCode::Esc)]).help("esc", "cancel the running statement"),
    row(Act::Close, Pane::Params, &[key(KeyCode::Esc)]).help("esc", "back to the editor, when nothing runs").footer("esc", "editor"),
    row(Act::Ask, Pane::Result, &[control('g')]).help("^g", "ask the [ai] command, with the columns of this result"),
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
    row(Act::ChartKind, Pane::Result, &[key(KeyCode::Char('t'))])
        .help("t", "on a chart: its next kind, grouped, stacked or line")
        .footer("t", ""),
    row(Act::ChartFlip, Pane::Result, &[key(KeyCode::Char('r'))])
        .help("r", "on a chart: its bars upright, or on their side")
        .footer("r", ""),
    row(Act::StepFormat, Pane::Result, &[key(KeyCode::Char('f'))])
        .help("f", "the column in its next format: raw, then readable ones")
        .footer("f", "format"),
    row(Act::SwitchFormats, Pane::Result, &[key(KeyCode::Char('F'))]).help("F", "every column raw, or readable"),
    row(Act::Filter, Pane::Result, &[key(KeyCode::Char('/'))])
        .help("/", "keep the rows loaded with a cell holding what you type")
        .footer("/", "filter"),
    row(Act::Sort, Pane::Result, &[key(KeyCode::Char('s'))]).help("s", "sort the rows loaded by the column: up, down, then as read"),
    row(Act::SwitchFreeze, Pane::Result, &[key(KeyCode::Char('z'))])
        .help("z Z", "keep the leading primary key, or the first column, on the left while the grid scrolls, or nothing; keep the columns up to the cursor's"),
    row(Act::FreezeThrough, Pane::Result, &[key(KeyCode::Char('Z'))]),
    row(Act::HideColumn, Pane::Result, &[key(KeyCode::Char('x'))]).help("x X", "hide the column from the grid; show every column again"),
    row(Act::ShowColumns, Pane::Result, &[key(KeyCode::Char('X'))]),
    row(Act::WhereEqual, Pane::Result, &[key(KeyCode::Char('='))])
        .help("= !", "add WHERE column = the cell's value, or <>, to the statement under the editor's cursor"),
    row(Act::WhereNotEqual, Pane::Result, &[key(KeyCode::Char('!'))]),
    row(Act::ColumnStats, Pane::Result, &[key(KeyCode::Char('i'))])
        .help("i", "the column's distinct values, NULLs, least, greatest and most common, over the rows loaded"),
    row(Act::JsonTree, Pane::Result, &[key(KeyCode::Char('o'))]).help("o", "the JSON cell as a tree that folds"),
    row(Act::Help, Pane::Result, &[key(KeyCode::Char('?'))]).help("?", HELP_KEYS),
    row(Act::Down, Pane::Plan, &[key(KeyCode::Char('j')), key(KeyCode::Down)]).help("j k", "next, previous node").footer("j/k", "nodes"),
    row(Act::Up, Pane::Plan, &[key(KeyCode::Char('k')), key(KeyCode::Up)]),
    row(Act::Open, Pane::Plan, &[key(KeyCode::Enter), key(KeyCode::Char(' '))])
        .help("⏎ space", "fold or unfold; on a node that does not fold, its parent")
        .footer("⏎", "fold"),
    row(Act::Chart, Pane::Plan, &[key(KeyCode::Char('v'))])
        .help("v", "the plan's JSON in the grid; there, the tree again")
        .footer("v", "grid"),
    row(Act::Help, Pane::Plan, &[key(KeyCode::Char('?'))]).help("?", HELP_KEYS),
    row(Act::Down, Pane::Queries, &[key(KeyCode::Char('j')), key(KeyCode::Down)]).help("j k", "next, previous query").footer("j/k", "move"),
    row(Act::Up, Pane::Queries, &[key(KeyCode::Char('k')), key(KeyCode::Up)]),
    row(Act::Open, Pane::Queries, &[key(KeyCode::Enter)]).help("⏎", "open the query, fold the folder").footer("⏎", "open"),
    row(Act::DeleteQuery, Pane::Queries, &[key(KeyCode::Char('d'))]).help("d", "delete the query, once you say yes").footer("d", "delete"),
    row(Act::SwitchTab, Pane::Queries, SWITCH_TAB).help("← → [ ]", "the schema instead").footer("←→", "schema"),
    row(Act::Help, Pane::Queries, &[key(KeyCode::Char('?'))]).help("?", HELP_KEYS),
    row(Act::Down, Pane::Schema, &[key(KeyCode::Char('j')), key(KeyCode::Down)]).help("j k", "next, previous table").footer("j/k", "move"),
    row(Act::Up, Pane::Schema, &[key(KeyCode::Char('k')), key(KeyCode::Up)]),
    row(Act::Open, Pane::Schema, &[key(KeyCode::Enter)])
        .help("⏎", "fold the schema, or write SELECT * FROM the table LIMIT 100 in the editor and run it")
        .footer("⏎", "rows"),
    row(Act::Preview, Pane::Schema, &[key(KeyCode::Char('p'))]).help("p", "the table's rows, as ⏎"),
    row(Act::Detail, Pane::Schema, &[key(KeyCode::Char('d'))]).help("d", "the table's columns and indexes").footer("d", "columns"),
    row(Act::Insert, Pane::Schema, &[key(KeyCode::Char('i'))]).help("i", "put the table's name in the editor").footer("i", "insert"),
    row(Act::Filter, Pane::Schema, &[key(KeyCode::Char('/'))])
        .help("/", "keep the tables whose name holds what you type")
        .footer("/", "filter"),
    row(Act::ReadAgain, Pane::Schema, &[key(KeyCode::Char('r'))]).help("r", "read the schema again"),
    row(Act::SwitchTab, Pane::Schema, SWITCH_TAB).help("← → [ ]", "the queries instead").footer("←→", "queries"),
    row(Act::Help, Pane::Schema, &[key(KeyCode::Char('?'))]).help("?", HELP_KEYS),
    row(Act::Open, Pane::SchemaFilter, &[key(KeyCode::Enter)]).help("⏎", "keep the filter").footer("⏎", "keep"),
    row(Act::Close, Pane::SchemaFilter, &[key(KeyCode::Esc)]).help("esc", "drop the filter").footer("esc", "drop"),
    row(Act::Quit, Pane::SchemaFilter, &[control('c')]).help("^c", "quit"),
    row(Act::Open, Pane::RowFilter, &[key(KeyCode::Enter)]).help("⏎", "keep the filter").footer("⏎", "keep"),
    row(Act::Close, Pane::RowFilter, &[key(KeyCode::Esc)]).help("esc", "drop the filter: every row shows again").footer("esc", "drop"),
    row(Act::Quit, Pane::RowFilter, &[control('c')]).help("^c", "quit"),
    row(Act::Down, Pane::Finder, &[key(KeyCode::Down), control('n')]).help("↓ ^n", "next, previous query").footer("↑↓", "pick"),
    row(Act::Up, Pane::Finder, &[key(KeyCode::Up), control('p')]),
    row(Act::Open, Pane::Finder, &[key(KeyCode::Enter)]).help("⏎", "open it in the editor").footer("⏎", "opens"),
    row(Act::Quit, Pane::Finder, &[control('c')]).help("^c", "quit"),
    row(Act::Close, Pane::Finder, &[key(KeyCode::Esc)]).help("esc", "close the finder").footer("esc", "closes"),
    row(Act::Find, Pane::Anywhere, &[control('p'), command('k')]).help("^p ⌘k", "find a saved query").footer("^p", "find"),
    row(Act::FindTable, Pane::Anywhere, &[control('t')]).help("^t", "find a table of the schema"),
    row(Act::Open, Pane::TableFinder, &[key(KeyCode::Enter)])
        .help("⏎", "write SELECT * FROM it LIMIT 100 in the editor and run it")
        .footer("⏎", "rows"),
    row(Act::Preview, Pane::TableFinder, &[control('r')]).help("^r", "its rows, as ⏎"),
    row(Act::Detail, Pane::TableFinder, &[control('d')]).help("^d", "its columns and indexes").footer("^d", "columns"),
    row(Act::Insert, Pane::TableFinder, &[key(KeyCode::Tab)]).help("⇥", "put its name in the editor").footer("⇥", "insert"),
    row(Act::Down, Pane::TableFinder, &[key(KeyCode::Down), control('n')]).help("↓ ^n", "next, previous table").footer("↑↓", "pick"),
    row(Act::Up, Pane::TableFinder, &[key(KeyCode::Up), control('p')]),
    row(Act::Close, Pane::TableFinder, &[key(KeyCode::Esc)]).help("esc", "close the finder").footer("esc", "closes"),
    row(Act::Quit, Pane::TableFinder, &[control('c')]).help("^c", "quit"),
    row(Act::Commands, Pane::Anywhere, &[control('l')]).help("^l", "every command and theme, filtered as you type"),
    row(Act::Commands, Pane::Result, &[key(KeyCode::Char(':'))]).help(":", COMMANDS_KEY),
    row(Act::Commands, Pane::Queries, &[key(KeyCode::Char(':'))]).help(":", COMMANDS_KEY),
    row(Act::Commands, Pane::Schema, &[key(KeyCode::Char(':'))]).help(":", COMMANDS_KEY),
    row(Act::Find, Pane::Result, &[control('k')]).help("^k", FIND_KEY),
    row(Act::Find, Pane::Queries, &[control('k')]).help("^k", FIND_KEY),
    row(Act::Find, Pane::Schema, &[control('k')]).help("^k", FIND_KEY),
    row(Act::Find, Pane::Plan, &[control('k')]).help("^k", FIND_KEY),
    row(Act::Down, Pane::Commands, &[key(KeyCode::Down), control('n')]).help("↓ ^n", "next, previous command").footer("↑↓", "pick"),
    row(Act::Up, Pane::Commands, &[key(KeyCode::Up), control('p')]),
    row(Act::Complete, Pane::Commands, &[key(KeyCode::Tab), key(KeyCode::BackTab)])
        .help("⇥ ⇧⇥", "complete a typed command, as :set theme=nord")
        .footer("⇥", "completes"),
    row(Act::TakeGhost, Pane::Commands, &[key(KeyCode::Right), key(KeyCode::End)]).help("→", "take the faded rest of a typed command"),
    row(Act::Open, Pane::Commands, &[key(KeyCode::Enter)]).help("⏎", "run it, or keep the theme").footer("⏎", "runs"),
    row(Act::Close, Pane::Commands, &[key(KeyCode::Esc), control('l')])
        .help("esc ^l", "close it, with the theme as it was")
        .footer("esc", "closes"),
    row(Act::Quit, Pane::Commands, &[control('c')]).help("^c", "quit"),
    row(Act::Cancel, Pane::Anywhere, &[key(KeyCode::Esc)]).help("esc", "cancel the running statement"),
    row(Act::SwitchPane, Pane::Anywhere, &[key(KeyCode::Tab)]).help("⇥ ⇧⇥", "next, previous pane").footer("⇥", "pane"),
    row(Act::SwitchPaneBack, Pane::Anywhere, &[key(KeyCode::BackTab)]),
    row(Act::FocusLeft, Pane::Anywhere, &[shift(KeyCode::Left)]).help("⇧←→↑↓", "the pane on that side"),
    row(Act::FocusRight, Pane::Anywhere, &[shift(KeyCode::Right)]),
    row(Act::FocusUp, Pane::Anywhere, &[shift(KeyCode::Up)]),
    row(Act::FocusDown, Pane::Anywhere, &[shift(KeyCode::Down)]),
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
    row(Act::Run, Pane::Confirm, &[key(KeyCode::Char('y'))]).help("y", "send the write").footer("y", "sends"),
    row(Act::Close, Pane::Confirm, &[key(KeyCode::Esc)]).help("esc", "send nothing, as any other key").footer("any other key", "cancels"),
    row(Act::Quit, Pane::Confirm, &[control('c')]).help("^c", "quit, sending nothing"),
    row(Act::DeleteQuery, Pane::ConfirmDelete, &[key(KeyCode::Char('y'))]).help("y", "delete the query").footer("y", "deletes"),
    row(Act::Close, Pane::ConfirmDelete, &[key(KeyCode::Esc)])
        .help("esc", "keep the query, as any other key")
        .footer("any other key", "keeps"),
    row(Act::Quit, Pane::ConfirmDelete, &[control('c')]).help("^c", "quit, deleting nothing"),
    row(Act::Down, Pane::Detail, &[key(KeyCode::Char('j')), key(KeyCode::Down)])
        .help("j k", "next, previous value, line by line through one too tall to fit")
        .footer("j/k", "values"),
    row(Act::Up, Pane::Detail, &[key(KeyCode::Char('k')), key(KeyCode::Up)]),
    row(Act::CopyCell, Pane::Detail, &[key(KeyCode::Char('y'))]).help("y", "copy the value under the cursor; a double click does too").footer("y", "copy"),
    row(Act::CopyRow, Pane::Detail, &[key(KeyCode::Char('Y'))]).help("Y", "copy the row as CSV"),
    row(Act::Right, Pane::Detail, &[key(KeyCode::Tab)]).help("⇥ ⇧⇥", "next, previous key or table pointing here").footer("⇥", "keys"),
    row(Act::Left, Pane::Detail, &[key(KeyCode::BackTab)]),
    row(Act::Open, Pane::Detail, &[key(KeyCode::Enter)])
        .help("⏎", "under the cursor: the row its key points to, or the rows pointing here")
        .footer("⏎", "follow"),
    row(Act::Edit, Pane::Detail, &[key(KeyCode::Char('e'))]).help("e", "write the join of the trail into the editor").footer("e", "join"),
    row(Act::Back, Pane::Detail, &[key(KeyCode::Backspace)]).help("⌫", "back to the row before").footer("⌫", "back"),
    row(Act::Close, Pane::Detail, &[key(KeyCode::Esc)]).help("esc", "back to the grid").footer("esc", "closes"),
    row(Act::Quit, Pane::Detail, &[control('c')]).help("^c", "quit"),
    row(Act::Close, Pane::ColumnStats, &[key(KeyCode::Esc), key(KeyCode::Char('i'))])
        .help("esc i", "back to the grid")
        .footer("esc", "closes"),
    row(Act::Quit, Pane::ColumnStats, &[control('c')]).help("^c", "quit"),
    row(Act::Down, Pane::JsonTree, &[key(KeyCode::Char('j')), key(KeyCode::Down)]).help("j k", "next, previous line"),
    row(Act::Up, Pane::JsonTree, &[key(KeyCode::Char('k')), key(KeyCode::Up)]),
    row(Act::Open, Pane::JsonTree, &[key(KeyCode::Enter), key(KeyCode::Char(' '))])
        .help("⏎ space", "fold or unfold; on a value that does not fold, the one holding it")
        .footer("⏎", "fold"),
    row(Act::CopyCell, Pane::JsonTree, &[key(KeyCode::Char('y'))]).help("y", "copy the value, a string unquoted").footer("y", "value"),
    row(Act::CopyPath, Pane::JsonTree, &[key(KeyCode::Char('Y'))]).help("Y", "copy the path, as jq writes it").footer("Y", "path"),
    row(Act::Close, Pane::JsonTree, &[key(KeyCode::Esc), key(KeyCode::Char('o'))])
        .help("esc o", "back to the grid")
        .footer("esc", "closes"),
    row(Act::Quit, Pane::JsonTree, &[control('c')]).help("^c", "quit"),
    row(Act::Down, Pane::Completion, &[key(KeyCode::Down), control('n')]).help("↓ ^n", "next, previous name").footer("↑↓", "pick"),
    row(Act::Up, Pane::Completion, &[key(KeyCode::Up), control('p')]),
    row(Act::Open, Pane::Completion, &[key(KeyCode::Enter), key(KeyCode::Tab)]).help("⏎ ⇥", "take the name").footer("⏎", "takes"),
    row(Act::Close, Pane::Completion, &[key(KeyCode::Esc)]).help("esc", "close the list").footer("esc", "closes"),
    row(Act::Quit, Pane::Completion, &[control('c')]).help("^c", "quit"),
    row(Act::Open, Pane::Instruction, &[key(KeyCode::Enter)]).help("⏎", "send the instruction").footer("⏎", "ask"),
    row(Act::Close, Pane::Instruction, &[key(KeyCode::Esc)]).help("esc", "ask nothing").footer("esc", "cancel"),
    row(Act::Quit, Pane::Instruction, &[control('c')]).help("^c", "quit"),
    row(Act::Close, Pane::Asking, &[key(KeyCode::Esc)]).help("esc", "stop the command").footer("esc", "stop"),
    row(Act::Quit, Pane::Asking, &[control('c')]).help("^c", "quit"),
    row(Act::Insert, Pane::Answer, &[key(KeyCode::Tab)])
        .help("⇥", "put the answer in place of the statement, to run with ⏎")
        .footer("⇥", "take"),
    row(Act::Close, Pane::Answer, &[key(KeyCode::Esc)]).help("esc", "drop the answer").footer("esc", "drop"),
    row(Act::Quit, Pane::Answer, &[control('c')]).help("^c", "quit"),
    row(Act::CopyCsv, Pane::CopyAs, &[key(KeyCode::Char('c'))]).help("c", "copy the result as CSV").footer("c", "CSV"),
    row(Act::CopyJson, Pane::CopyAs, &[key(KeyCode::Char('j'))]).help("j", "copy it as JSON").footer("j", "JSON"),
    row(Act::CopyMarkdown, Pane::CopyAs, &[key(KeyCode::Char('m'))]).help("m", "copy it as a Markdown table").footer("m", "Markdown"),
    row(Act::Close, Pane::CopyAs, &[key(KeyCode::Esc)]).help("esc", "copy nothing").footer("esc", "cancel"),
    row(Act::Quit, Pane::CopyAs, &[control('c')]).help("^c", "quit"),
    row(Act::EditOutside, Pane::Editor, &[control('x')]).help("^x", "edit the saved query in $EDITOR"),
    row(Act::Save, Pane::Anywhere, &[control('s')]).help("^s", "save the query, naming a new one").footer("^s", "save"),
    row(Act::SaveAs, Pane::Anywhere, &[control('d'), alt('s')]).help("^d ⌥s", "save the text under a new path, the open query left as it is"),
    row(Act::NewQuery, Pane::Anywhere, &[control('q')]).help("^q", "a new, empty query"),
    row(Act::History, Pane::Anywhere, &[control('r')]).help("^r", "search the statements run on this connection"),
    row(Act::Versions, Pane::Anywhere, &[control('v')]).help("^v", "the versions of the saved query its saves replaced"),
    row(Act::Connections, Pane::Anywhere, &[control('o')]).help("^o", "pick a connection").footer("^o", "connections"),
    row(Act::Quit, Pane::Anywhere, &[control('c')]).help("^c", "quit").footer("^c", "quit"),
];

/// The editor's keys that move the cursor or delete as in a shell: text keys, not acts, so listed in its help only.
const EDITING_KEYS: &[Hint] = &[
    Hint { keys: "⌥←→ ⌥b ⌥f", does: "a word left, right" },
    Hint { keys: "^a ^e", does: "start, end of the line" },
    Hint { keys: "^k ^u", does: "delete to the line's end, start" },
    Hint { keys: "^w ⌥⌫", does: "delete the word before the cursor" },
];

/// Whether `pressed` is `own`; a typed character is its own shift, so `G` matches with or without it, and a shifted
/// `z`, as `⇧^z` comes, is `Z`.
fn is_the_key(own: &KeyEvent, pressed: KeyEvent) -> bool {
    let is_shifted = pressed.modifiers.contains(KeyModifiers::SHIFT);
    let (code, modifiers) = match pressed.code {
        KeyCode::Char(c) if is_shifted => (KeyCode::Char(c.to_ascii_uppercase()), pressed.modifiers - KeyModifiers::SHIFT),
        KeyCode::BackTab => (pressed.code, pressed.modifiers - KeyModifiers::SHIFT),
        code => (code, pressed.modifiers),
    };
    own.code == code && own.modifiers == modifiers
}

fn works_in(row: &Row, pane: Pane) -> bool {
    row.pane == pane
        || (row.pane == Pane::Anywhere
            && !matches!(
                pane,
                Pane::Finder
                    | Pane::Commands
                    | Pane::TableFinder
                    | Pane::Picker
                    | Pane::Form
                    | Pane::Prompt
                    | Pane::Confirm
                    | Pane::ConfirmDelete
                    | Pane::Detail
                    | Pane::ColumnStats
                    | Pane::JsonTree
                    | Pane::CopyAs
                    | Pane::SchemaFilter
                    | Pane::RowFilter
                    | Pane::Completion
                    | Pane::Instruction
                    | Pane::Asking
                    | Pane::Answer
            ))
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
    rows_with_a_hint(pane).filter(|row| !row.act.is_chart())
}

fn rows_with_a_hint(pane: Pane) -> impl Iterator<Item = &'static Row> {
    let own = ROWS.iter().filter(move |row| row.pane == pane);
    let anywhere = ROWS.iter().filter(move |row| row.pane == Pane::Anywhere && works_in(row, pane));
    own.chain(anywhere).filter(|row| row.footer.is_some())
}

/// The footer's hints for `pane` that fit in `columns`, in the footer's order; `width` says what each hint takes.
/// The way out and the way to every other key are the last to go; the others go from the end.
pub(crate) fn footer_within(pane: Pane, columns: usize, width: impl Fn(Hint) -> usize) -> Vec<Hint> {
    fit_hints(&footer_rows(pane).copied().collect::<Vec<_>>(), columns, width)
}

/// The footer's hints while the result pane has the keys and shows a chart: the chart's keys, each naming where `next`
/// says it leads and left out where it leads nowhere, without those of the hidden grid.
pub(crate) fn chart_footer_within(next: ChartNext, columns: usize, width: impl Fn(Hint) -> usize) -> Vec<Hint> {
    let rows: Vec<Row> = rows_with_a_hint(Pane::Result).filter(|row| !row.act.is_grid()).filter_map(|row| with_next(row, next)).collect();
    fit_hints(&rows, columns, width)
}

fn with_next(row: &Row, next: ChartNext) -> Option<Row> {
    let does = match row.act {
        Act::ChartKind => next.kind?.name(),
        Act::ChartFlip => next.orientation?.shown_name(),
        Act::Chart => "grid",
        _ => return Some(*row),
    };
    Some(Row { footer: row.footer.map(|hint| Hint { does, ..hint }), ..*row })
}

fn fit_hints(rows: &[Row], columns: usize, width: impl Fn(Hint) -> usize) -> Vec<Hint> {
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
    let shown = Pane::IN_HELP.into_iter().filter(|&listed| every_key || listed == pane || listed == Pane::Anywhere);
    shown.map(|listed| Section { title: listed.title(), rows: help_rows(listed) }).collect()
}

/// Every act's name, grouped under the pane of its first row, panes in the order of the help; what `pgbg usage` lists
/// when unused.
pub(crate) fn usage_groups() -> Vec<UsageGroup> {
    let first: Vec<&Row> = first_rows().collect();
    let names_in = |pane: Pane| first.iter().filter(|row| row.pane == pane).map(|row| row.act.name()).collect();
    Pane::IN_HELP.into_iter().map(|pane| UsageGroup { title: pane.title(), actions: names_in(pane) }).collect()
}

/// The first row of each act.
fn first_rows() -> impl Iterator<Item = &'static Row> {
    ROWS.iter().enumerate().filter(|(at, row)| !ROWS[..*at].iter().any(|before| before.act == row.act)).map(|(_, row)| row)
}

/// One act as the command palette lists it: from the row that works in the pane it opened on, else from its first row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Listing {
    pub(crate) act: Act,
    pub(crate) keys: String,
    pub(crate) does: String,
    /// The pane of that row.
    pub(crate) pane: Pane,
    /// Whether it works in the pane the palette opened on; a ghost act never does, as opening the palette drops the ghost.
    pub(crate) runs: bool,
}

/// Every act once, as the palette opened on `pane` lists it.
pub(crate) fn listings_from(pane: Pane) -> impl Iterator<Item = Listing> {
    first_rows().map(move |first| listing_of(first, pane))
}

fn listing_of(first: &'static Row, pane: Pane) -> Listing {
    let here = ROWS.iter().find(|row| row.act == first.act && works_in(row, pane)).filter(|row| !row.act.is_ghost());
    let row = here.unwrap_or(first);
    Listing {
        act: row.act,
        keys: row.help.map_or_else(|| row.keys.iter().map(key_written).collect::<Vec<_>>().join(" "), |help| help.keys.to_owned()),
        does: row.help.map_or_else(|| row.act.name().replace('_', " "), |help| help.does.to_owned()),
        pane: row.pane,
        runs: here.is_some(),
    }
}

/// `key` as the help writes it, for a row with no line of its own.
fn key_written(key: &KeyEvent) -> String {
    let held = [(KeyModifiers::CONTROL, "^"), (KeyModifiers::ALT, "⌥"), (KeyModifiers::SUPER, "⌘"), (KeyModifiers::SHIFT, "⇧")];
    let marks: String = held.iter().filter(|(modifier, _)| key.modifiers.contains(*modifier)).map(|(_, mark)| *mark).collect();
    let code = match key.code {
        KeyCode::BackTab => "⇧⇥".to_owned(),
        KeyCode::Left => "←".to_owned(),
        KeyCode::Right => "→".to_owned(),
        KeyCode::Up => "↑".to_owned(),
        KeyCode::Down => "↓".to_owned(),
        KeyCode::PageUp => "PgUp".to_owned(),
        KeyCode::PageDown => "PgDn".to_owned(),
        code => code.to_string(),
    };
    format!("{marks}{code}")
}

fn help_rows(pane: Pane) -> Vec<HelpRow> {
    let hints = ROWS.iter().filter(|row| row.pane == pane).filter_map(|row| row.help);
    let editing = if pane == Pane::Editor { EDITING_KEYS } else { &[] };
    hints.chain(editing.iter().copied()).map(|hint| HelpRow { keys: hint.keys.to_owned(), does: hint.does }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::{ChartKind, Orientation};
    use strum::IntoEnumIterator;

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
    fn the_chart_keys_are_hinted_on_a_chart_and_not_on_the_grid() {
        let next = ChartNext { kind: Some(ChartKind::Stacked), orientation: Some(Orientation::Vertical) };
        let on_chart = chart_footer_within(next, 500, hint_width);
        let on_grid = footer_within(Pane::Result, 500, hint_width);
        let has = |hints: &[Hint], keys, does| hints.contains(&Hint { keys, does });
        assert!(has(&on_chart, "t", "stacked") && has(&on_chart, "r", "upright") && !has(&on_chart, "j/k", "rows"), "{on_chart:?}");
        assert!(!on_grid.iter().any(|hint| ["t", "r"].contains(&hint.keys)) && has(&on_grid, "j/k", "rows"), "{on_grid:?}");
    }

    #[test]
    fn v_on_a_chart_names_the_grid_it_goes_back_to() {
        let hints = chart_footer_within(ChartNext::default(), 500, hint_width);
        assert!(hints.contains(&Hint { keys: "v", does: "grid" }), "{hints:?}");
    }

    #[test]
    fn a_chart_key_that_leads_nowhere_has_no_hint() {
        let keys: Vec<&str> = chart_footer_within(ChartNext::default(), 500, hint_width).iter().map(|hint| hint.keys).collect();
        assert!(!keys.contains(&"t") && !keys.contains(&"r") && keys.contains(&"v"), "{keys:?}");
    }

    #[test]
    fn copying_the_whole_result_works_on_a_chart_too() {
        assert!(Act::CopyCell.is_grid() && !Act::CopyResult.is_grid());
    }

    #[test]
    fn the_editor_help_lists_the_keys_that_complete_a_name() {
        let keys: Vec<String> = help_rows(Pane::Editor).into_iter().map(|row| row.keys).collect();
        assert!(keys.contains(&"→ ⇥".to_owned()) && keys.contains(&"^space ^n".to_owned()), "{keys:?}");
    }

    #[test]
    fn every_action_has_a_key() {
        for act in Act::iter() {
            assert!(ROWS.iter().any(|row| row.act == act && !row.keys.is_empty()), "{act:?}");
        }
    }

    #[test]
    fn no_key_does_two_actions_in_one_pane() {
        for pane in [
            Pane::Editor,
            Pane::Result,
            Pane::Plan,
            Pane::Queries,
            Pane::Schema,
            Pane::SchemaFilter,
            Pane::RowFilter,
            Pane::Finder,
            Pane::Commands,
            Pane::TableFinder,
            Pane::Picker,
            Pane::Form,
            Pane::Prompt,
            Pane::Confirm,
            Pane::ConfirmDelete,
            Pane::Detail,
            Pane::ColumnStats,
            Pane::JsonTree,
            Pane::CopyAs,
            Pane::Completion,
            Pane::Instruction,
            Pane::Asking,
            Pane::Answer,
        ] {
            let rows = ROWS.iter().filter(|row| works_in(row, pane) && !row.act.is_ghost());
            let keys: Vec<&KeyEvent> = rows.flat_map(|row| row.keys).collect();
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
    fn command_k_finds_a_query_as_ctrl_p_does() {
        assert_eq!(act_for(command('k'), Pane::Editor), Some(Act::Find));
        assert_eq!(act_for(key(KeyCode::Char('k')), Pane::Editor), None);
    }

    #[test]
    fn option_s_saves_under_a_new_path_from_the_editor_and_the_result() {
        assert_eq!([Pane::Editor, Pane::Result].map(|pane| act_for(alt('s'), pane)), [Some(Act::SaveAs); 2]);
    }

    #[test]
    fn option_e_and_option_a_explain_in_the_editor_alone() {
        assert_eq!([alt('e'), alt('a')].map(|key| act_for(key, Pane::Editor)), [Some(Act::Explain), Some(Act::ExplainAnalyze)]);
        assert_eq!([alt('e'), alt('a')].map(|key| act_for(key, Pane::Result)), [None, None]);
    }

    #[test]
    fn every_option_key_has_a_key_beside_it_that_needs_no_option_setting() {
        let is_option = |pressed: &KeyEvent| pressed.modifiers.contains(KeyModifiers::ALT);
        for row in ROWS.iter().filter(|row| row.keys.iter().any(is_option)) {
            assert!(!row.keys.iter().all(is_option), "{:?}", row.act);
        }
    }

    #[test]
    fn the_keys_beside_option_work_from_the_editor() {
        let pressed = [control('b'), control('f'), control('d'), key(KeyCode::F(2))];
        let acts = [Act::Explain, Act::ExplainAnalyze, Act::SaveAs, Act::CopyStatement].map(Some);
        assert_eq!(pressed.map(|key| act_for(key, Pane::Editor)), acts);
    }

    #[test]
    fn the_shell_keys_of_a_line_are_left_to_the_editor() {
        for key in
            [control('a'), control('e'), control('k'), control('u'), control('w'), KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT)]
        {
            assert_eq!(act_for(key, Pane::Editor), None, "{key:?}");
        }
    }

    #[test]
    fn the_editor_help_lists_its_shell_keys() {
        let keys: Vec<String> = help_rows(Pane::Editor).into_iter().map(|row| row.keys).collect();
        assert!(keys.contains(&"^a ^e".to_owned()) && keys.contains(&"^k ^u".to_owned()), "{keys:?}");
    }

    #[test]
    fn ctrl_n_lists_the_names_where_macos_takes_ctrl_space() {
        assert_eq!(act_for(control('n'), Pane::Editor), Some(Act::Complete));
    }

    #[test]
    fn a_question_mark_types_in_the_editor_and_the_bar_and_opens_the_help_elsewhere() {
        let question_mark = key(KeyCode::Char('?'));
        assert_eq!([Pane::Editor, Pane::Params].map(|pane| act_for(question_mark, pane)), [None, None]);
        for pane in [Pane::Result, Pane::Queries, Pane::Schema] {
            assert_eq!(act_for(question_mark, pane), Some(Act::Help), "{pane:?}");
        }
    }

    #[test]
    fn ctrl_shift_z_redoes_whether_the_terminal_sends_the_capital_or_the_shift() {
        assert_eq!(act_for(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::SHIFT), Pane::Editor), Some(Act::Redo));
        assert_eq!(act_for(control('Z'), Pane::Editor), Some(Act::Redo));
        assert_eq!(act_for(control('z'), Pane::Editor), Some(Act::Undo));
    }

    #[test]
    fn a_colon_opens_the_palette_where_it_types_nothing_and_types_in_the_editor_and_the_bar() {
        let colon = key(KeyCode::Char(':'));
        assert_eq!([Pane::Editor, Pane::Params].map(|pane| act_for(colon, pane)), [None, None]);
        for pane in [Pane::Result, Pane::Queries, Pane::Schema] {
            assert_eq!(act_for(colon, pane), Some(Act::Commands), "{pane:?}");
        }
    }

    #[test]
    fn ctrl_l_opens_the_palette_from_every_pane_and_no_overlay_reads_it_as_that() {
        for pane in [Pane::Editor, Pane::Params, Pane::Result, Pane::Queries, Pane::Schema] {
            assert_eq!(act_for(control('l'), pane), Some(Act::Commands), "{pane:?}");
        }
        assert_eq!(act_for(control('l'), Pane::RowFilter), None);
    }

    #[test]
    fn the_palette_lists_every_act_once_with_its_keys_and_what_it_does() {
        let listings: Vec<Listing> = listings_from(Pane::Editor).collect();
        for act in Act::iter() {
            let found: Vec<&Listing> = listings.iter().filter(|listing| listing.act == act).collect();
            assert_eq!(found.len(), 1, "{act:?}");
            assert!(!found[0].keys.is_empty() && !found[0].does.is_empty(), "{act:?}");
        }
    }

    #[test]
    fn the_palette_lists_removing_a_connection_and_deleting_a_query() {
        let does = |act| listings_from(Pane::Editor).find(|listing| listing.act == act).map(|listing| listing.does);
        assert_eq!(does(Act::Delete).as_deref(), Some("remove the connection from the config, once you say yes"));
        assert_eq!(does(Act::DeleteQuery).as_deref(), Some("delete the query, once you say yes"));
    }

    #[test]
    fn a_listing_takes_the_help_of_the_pane_it_opened_on() {
        let down_from = |pane| listings_from(pane).find(|listing| listing.act == Act::Down).unwrap();
        assert_eq!((down_from(Pane::Result).keys.as_str(), down_from(Pane::Result).does.as_str()), ("j k", "next, previous row"));
        assert_eq!(down_from(Pane::Queries).does, "next, previous query");
    }

    #[test]
    fn a_row_with_no_help_is_listed_by_its_name_and_written_keys() {
        let listing = listings_from(Pane::Result).find(|listing| listing.act == Act::Up).unwrap();
        assert_eq!((listing.keys.as_str(), listing.does.as_str()), ("k ↑", "up"));
        let focus = listings_from(Pane::Editor).find(|listing| listing.act == Act::FocusUp).unwrap();
        assert_eq!((focus.keys.as_str(), focus.does.as_str()), ("⇧↑", "focus up"));
    }

    #[test]
    fn a_ghost_act_never_runs_from_the_palette() {
        assert!(!listings_from(Pane::Editor).any(|listing| listing.act.is_ghost() && listing.runs));
    }

    #[test]
    fn shift_tab_reads_the_same_whether_the_terminal_says_shift_or_not() {
        assert_eq!(act_for(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT), Pane::Form), Some(Act::Up));
    }

    #[test]
    fn the_first_help_lists_the_focused_pane_and_what_works_anywhere() {
        let titles: Vec<&str> = sections(Pane::Result, false).iter().map(|section| section.title).collect();
        assert_eq!(titles, ["result", "anywhere"]);
        assert_eq!(sections(Pane::Result, true).len(), 25);
    }
}
