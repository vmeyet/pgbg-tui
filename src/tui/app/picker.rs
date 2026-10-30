//! `ctrl-o`, the connection picker: typing narrows the connections as fzf does.
//! Picking one shows its sheet, and opens its session the first time.
use super::search::{Search, Searching};
use super::{Action, App, ConfigChangeId, Form, Overlay, SessionState, Sheet};
use crate::act::{Act, Pane};
use crate::connection::{Connection, ConnectionName};
use crate::secret::Secret;
use crossterm::event::{KeyCode, KeyEvent};

/// The picker: the connections that match the typed line, and the note shown under them.
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
    /// A message, cleared at the next key.
    Said(String),
}

/// What a key does in the picker.
enum Picked {
    Stay(Picker),
    Close,
    Show(Connection),
    Quit,
    Remove(ConnectionName),
    Form(Box<Form>),
    CloseSession { search: Search<Connection>, name: ConnectionName },
}

impl App {
    pub(super) fn open_picker(&mut self) {
        self.overlay = Some(Overlay::Picker(Picker { search: Search::new(&self.connections), note: None }));
    }

    /// The sheet of the connection `name`, once it has been opened.
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
            Picked::Show(connection) => {
                let listed = self.queries_folder().clone();
                let shown = self.show(connection);
                return shown.into_iter().chain(self.follow_queries_folder(&listed)).collect();
            }
            Picked::Quit => return self.quit(),
            Picked::Form(form) => self.overlay = Some(Overlay::Form(form)),
            Picked::Remove(name) => {
                let id = self.new_config_change();
                return vec![Action::RemoveConnection { id, connection: name }];
            }
            Picked::CloseSession { search, name } => {
                if let Some(sheet) = self.sheet_of_mut(&name) {
                    sheet.close();
                }
                let note = Some(Note::Said(format!("closed the session of `{name}`")));
                self.overlay = Some(Overlay::Picker(Picker { search, note }));
                return vec![Action::Close(name)];
            }
        }
        vec![]
    }

    /// What `key` does: a pending question takes it first; otherwise the search moves, picks and closes, and other
    /// keys act on the highlighted row.
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
            Act::CloseSession => match name.filter(|name| self.has_session(name)) {
                Some(name) if name != *self.sheet().name() => return Picked::CloseSession { search, name },
                Some(name) => Some(on_screen(&name)),
                None => None,
            },
            _ => None,
        };
        Picked::Stay(Picker { search, note })
    }

    /// Whether `name` has a sheet whose session was not closed.
    fn has_session(&self, name: &ConnectionName) -> bool {
        self.sheet_of(name).is_some_and(|sheet| sheet.session != SessionState::Closed)
    }

    /// The question asked before removing `name`; the connection on screen cannot be removed.
    fn removal(&self, name: ConnectionName) -> Note {
        if name == *self.sheet().name() { on_screen(&name) } else { Note::ConfirmRemove(name) }
    }

    /// Shows the sheet of `connection`, creating it if needed.
    /// Its session opens when it has none, or when the last one failed or was closed.
    fn show(&mut self, connection: Connection) -> Vec<Action> {
        let Some(open) = self.sheets.iter().position(|sheet| sheet.name() == connection.name()) else {
            let session = self.new_session();
            self.sheets.push(Sheet::new(&connection, session));
            self.shown = self.sheets.len() - 1;
            return vec![Action::Open { connection, session }];
        };
        self.shown = open;
        match self.sheet().session {
            SessionState::Failed(_) => self.sheet_mut().session = SessionState::Opening,
            SessionState::Closed => {
                let session = self.new_session();
                self.sheet_mut().reopen(connection.clone(), session);
            }
            SessionState::Opening | SessionState::Open => return vec![],
        }
        vec![Action::Open { connection, session: self.sheet().session_id }]
    }

    /// Forgets the sheet of `name`, which is not the one on screen.
    fn drop_sheet(&mut self, name: &ConnectionName) {
        let shown = self.sheet().name().clone();
        self.sheets.retain(|sheet| sheet.name() != name);
        self.shown = self.sheets.iter().position(|sheet| *sheet.name() == shown).unwrap_or_default();
    }

    /// Takes the connections the config holds after the change `id`.
    /// The sheet of a removed connection closes, the sheet of a changed one opens its session again unless it was
    /// closed, the form that sent `id` closes, the status line shows `warning`, and the queries are listed again when the
    /// connection on screen now uses another folder.
    pub(super) fn take_connections(&mut self, id: ConfigChangeId, connections: Vec<Connection>, warning: Option<String>) -> Vec<Action> {
        let listed = self.queries_folder().clone();
        let closed = self.close_gone(&connections);
        let reopened = self.reopen_changed(&connections);
        let relisted = self.follow_queries_folder(&listed);
        self.connections = connections;
        if let Some(Overlay::Picker(picker)) = &mut self.overlay {
            picker.search.rank(&self.connections);
        }
        self.clear_said();
        self.config_failure = warning;
        if self.is_form_waiting_on(id) {
            self.overlay = None;
        }
        closed.into_iter().chain(reopened).chain(relisted).collect()
    }

    /// Drops the sheets of connections no longer in `connections`, except the one on screen.
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
        let changed_now: Vec<Connection> = self.sheets.iter().filter_map(changed).collect();
        changed_now.into_iter().filter_map(|now| self.reopen(now)).collect()
    }

    /// Opens a new session on the sheet of `connection`, as the config now says; a closed sheet only takes the new
    /// connection and stays closed.
    fn reopen(&mut self, connection: Connection) -> Option<Action> {
        let sheet = self.sheet_of_mut(connection.name())?;
        if sheet.session == SessionState::Closed {
            sheet.take_closed(connection);
            return None;
        }
        let session = self.new_session();
        self.sheet_of_mut(connection.name())?.reopen(connection.clone(), session);
        Some(Action::Reopen { connection, session })
    }

    /// Shows why the change `id` failed: in the form that sent it, which takes back its typed password, or else in the
    /// status line.
    pub(super) fn config_failed(&mut self, id: ConfigChangeId, why: String, password: Option<Secret>) {
        match &mut self.overlay {
            Some(Overlay::Form(form)) if form.is_waiting_on(id) => form.failed(why, password),
            _ => self.config_failure = Some(why),
        }
    }

    fn is_form_waiting_on(&self, id: ConfigChangeId) -> bool {
        matches!(&self.overlay, Some(Overlay::Form(form)) if form.is_waiting_on(id))
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
    use super::super::fixtures::first_run;
    use super::super::{Answer, ConfigAnswer, Incoming, QueryAnswer, Run, RunId, SessionId, SessionState};
    use super::*;
    use crate::connection::WritePolicy;
    use crate::failure::{Failure, Lost};
    use crate::fixtures::{named, orders};
    use crate::tui::theme::Theme;
    use crossterm::event::KeyModifiers;
    use std::time::Duration;

    /// The app with three connections, showing `local`.
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

    pub(in crate::tui) fn switch_to(app: &mut App, typed: &str) -> Vec<Action> {
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
        assert!(matches!(&*actions, [Action::Open { connection: prod, .. }, _] if prod.name() == &name("prod")), "{actions:?}");
        assert_eq!((app.sheet().name(), &app.sheet().session, app.overlay.is_none()), (&name("prod"), &SessionState::Opening, true));
    }

    #[test]
    fn switching_back_is_instant_and_finds_the_editor_and_result_as_they_were() {
        let mut app = on_local();
        press(&mut app, "SELECT 1");
        key(&mut app, KeyCode::Enter);
        app.apply(Answer::Session {
            connection: name("local"),
            session: SessionId::FIRST,
            incoming: Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::ZERO },
        });
        switch_to(&mut app, "prod");
        assert_eq!(app.sheet().editor.text(), "");
        assert_eq!(switch_to(&mut app, "local"), [Action::ListQueries(name("local"))]);
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
            session: SessionId::FIRST,
            incoming: Incoming::Result { run: RunId::FIRST, result: orders(), took: Duration::ZERO },
        });
        assert_eq!(app.sheet().run, Run::Idle);
        assert!(matches!(app.sheet_of(&name("local")).unwrap().run, Run::Done { .. }));
    }

    fn listed_on_local(app: &mut App) {
        let path = crate::query::QueryPath::parse("mine.sql").unwrap();
        let queries = vec![crate::query::Query { folder: name("local"), path }];
        app.apply(Answer::Queries(QueryAnswer::All { folder: name("local"), queries }));
    }

    #[test]
    fn switching_lists_the_queries_of_the_new_connection_and_never_shows_the_old_ones() {
        let mut app = on_local();
        listed_on_local(&mut app);
        let actions = switch_to(&mut app, "prod");
        assert_eq!(actions.last(), Some(&Action::ListQueries(name("prod"))));
        assert_eq!(app.queries.all, []);
    }

    #[test]
    fn a_list_for_a_connection_no_longer_on_screen_is_dropped() {
        let mut app = on_local();
        switch_to(&mut app, "prod");
        listed_on_local(&mut app);
        assert_eq!(app.queries.all, []);
    }

    #[test]
    fn switching_to_a_connection_that_shares_the_queries_keeps_the_list() {
        let local = named("local", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n");
        let replica = named("replica", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nqueries = \"local\"\n");
        let mut app = App::new(vec![local.clone(), replica], &local, Theme::default());
        listed_on_local(&mut app);
        let actions = switch_to(&mut app, "replica");
        assert!(matches!(&*actions, [Action::Open { .. }]), "{actions:?}");
        assert_eq!(app.queries.all.len(), 1);
    }

    #[test]
    fn a_saved_query_stays_with_the_connection_it_was_opened_on() {
        let mut app = on_local();
        let customers = crate::query::Query { folder: name("local"), path: crate::query::QueryPath::parse("customers.sql").unwrap() };
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: name("local"), query: customers.clone(), text: "SELECT 1".into() }));
        switch_to(&mut app, "prod");
        assert_eq!(app.sheet().opened, None);
        switch_to(&mut app, "local");
        assert_eq!(app.sheet().opened, Some(customers));
    }

    #[test]
    fn a_saved_query_read_after_a_switch_lands_on_the_sheet_that_asked_for_it() {
        let mut app = on_local();
        let customers = crate::query::Query { folder: name("local"), path: crate::query::QueryPath::parse("customers.sql").unwrap() };
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
        assert_eq!(run, [first_run(&name("prod"), "SELECT 1")]);
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
        let session = app.sheet().session_id;
        app.apply(Answer::Session { connection: name("prod"), session, incoming: Incoming::NotOpened(Failure::Login("no route".into())) });
        switch_to(&mut app, "local");
        let actions = switch_to(&mut app, "prod");
        assert!(matches!(&*actions, [Action::Open { connection: prod, .. }, _] if prod.name() == &name("prod")), "{actions:?}");
        assert_eq!(app.sheet().session, SessionState::Opening);
    }

    #[test]
    fn a_connection_added_while_the_picker_is_open_is_listed() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        let mut connections = app.connections.clone();
        connections.push(named("replica", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n"));
        app.take_connections(ConfigChangeId::FIRST, connections, None);
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
        assert_eq!(press(&mut app, "y"), [Action::RemoveConnection { id: ConfigChangeId::FIRST, connection: name("local") }]);
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
        assert_eq!(app.sheet_of(&name("local")).map(|sheet| &sheet.session), Some(&SessionState::Closed));
        assert_eq!(app.sheet().name(), &name("prod"));
    }

    #[test]
    fn ctrl_x_keeps_the_text_of_the_editor_it_closes() {
        let mut app = on_local();
        press(&mut app, "SELECT 1");
        switch_to(&mut app, "prod");
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'x');
        assert_eq!(app.sheet_of(&name("local")).map(|sheet| sheet.editor.text()), Some("SELECT 1"));
    }

    #[test]
    fn a_connection_closed_with_ctrl_x_opens_again_once_picked() {
        let mut app = on_prod_with_local_open();
        ctrl(&mut app, 'x');
        let local = app.connections[0].clone();
        assert!(
            matches!(&*key(&mut app, KeyCode::Enter), [Action::Open { connection, .. }, Action::ListQueries(_)] if *connection == local)
        );
        assert_eq!(app.sheet().session, SessionState::Opening);
    }

    #[test]
    fn a_late_word_from_a_session_closed_with_ctrl_x_leaves_it_closed() {
        let mut app = on_prod_with_local_open();
        ctrl(&mut app, 'x');
        app.apply(Answer::Session { connection: name("local"), session: SessionId::FIRST, incoming: Incoming::Opened });
        assert_eq!(app.sheet_of(&name("local")).map(|sheet| &sheet.session), Some(&SessionState::Closed));
    }

    #[test]
    fn words_of_a_session_closed_with_ctrl_x_never_reach_the_one_opened_after_it() {
        let mut app = on_prod_with_local_open();
        let closed = app.sheet_of(&name("local")).unwrap().session_id;
        ctrl(&mut app, 'x');
        key(&mut app, KeyCode::Enter);
        app.apply(Answer::Session { connection: name("local"), session: closed, incoming: Incoming::Opened });
        app.apply(Answer::Session {
            connection: name("local"),
            session: closed,
            incoming: Incoming::Closed(Lost::LinkFailed("gone".into())),
        });
        assert_eq!(app.sheet().session, SessionState::Opening);
    }

    #[test]
    fn a_statement_running_when_ctrl_x_closes_its_session_fails() {
        let mut app = on_local();
        press(&mut app, "SELECT pg_sleep(5)");
        key(&mut app, KeyCode::Enter);
        switch_to(&mut app, "prod");
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'x');
        assert!(matches!(app.sheet_of(&name("local")).map(|sheet| &sheet.run), Some(Run::Failed(_))));
    }

    #[test]
    fn ctrl_x_keeps_the_picker_open_and_says_the_session_closed() {
        let mut app = on_prod_with_local_open();
        ctrl(&mut app, 'x');
        assert_eq!(picker(&app).note, Some(Note::Said("closed the session of `local`".into())));
    }

    #[test]
    fn a_connection_gone_from_the_config_closes_and_one_changed_opens_again_as_it_is_now() {
        let mut app = on_prod_with_local_open();
        let prod =
            named("prod", "tier = \"production\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\nwrites = \"allowed\"\n");
        let answer = ConfigAnswer::Changed { connections: vec![prod.clone()], warning: None };
        let actions = app.apply(Answer::Config { id: ConfigChangeId::FIRST, answer });
        assert!(
            matches!(&*actions, [Action::Close(local), Action::Reopen { connection, .. }] if *local == name("local") && *connection == prod)
        );
        assert_eq!(app.sheet().connection.writes(), WritePolicy::Allowed);
        assert_eq!(app.connections.len(), 1);
    }

    #[test]
    fn a_closed_sheet_takes_its_changed_connection_without_opening_again() {
        let mut app = on_prod_with_local_open();
        ctrl(&mut app, 'x');
        let local = named("local", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nwrites = \"allowed\"\n");
        let prod = app.connections[1].clone();
        let actions = app.apply(Answer::Config {
            id: ConfigChangeId::FIRST,
            answer: ConfigAnswer::Changed { connections: vec![local, prod], warning: None },
        });
        let sheet = app.sheet_of(&name("local")).unwrap();
        assert_eq!((actions, &sheet.session, sheet.connection.writes()), (vec![], &SessionState::Closed, WritePolicy::Allowed));
    }

    #[test]
    fn a_save_whose_keychain_step_failed_closes_the_form_and_warns_in_the_status_line() {
        let mut app = on_prod_with_local_open();
        let connections = app.connections.clone();
        let warning = Some("saved `prod`, but the password did not reach the keychain".to_owned());
        app.apply(Answer::Config { id: ConfigChangeId::FIRST, answer: ConfigAnswer::Changed { connections, warning: warning.clone() } });
        assert_eq!(app.config_failure, warning);
    }

    fn failed(why: &str) -> Answer {
        Answer::Config { id: ConfigChangeId::FIRST, answer: ConfigAnswer::Failed { why: why.into(), password: None } }
    }

    #[test]
    fn a_removal_that_failed_says_why_in_the_status_line_until_the_next_key() {
        let mut app = on_prod_with_local_open();
        ctrl(&mut app, 'd');
        press(&mut app, "y");
        app.apply(failed("permission denied"));
        assert_eq!(app.config_failure.as_deref(), Some("permission denied"));
        press(&mut app, "x");
        assert_eq!(app.config_failure, None);
    }

    #[test]
    fn a_change_that_failed_leaves_the_running_statement_as_it_is() {
        let mut app = on_local();
        press(&mut app, "SELECT 1");
        key(&mut app, KeyCode::Enter);
        app.apply(failed("permission denied"));
        assert!(matches!(app.sheet().run, Run::Running { .. }), "{:?}", app.sheet().run);
    }

    #[test]
    fn a_statement_running_when_its_connection_changes_fails_as_its_session_opens_again() {
        let mut app = on_local();
        press(&mut app, "SELECT pg_sleep(5)");
        key(&mut app, KeyCode::Enter);
        let local = named("local", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nwrites = \"allowed\"\n");
        let mut connections = app.connections.clone();
        connections[0] = local;
        app.apply(Answer::Config { id: ConfigChangeId::FIRST, answer: ConfigAnswer::Changed { connections, warning: None } });
        assert_eq!(app.sheet().run, Run::Failed("cancelled: its session was reopened".into()));
    }
}
