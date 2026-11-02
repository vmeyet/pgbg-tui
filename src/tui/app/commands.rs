//! `ctrl-l`, or `:` where it types nothing: the command palette, every act narrowed as fzf does.
//! An act runs as its key does, so a write still asks.
use super::search::{Search, Searched, Searching};
use super::{Action, App, Overlay};
use crate::act::{self, Act, Listing, Pane};
use crossterm::event::KeyEvent;

impl Searched for Listing {
    fn search_text(&self) -> String {
        format!("{} {}", self.does, self.act.name())
    }
}

/// Every act and the search over them.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Commands {
    all: Vec<Listing>,
    pub(in crate::tui) search: Search<Listing>,
}

impl App {
    pub(super) fn open_commands(&mut self) {
        let all: Vec<Listing> = act::listings_from(self.pane()).collect();
        let search = Search::new(&all);
        self.keep_commands(Commands { all, search });
    }

    /// The palette after `key`: `⏎` runs the highlighted act.
    pub(super) fn commands_key(&mut self, commands: Commands, key: KeyEvent) -> Vec<Action> {
        let Commands { all, search } = commands;
        match search.after(key, Pane::Commands, &all) {
            Searching::Typed(search) | Searching::Unhandled(search, _) => self.keep_commands(Commands { all, search }),
            Searching::Closed => {}
            Searching::Picked(listing) => return self.run_listing(&listing),
            Searching::Quit => return self.quit(),
        }
        vec![]
    }

    pub(super) fn paste_in_commands(&mut self, text: &str) {
        let Some(Overlay::Commands(mut commands)) = self.overlay.take() else { return };
        commands.search.paste(text, &commands.all);
        self.keep_commands(*commands);
    }

    /// Keeps the palette open, what runs on top of what it lists.
    fn keep_commands(&mut self, mut commands: Commands) {
        commands.search.shown.sort_by_key(|listing| !listing.runs);
        self.overlay = Some(Overlay::Commands(Box::new(commands)));
    }

    fn run_listing(&mut self, listing: &Listing) -> Vec<Action> {
        if listing.runs {
            return self.act(listing.act);
        }
        self.note = Some(format!("only in {}: {}", listing.pane.title(), listing.keys));
        vec![]
    }

    /// The act `⏎` would run in the open palette.
    pub(super) fn act_picked_in_commands(&self) -> Option<Act> {
        let Some(Overlay::Commands(commands)) = &self.overlay else { return None };
        commands.search.selected().filter(|listing| listing.runs).map(|listing| listing.act)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        app, app_on, connection, control, enter, first_run, on_the_result, opened_on, press, shop, typed, view, with_every_pane,
        with_focus, with_result,
    };
    use super::super::{Focus, View};
    use super::*;
    use crate::fixtures::orders;
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use std::time::Instant;

    fn shown(app: &App) -> &[Listing] {
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
    fn typing_narrows_the_list_and_what_runs_comes_first() {
        let app = opened_on(app(), "copy");
        assert!(shown(&app).len() < act::listings_from(Pane::Editor).count());
        let last_running = shown(&app).iter().rposition(|listing| listing.runs).unwrap();
        assert!(shown(&app)[..=last_running].iter().all(|listing| listing.runs));
    }

    #[test]
    fn enter_runs_an_act_of_the_pane_it_opened_on() {
        let mut app = opened_on(on_the_result(orders()), "as a grid or a chart");
        enter(&mut app);
        assert!(matches!(view(&app), View::Chart(_)));
        assert!(app.overlay.is_none());
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
    fn a_paste_narrows_the_list() {
        let mut app = opened_on(app(), "");
        app.paste("copy the selection\n");
        assert_eq!(shown(&app).first().map(|listing| listing.act), Some(Act::CopySelection));
    }

    #[test]
    fn the_act_enter_runs_is_the_one_counted() {
        let enter_key = KeyEvent::from(KeyCode::Enter);
        assert_eq!(opened_on(on_the_result(orders()), "as a grid or a chart").act_of(enter_key), Some(Act::Chart));
        assert_eq!(opened_on(with_result(), "as a grid or a chart").act_of(enter_key), Some(Act::Open), "it runs nothing");
        assert_eq!(app().act_of(control('l')), Some(Act::Commands));
    }

    #[test]
    fn ctrl_c_in_the_palette_quits() {
        let mut app = opened_on(app(), "");
        app.handle_key(control('c'));
        assert!(app.should_quit());
    }
}
