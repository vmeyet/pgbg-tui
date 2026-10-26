//! `ctrl-o`, the connection picker: typing narrows the connections as fzf does, and picking one shows its sheet,
//! opening its session the first time.
use super::search::{Search, Searching};
use super::{Action, App, Form, Overlay, Run, SessionState, Sheet};
use crate::act::{Act, Pane};
use crate::connection::{Connection, ConnectionName};
use crossterm::event::{KeyCode, KeyEvent};

/// The picker: the connections matching the line typed, and the line it says under them.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Picker {
    pub(in crate::tui) search: Search<Connection>,
    pub(in crate::tui) note: Option<Note>,
}

/// The line under the picker's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Note {
    /// `y` removes this connection, any other key keeps it.
    ConfirmRemove(ConnectionName),
    /// Something to read, gone at the next key.
    Said(String),
    /// Why the config did not change, gone at the next key.
    Failed(String),
}

/// What a key does in the picker.
enum Picked {
    Stay(Picker),
    Close,
    Show(Connection),
    Quit,
    Remove(ConnectionName),
    Form(Box<Form>),
    CloseSession(ConnectionName),
}

impl App {
    pub(super) fn open_picker(&mut self) {
        self.overlay = Some(Overlay::Picker(Picker { search: Search::new(&self.connections), note: None }));
    }

    /// The sheet of the connection `name`, once it was opened.
    pub(in crate::tui) fn sheet_of(&self, name: &ConnectionName) -> Option<&Sheet> {
        self.sheets.iter().find(|sheet| sheet.name() == name)
    }

    pub(super) fn sheet_of_mut(&mut self, name: &ConnectionName) -> Option<&mut Sheet> {
        self.sheets.iter_mut().find(|sheet| sheet.name() == name)
    }

    pub(super) fn picker_key(&mut self, picker: Picker, key: KeyEvent) -> Vec<Action> {
        match self.picked(picker, key) {
            Picked::Stay(picker) => self.overlay = Some(Overlay::Picker(picker)),
            Picked::Close => {}
            Picked::Show(connection) => return self.show(connection),
            Picked::Quit => return self.quit(),
            Picked::Form(form) => self.overlay = Some(Overlay::Form(form)),
            Picked::Remove(name) => {
                self.say(format!("removing `{name}`…"));
                return vec![Action::RemoveConnection(name)];
            }
            Picked::CloseSession(name) => {
                self.drop_sheet(&name);
                self.say(format!("closed the session of `{name}`"));
                return vec![Action::Close(name)];
            }
        }
        vec![]
    }

    /// What `key` does: a pending question takes it; else the search moves, picks and closes, the rest acts on the highlighted row.
    fn picked(&self, picker: Picker, key: KeyEvent) -> Picked {
        let Picker { search, note } = picker;
        if let Some(Note::ConfirmRemove(name)) = note {
            let is_yes = key.code == KeyCode::Char('y');
            return if is_yes { Picked::Remove(name) } else { Picked::Stay(Picker { search, note: None }) };
        }
        match search.after(key, Pane::Picker, &self.connections) {
            Searching::Typed(search) => Picked::Stay(Picker { search, note: None }),
            Searching::Closed => Picked::Close,
            Searching::Picked(connection) => Picked::Show(connection),
            Searching::Quit => Picked::Quit,
            Searching::Unhandled(search, act) => self.act_on_selected(search, act),
        }
    }

    /// What `act` does to the highlighted connection: edit it, remove it or close its session; `ctrl-a` adds one.
    fn act_on_selected(&self, search: Search<Connection>, act: Act) -> Picked {
        let selected = search.selected().cloned();
        let name = selected.as_ref().map(|connection| connection.name().clone());
        let note = match act {
            Act::Add => return Picked::Form(Box::new(Form::adding())),
            Act::Edit => match &selected {
                Some(connection) => return Picked::Form(Box::new(Form::editing(connection))),
                None => None,
            },
            Act::Delete => name.map(|name| self.removal(name)),
            Act::CloseSession => match name.filter(|name| self.sheet_of(name).is_some()) {
                Some(name) if name != *self.sheet().name() => return Picked::CloseSession(name),
                Some(name) => Some(on_screen(&name)),
                None => None,
            },
            _ => None,
        };
        Picked::Stay(Picker { search, note })
    }

    /// The question before removing `name`; the connection on screen cannot go.
    fn removal(&self, name: ConnectionName) -> Note {
        if name == *self.sheet().name() { on_screen(&name) } else { Note::ConfirmRemove(name) }
    }

