//! `ctrl-o`, the connection picker: typing narrows the connections as fzf does, and picking one shows its sheet,
//! opening its session the first time.
use super::{Action, App, Sheet};
use crate::act::{self, Act, Pane};
use crate::connection::{Connection, ConnectionName};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vtui::fuzzy;
use vtui::palette::{Mode, Palette};

/// The picker searches connections only, so no character switches its mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct Picking;

impl Mode for Picking {
    fn switched_by(self, _first: char) -> Option<Self> {
        None
    }

    fn takes_commands(self) -> bool {
        false
    }
}

/// What a key does in the picker.
enum Picked {
    Stay(Palette<Picking>),
    Close,
    Show(Connection),
    Quit,
}

impl App {
    pub(super) fn open_picker(&mut self) {
        self.picker = Some(Palette::new(Picking, vec![]));
    }

    /// The connections whose name matches `typed`, best first; every one, by name, when nothing is typed.
    pub(in crate::tui) fn matching(&self, typed: &str) -> Vec<&Connection> {
        let names = self.connections.iter().map(|connection| (connection.name().to_string(), connection));
        fuzzy::rank(typed, names).into_iter().map(|(_, connection)| connection).collect()
    }

    /// The sheet of the connection `name`, once it was opened.
    pub(in crate::tui) fn sheet_of(&self, name: &ConnectionName) -> Option<&Sheet> {
        self.sheets.iter().find(|sheet| &sheet.name == name)
    }

    pub(super) fn picker_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Some(picker) = self.picker.take() else { return vec![] };
        match self.picked(picker, key) {
            Picked::Stay(picker) => self.picker = Some(picker),
            Picked::Close => {}
            Picked::Show(connection) => return self.show(connection),
            Picked::Quit => return self.quit(),
        }
        vec![]
    }

    /// What `key` does: the picker's own keys move, pick and close, the rest types.
    fn picked(&self, mut picker: Palette<Picking>, key: KeyEvent) -> Picked {
        let matching = self.matching(&picker.input);
        match act::act_for(key, Pane::Picker) {
            Some(Act::Down) => picker.move_by(1, matching.len()),
            Some(Act::Up) => picker.move_by(-1, matching.len()),
            Some(Act::Open) => return matching.get(picker.selected).map_or(Picked::Stay(picker), |&c| Picked::Show(c.clone())),
            Some(Act::Close) => return Picked::Close,
            Some(Act::Quit) => return Picked::Quit,
            _ => match key.code {
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => picker.type_char(c),
                KeyCode::Backspace if !picker.backspace() => return Picked::Close,
                _ => {}
            },
        }
        Picked::Stay(picker)
    }

    /// Shows the sheet of `connection`, starting one and opening its session when it has none.
    fn show(&mut self, connection: Connection) -> Vec<Action> {
        if let Some(open) = self.sheets.iter().position(|sheet| &sheet.name == connection.name()) {
            self.shown = open;
            return vec![];
        }
        self.sheets.push(Sheet::new(&connection));
        self.shown = self.sheets.len() - 1;
        vec![Action::Open(connection)]
    }
}

#[cfg(test)]
pub(in crate::tui) mod tests {
    use super::super::tests::orders;
    use super::super::{Answer, Incoming, Link, QueryAnswer, Run, RunId};
    use super::*;
    use crate::connection::Entry;
    use crate::db::model::Statement;
    use std::time::Duration;
    use vtui::theme::Theme;

    pub(in crate::tui) fn named(name: &str, keys: &str) -> Connection {
        Connection::new(name, toml::from_str::<Entry>(keys).unwrap()).unwrap()
    }

    /// Three connections, the TUI on `local`.
    pub(in crate::tui) fn on_local() -> App {
        let local = named("local", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n");
        let prod = named(
            "prod",
            "tier = \"production\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\ntunnel = { command = [\"ssh\", \"-L\", \"{port}:localhost:5432\", \"db.example.com\"] }\n",
        );
        let staging =
            named("staging", "tier = \"staging\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\nwrites = \"allowed\"\n");
        App::new(vec![local.clone(), prod, staging], &local, Theme::default())
    }

    fn name(text: &str) -> ConnectionName {
        text.parse().unwrap()
    }

    pub(in crate::tui) fn press(app: &mut App, keys: &str) -> Vec<Action> {
        keys.chars().flat_map(|c| app.handle_key(KeyEvent::from(KeyCode::Char(c)))).collect()
    }

    pub(in crate::tui) fn ctrl(app: &mut App, c: char) -> Vec<Action> {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn key(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code))
    }

    fn names(app: &App) -> Vec<String> {
        app.matching(&app.picker.as_ref().unwrap().input).iter().map(|c| c.name().to_string()).collect()
    }

