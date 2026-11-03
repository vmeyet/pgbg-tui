//! `ctrl-l`, or `:` where it types nothing: the command palette, every act and every theme narrowed as fzf does.
//! An act runs as its key does, so a write still asks; a theme shows while highlighted, and `⏎` keeps it.
use super::search::{self, Search, Searched, Searching};
use super::{Action, App, Overlay};
use crate::act::{self, Act, Listing, Pane};
use crate::tui::theme::Theme;
use crossterm::event::KeyEvent;
use vtui::theme::Theme as Palette;

/// One row of the palette.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) enum Command {
    Act(Listing),
    Theme(Palette),
}

impl Command {
    /// Whether `⏎` does it from the pane the palette opened on.
    fn runs(&self) -> bool {
        match self {
            Self::Act(listing) => listing.runs,
            Self::Theme(_) => true,
        }
    }

    /// The act `⏎` runs, when it runs one.
    fn act_to_run(&self) -> Option<Act> {
        match self {
            Self::Act(Listing { act, runs: true, .. }) => Some(*act),
            Self::Act(_) | Self::Theme(_) => None,
        }
    }
}

impl Searched for Command {
    fn search_text(&self) -> String {
        match self {
            Self::Act(listing) => format!("{} {}", listing.does, listing.act.name()),
            Self::Theme(palette) => format!("theme {}", palette.name),
        }
    }
}

/// Every command, the search over them, and the theme to go back to.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Commands {
    all: Vec<Command>,
    pub(in crate::tui) search: Search<Command>,
    kept: Theme,
}

fn commands_from(pane: Pane) -> Vec<Command> {
    let themes = Palette::NAMES.into_iter().filter_map(Palette::named).map(Command::Theme);
    act::listings_from(pane).map(Command::Act).chain(themes).collect()
}

impl App {
    pub(super) fn open_commands(&mut self) {
        let all = commands_from(self.pane());
        let search = Search::new(&all);
        self.keep_commands(Commands { all, search, kept: self.theme });
    }

    /// The palette after `key`: it closes with the theme it opened on, or `⏎` runs the highlighted command.
    pub(super) fn commands_key(&mut self, commands: Commands, key: KeyEvent) -> Vec<Action> {
        let Commands { all, search, kept } = commands;
        match search::after(search, key, Pane::Commands, &all) {
            Searching::Typed(search) | Searching::Unhandled(search, _) => self.keep_commands(Commands { all, search, kept }),
            Searching::Closed => self.theme = kept,
            Searching::Picked(command) => return self.run_command(command),
            Searching::Quit => return self.quit(),
        }
        vec![]
    }

    pub(super) fn paste_in_commands(&mut self, text: &str) {
        let Some(Overlay::Commands(mut commands)) = self.overlay.take() else { return };
        commands.search.paste(text, &commands.all);
        self.keep_commands(*commands);
    }

    /// Keeps the palette open, what runs on top of what it lists, the highlighted theme on screen.
    fn keep_commands(&mut self, mut commands: Commands) {
        commands.search.shown.sort_by_key(|command| !command.runs());
        self.theme = match commands.search.selected() {
            Some(Command::Theme(palette)) => Theme::from(*palette).with_ground(self.ground),
            _ => commands.kept,
        };
        self.overlay = Some(Overlay::Commands(Box::new(commands)));
    }

    fn run_command(&mut self, command: Command) -> Vec<Action> {
        if let Some(act) = command.act_to_run() {
            return self.act(act);
        }
        match command {
            Command::Act(listing) => self.note = Some(format!("only in {}: {}", listing.pane.title(), listing.keys)),
            Command::Theme(theme) => return vec![Action::SaveTheme { id: self.new_config_change(), theme }],
        }
        vec![]
    }

    /// The act `⏎` would run in the open palette.
    pub(super) fn act_picked_in_commands(&self) -> Option<Act> {
        let Some(Overlay::Commands(commands)) = &self.overlay else { return None };
        commands.search.selected()?.act_to_run()
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        app, app_on, connection, control, enter, first_run, on_the_result, opened_on, press, shop, typed, view, with_every_pane,
        with_focus, with_result,
    };
    use super::super::{Answer, ConfigAnswer, ConfigChangeId, Focus, Prompt, View};
    use super::*;
    use crate::fixtures::{nord, orders};
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::style::Color;
    use std::time::Instant;