    fn say(&mut self, said: String) {
        if let Some(Overlay::Picker(picker)) = &mut self.overlay {
            picker.note = Some(Note::Said(said));
        }
    }

    /// Shows the sheet of `connection`, starting one and opening its session when it has none, or when it failed.
    fn show(&mut self, connection: Connection) -> Vec<Action> {
        if let Some(open) = self.sheets.iter().position(|sheet| sheet.name() == connection.name()) {
            self.shown = open;
            if !matches!(self.sheet().session, SessionState::Failed(_)) {
                return vec![];
            }
            self.sheet_mut().session = SessionState::Opening;
            return vec![Action::Open(connection)];
        }
        self.sheets.push(Sheet::new(&connection));
        self.shown = self.sheets.len() - 1;
        vec![Action::Open(connection)]
    }

    /// Forgets the sheet of `name`, which is not the one on screen.
    fn drop_sheet(&mut self, name: &ConnectionName) {
        let shown = self.sheet().name().clone();
        self.sheets.retain(|sheet| sheet.name() != name);
        self.shown = self.sheets.iter().position(|sheet| *sheet.name() == shown).unwrap_or_default();
    }

    /// Takes the connections the config holds now: the sheet of one gone closes, the sheet of one changed opens again.
    pub(super) fn take_connections(&mut self, connections: Vec<Connection>) -> Vec<Action> {
        let closed = self.close_gone(&connections);
        let reopened = self.reopen_changed(&connections);
        self.connections = connections;
        if let Some(Overlay::Picker(picker)) = &mut self.overlay {
            picker.search.rank(&self.connections);
        }
        self.clear_said();
        if matches!(self.overlay, Some(Overlay::Form(_))) {
            self.overlay = None;
        }
        closed.into_iter().chain(reopened).collect()
    }

    /// Drops the sheets of connections no longer in `connections`, but the one on screen.
    fn close_gone(&mut self, connections: &[Connection]) -> Vec<Action> {
        let shown = self.sheet().name().clone();
        let is_gone = |name: &ConnectionName| *name != shown && !connections.iter().any(|c| c.name() == name);
        let gone: Vec<ConnectionName> = self.sheets.iter().map(Sheet::name).filter(|&name| is_gone(name)).cloned().collect();
        for name in &gone {
            self.drop_sheet(name);
        }
        gone.into_iter().map(Action::Close).collect()
    }

    fn reopen_changed(&mut self, connections: &[Connection]) -> Vec<Action> {
        let changed =
            |sheet: &Sheet| connections.iter().find(|c| c.name() == sheet.name()).filter(|&now| *now != sheet.connection).cloned();
        let reopen = |sheet: &mut Sheet| {
            let now = changed(sheet)?;
            sheet.reopen(now.clone());
            Some(Action::Reopen(now))
        };
        self.sheets.iter_mut().filter_map(reopen).collect()
    }

    /// Says why the config did not change: in the picker when it is open, else in the result.
    pub(super) fn config_failed(&mut self, error: String) {
        match &mut self.overlay {
            Some(Overlay::Picker(picker)) => picker.note = Some(Note::Failed(error)),
            Some(Overlay::Form(form)) => form.failed(error),
            _ => self.sheet_mut().run = Run::Failed(error),
        }
    }

    fn clear_said(&mut self) {
        if let Some(Overlay::Picker(picker)) = &mut self.overlay
            && matches!(picker.note, Some(Note::Said(_)))
        {
            picker.note = None;
        }
    }
}

fn on_screen(name: &ConnectionName) -> Note {
    Note::Said(format!("`{name}` is on screen: switch to another connection first"))
}

#[cfg(test)]
pub(in crate::tui) mod tests {
    use super::super::tests::orders;
    use super::super::{Answer, ConfigAnswer, Incoming, QueryAnswer, Run, RunId, SessionState};
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::model::Statement;
    use crate::fixtures::named;
    use crossterm::event::KeyModifiers;
    use std::time::Duration;
    use vtui::theme::Theme;

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

    fn picker(app: &App) -> &Picker {
        let Some(Overlay::Picker(picker)) = &app.overlay else { panic!("no picker") };
        picker
    }