    fn switch_to(app: &mut App, typed: &str) -> Vec<Action> {
        ctrl(app, 'o');
        press(app, typed);
        key(app, KeyCode::Enter)
    }

    #[test]
    fn ctrl_o_lists_every_connection_by_name() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        assert_eq!(names(&app), ["local", "prod", "staging"]);
    }

    #[test]
    fn typing_narrows_the_list_and_types_nothing_in_the_editor() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        press(&mut app, "sg");
        assert_eq!(names(&app), ["staging"]);
        assert_eq!(app.sheet().editor.text(), "");
    }

    #[test]
    fn picking_a_connection_not_yet_open_shows_it_and_opens_its_session() {
        let mut app = on_local();
        let actions = switch_to(&mut app, "prod");
        assert!(matches!(&*actions, [Action::Open(prod)] if prod.name() == &name("prod")), "{actions:?}");
        assert_eq!((&app.sheet().name, &app.sheet().link, app.picker.is_none()), (&name("prod"), &Link::Opening, true));
    }

    #[test]
    fn switching_back_is_instant_and_finds_the_editor_and_result_as_they_were() {
        let mut app = on_local();
        press(&mut app, "SELECT 1");
        key(&mut app, KeyCode::Enter);
        app.apply(Answer::Session {
            connection: name("local"),
            incoming: Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::ZERO },
        });
        switch_to(&mut app, "prod");
        assert_eq!(app.sheet().editor.text(), "");
        assert_eq!(switch_to(&mut app, "local"), []);
        assert_eq!(app.sheet().editor.text(), "SELECT 1");
        assert!(matches!(app.sheet().run, Run::Done { .. }));
    }

    #[test]
    fn an_answer_from_a_connection_not_on_screen_lands_on_its_own_sheet() {
        let mut app = on_local();
        press(&mut app, "SELECT 1");
        key(&mut app, KeyCode::Enter);
        switch_to(&mut app, "prod");
        app.apply(Answer::Session {
            connection: name("local"),
            incoming: Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::ZERO },
        });
        assert_eq!(app.sheet().run, Run::Idle);
        assert!(matches!(app.sheet_of(&name("local")).unwrap().run, Run::Done { .. }));
    }

    #[test]
    fn a_saved_query_stays_with_the_connection_it_was_opened_on() {
        let mut app = on_local();
        let customers =
            crate::query::Query { source: crate::query::Source::Project, path: crate::query::QueryPath::parse("customers.sql").unwrap() };
        app.apply(Answer::Queries(QueryAnswer::Opened { query: customers.clone(), text: "SELECT 1".into() }));
        switch_to(&mut app, "prod");
        assert_eq!(app.sheet().opened, None);
        switch_to(&mut app, "local");
        assert_eq!(app.sheet().opened, Some(customers));
    }

    #[test]
    fn a_statement_runs_on_the_connection_on_screen() {
        let mut app = on_local();
        switch_to(&mut app, "prod");
        press(&mut app, "SELECT 1");
        let run = key(&mut app, KeyCode::Enter);
        assert_eq!(run, [Action::Run { connection: name("prod"), run: RunId::FIRST, statement: Statement::new("SELECT 1") }]);
    }

    #[test]
    fn quitting_cancels_the_running_statement_of_every_connection() {
        let mut app = on_local();
        press(&mut app, "SELECT 1");
        key(&mut app, KeyCode::Enter);
        switch_to(&mut app, "prod");
        press(&mut app, "SELECT 2");
        key(&mut app, KeyCode::Enter);
        let cancels = ctrl(&mut app, 'c');
        let expected = [name("local"), name("prod")].map(|connection| Action::Cancel { connection, run: RunId::FIRST });
        assert_eq!(cancels, expected);
    }

    #[test]
    fn esc_or_ctrl_o_closes_the_picker_and_gives_the_keys_back_to_the_editor() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        key(&mut app, KeyCode::Esc);
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'o');
        press(&mut app, "x");
        assert_eq!((app.picker.is_none(), app.sheet().editor.text()), (true, "x"));
    }

    #[test]
    fn ctrl_c_in_the_picker_quits() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'c');
        assert!(app.should_quit);
    }

    #[test]
    fn enter_without_a_match_keeps_the_picker_open() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        press(&mut app, "zz");
        assert_eq!(key(&mut app, KeyCode::Enter), []);
        assert!(app.picker.is_some());
    }

    #[test]
    fn arrows_move_the_highlight_inside_the_list() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        key(&mut app, KeyCode::Down);
        ctrl(&mut app, 'n');
        ctrl(&mut app, 'n');
        key(&mut app, KeyCode::Up);
        assert_eq!(app.picker.as_ref().unwrap().selected, 1);
    }
}