    fn shown(app: &App) -> &[Command] {
        let Some(Overlay::Commands(commands)) = &app.overlay else { panic!("no palette") };
        &commands.search.shown
    }

    #[test]
    fn a_colon_on_the_result_opens_the_palette() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Char(':'));
        assert!(matches!(app.overlay, Some(Overlay::Commands(_))));
    }

    #[test]
    fn a_colon_stays_text_in_the_editor() {
        let app = typed("SELECT 1:");
        assert_eq!((app.overlay.is_none(), app.sheet().editor.text()), (true, "SELECT 1:"));
    }

    #[test]
    fn a_colon_stays_text_in_the_parameter_bar() {
        let mut app = with_focus(typed("SELECT {{id}}"), Focus::Params);
        press(&mut app, KeyCode::Char(':'));
        assert!(app.overlay.is_none());
    }

    #[test]
    fn a_colon_stays_text_in_the_row_filter() {
        let mut app = on_the_result(orders());
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char(':'));
        let Some(Overlay::RowFilter(filter)) = &app.overlay else { panic!("no filter") };
        assert_eq!(filter.text(), ":");
    }

    #[test]
    fn a_colon_stays_text_in_a_prompt() {
        let mut app = typed("SELECT 1");
        app.handle_key(control('s'));
        press(&mut app, KeyCode::Char(':'));
        assert!(matches!(app.overlay, Some(Overlay::Prompt(_))));
    }

    #[test]
    fn the_palette_lists_every_theme_vtui_knows() {
        let themes: Vec<&str> = commands_from(Pane::Editor)
            .iter()
            .filter_map(|command| if let Command::Theme(palette) = command { Some(palette.name) } else { None })
            .collect();
        assert_eq!(themes, Palette::NAMES);
    }

    #[test]
    fn typing_narrows_the_list_and_what_runs_comes_first() {
        let app = opened_on(app(), "copy");
        assert!(shown(&app).len() < commands_from(Pane::Editor).len());
        let last_running = shown(&app).iter().rposition(Command::runs).unwrap();
        assert!(shown(&app)[..=last_running].iter().all(Command::runs));
    }

    #[test]
    fn enter_runs_an_act_of_the_pane_it_opened_on() {
        let mut app = opened_on(on_the_result(orders()), "as a grid or a chart");
        enter(&mut app);
        assert!(matches!(view(&app), View::Chart(_)));
        assert!(app.overlay.is_none());
    }

    #[test]
    fn save_as_runs_from_the_palette_on_the_editor() {
        let mut app = opened_on(typed("SELECT 1"), "save as");
        enter(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::Prompt(Prompt::SaveAs { .. }))), "{:?}", app.overlay);
    }

    #[test]
    fn plan_runs_the_explain_from_the_palette_on_the_editor() {
        let mut app = opened_on(typed("SELECT 2"), "plan");
        assert_eq!(enter(&mut app), [first_run(&shop(), "EXPLAIN (FORMAT JSON) SELECT 2")]);
    }

    #[test]
    fn plan_as_it_ran_runs_the_analyze_from_the_palette_on_the_editor() {
        let mut app = opened_on(typed("SELECT 2"), "plan as it ran");
        let actions = enter(&mut app);
        assert!(matches!(&actions[..], [Action::Analyze { .. }]), "{actions:?}");
    }

    #[test]
    fn an_act_of_another_pane_runs_nothing_and_says_where_it_works() {
        let mut app = opened_on(with_result(), "as a grid or a chart");
        assert_eq!(enter(&mut app), []);
        assert_eq!((view(&app), app.note()), (&View::Grid, Some("only in result: v")));
    }

    #[test]
    fn running_the_statement_from_the_palette_runs_it_as_its_key_does() {
        let mut app = opened_on(typed("SELECT 1"), "run the statement");
        assert_eq!(enter(&mut app), [first_run(&shop(), "SELECT 1")]);
    }

    #[test]
    fn a_write_run_from_the_palette_still_asks_first() {
        let mut app = app_on(&connection("local", "allowed"));
        "DELETE FROM orders".chars().for_each(|c| press(&mut app, KeyCode::Char(c)));
        let mut app = opened_on(app, "run the statement");
        assert!(!enter(&mut app).iter().any(|action| matches!(action, Action::Run { .. })));
        assert!(matches!(app.overlay, Some(Overlay::Prompt(_))));
    }

    #[test]
    fn closing_the_parameter_bar_from_the_palette_gives_the_editor_the_keys() {
        let mut app = opened_on(with_focus(typed("SELECT {{id}}"), Focus::Params), "back to the editor");
        enter(&mut app);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn the_palette_keeps_the_selection_for_the_act_it_runs() {
        let mut app = with_every_pane();
        for (kind, column) in [(MouseEventKind::Down(MouseButton::Left), 31), (MouseEventKind::Drag(MouseButton::Left), 37)] {
            app.handle_mouse(MouseEvent { kind, column, row: 2, modifiers: KeyModifiers::NONE }, Instant::now());
        }
        let mut app = opened_on(app, "copy the selection");
        let copied = enter(&mut app);
        assert!(matches!(copied.as_slice(), [Action::Copy { text, .. }] if text == "SELECT"), "{copied:?}");
        assert_eq!(app.sheet().editor.selected(), None);
    }

    #[test]
    fn a_paste_narrows_the_list_and_shows_the_theme_it_highlights() {
        let mut app = opened_on(app(), "");
        app.paste("theme nord\n");
        assert_eq!(shown(&app).first(), Some(&Command::Theme(nord())));
        assert_eq!(app.theme().name, "nord");
    }

    #[test]
    fn a_highlighted_theme_shows_and_moving_off_it_brings_the_old_one_back() {
        let mut app = opened_on(app(), "theme");
        let Some(Command::Theme(first)) = shown(&app).first().cloned() else { panic!("no theme first") };
        assert_eq!(app.theme().name, first.name);
        let down = (0..Palette::NAMES.len()).map(|_| KeyCode::Down);
        down.for_each(|code| press(&mut app, code));
        assert_eq!(*app.theme(), Theme::default());
    }

    #[test]
    fn esc_brings_the_old_theme_back_and_writes_nothing() {
        let mut app = opened_on(app(), "theme nord");
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Esc)), []);
        assert_eq!(*app.theme(), Theme::default());
    }

    #[test]
    fn enter_keeps_the_theme_and_asks_to_write_it() {
        let mut app = opened_on(app(), "theme nord");
        assert_eq!(enter(&mut app), [Action::SaveTheme { id: ConfigChangeId::FIRST, theme: nord() }]);
        assert_eq!(app.theme().name, "nord");
        assert!(app.overlay.is_none());
    }

    #[test]
    fn a_previewed_theme_takes_the_terminals_ground_as_the_first_one_did() {
        let app = opened_on(app().with_ground(Some(0x10_20_30)), "theme default");
        assert_eq!(app.theme().base, Color::Rgb(0x10, 0x20, 0x30));
    }

    #[test]
    fn a_failed_write_warns_and_keeps_the_theme() {
        let mut app = opened_on(app(), "theme nord");
        enter(&mut app);
        let why = "theme nord is for this session, not saved: permission denied".to_owned();
        app.apply(Answer::Config { id: ConfigChangeId::FIRST, answer: ConfigAnswer::Failed { why: why.clone(), password: None } });
        assert_eq!((app.config_failure(), app.theme().name), (Some(why.as_str()), "nord"));
    }

    #[test]
    fn a_written_theme_says_so() {
        let mut app = app();
        app.apply(Answer::Config { id: ConfigChangeId::FIRST, answer: ConfigAnswer::ThemeSaved("nord") });
        assert_eq!(app.note(), Some("theme nord saved"));
    }

    #[test]
    fn the_act_enter_runs_is_the_one_counted() {
        let enter_key = KeyEvent::from(KeyCode::Enter);
        assert_eq!(opened_on(on_the_result(orders()), "as a grid or a chart").act_of(enter_key), Some(Act::Chart));
        assert_eq!(opened_on(with_result(), "as a grid or a chart").act_of(enter_key), Some(Act::Open), "it runs nothing");
        assert_eq!(opened_on(app(), "theme nord").act_of(enter_key), Some(Act::Open));
        assert_eq!(app().act_of(control('l')), Some(Act::Commands));
    }

    #[test]
    fn ctrl_c_in_the_palette_quits() {
        let mut app = opened_on(app(), "");
        app.handle_key(control('c'));
        assert!(app.should_quit());
    }
}