    fn names(app: &App) -> Vec<String> {
        picker(app).search.shown.iter().map(|c| c.name().to_string()).collect()
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
        assert_eq!((app.sheet().name(), &app.sheet().session, app.overlay.is_none()), (&name("prod"), &SessionState::Opening, true));
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
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: name("local"), query: customers.clone(), text: "SELECT 1".into() }));
        switch_to(&mut app, "prod");
        assert_eq!(app.sheet().opened, None);
        switch_to(&mut app, "local");
        assert_eq!(app.sheet().opened, Some(customers));
    }

    #[test]
    fn a_saved_query_read_after_a_switch_lands_on_the_sheet_that_asked_for_it() {
        let mut app = on_local();
        let customers =
            crate::query::Query { source: crate::query::Source::Project, path: crate::query::QueryPath::parse("customers.sql").unwrap() };
        switch_to(&mut app, "prod");
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: name("local"), query: customers.clone(), text: "SELECT 1".into() }));
        app.apply(Answer::Queries(QueryAnswer::NotOpened {
            connection: name("local"),
            query: customers,
            error: "permission denied".into(),
        }));
        assert_eq!((app.sheet().editor.text(), &app.sheet().query_error), ("", &None));
        let local = app.sheet_of(&name("local")).unwrap();
        assert_eq!(local.editor.text(), "SELECT 1");
        assert!(local.query_error.is_some());
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
        assert_eq!((app.overlay.is_none(), app.sheet().editor.text()), (true, "x"));
    }

    #[test]
    fn picking_a_connection_whose_session_failed_opens_it_again() {
        let mut app = on_local();
        switch_to(&mut app, "prod");
        app.apply(Answer::Session { connection: name("prod"), incoming: Incoming::NotOpened("no route".into()) });
        switch_to(&mut app, "local");
        let actions = switch_to(&mut app, "prod");
        assert!(matches!(&*actions, [Action::Open(prod)] if prod.name() == &name("prod")), "{actions:?}");
        assert_eq!(app.sheet().session, SessionState::Opening);
    }

    #[test]
    fn a_connection_added_while_the_picker_is_open_is_listed() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        let mut connections = app.connections.clone();
        connections.push(named("replica", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n"));
        app.take_connections(connections);
        assert_eq!(names(&app), ["local", "prod", "staging", "replica"]);
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
        assert!(matches!(app.overlay, Some(Overlay::Picker(_))));
    }

    #[test]
    fn arrows_move_the_highlight_inside_the_list() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        key(&mut app, KeyCode::Down);
        ctrl(&mut app, 'n');
        ctrl(&mut app, 'n');
        key(&mut app, KeyCode::Up);
        assert_eq!(picker(&app).search.palette.selected, 1);
    }

    fn on_prod_with_local_open() -> App {
        let mut app = on_local();
        switch_to(&mut app, "prod");
        ctrl(&mut app, 'o');
        app
    }

    #[test]
    fn ctrl_d_asks_before_removing_and_y_removes() {
        let mut app = on_prod_with_local_open();
        ctrl(&mut app, 'd');
        assert_eq!(picker(&app).note, Some(Note::ConfirmRemove(name("local"))));
        assert_eq!(press(&mut app, "y"), [Action::RemoveConnection(name("local"))]);
    }

    #[test]
    fn any_other_key_than_y_keeps_the_connection() {
        let mut app = on_prod_with_local_open();
        ctrl(&mut app, 'd');
        assert_eq!(press(&mut app, "n"), []);
        assert_eq!((picker(&app).note.as_ref(), picker(&app).search.palette.input.as_str()), (None, ""));
    }

    #[test]
    fn the_connection_on_screen_is_neither_removed_nor_closed() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'd');
        assert!(matches!(&picker(&app).note, Some(Note::Said(said)) if said.contains("on screen")));
        assert_eq!(ctrl(&mut app, 'x'), []);
    }

    #[test]
    fn ctrl_x_closes_the_session_of_a_connection_not_on_screen() {
        let mut app = on_prod_with_local_open();
        assert_eq!(ctrl(&mut app, 'x'), [Action::Close(name("local"))]);
        assert!(app.sheet_of(&name("local")).is_none());
        assert_eq!(app.sheet().name(), &name("prod"));
    }

    #[test]
    fn a_connection_gone_from_the_config_closes_and_one_changed_opens_again_as_it_is_now() {
        let mut app = on_prod_with_local_open();
        let prod =
            named("prod", "tier = \"production\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\nwrites = \"allowed\"\n");
        let actions = app.apply(Answer::Config(ConfigAnswer::Changed(vec![prod.clone()])));
        assert_eq!(actions, [Action::Close(name("local")), Action::Reopen(prod)]);
        assert_eq!(app.sheet().connection.writes(), WritePolicy::Allowed);
        assert_eq!(app.connections.len(), 1);
    }

    #[test]
    fn a_change_that_failed_says_why_in_the_picker() {
        let mut app = on_prod_with_local_open();
        app.apply(Answer::Config(ConfigAnswer::Failed("permission denied".into())));
        assert_eq!(picker(&app).note, Some(Note::Failed("permission denied".into())));
    }
}
