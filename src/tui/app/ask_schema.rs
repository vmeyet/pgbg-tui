//! An ask from `ctrl-g` and the schema it goes with: the ask waits while the schema is on its way, starts its read once
//! the session opens, and says why when the schema will not come.
//! The idle ghost never waits: it asks with what is read so far.
use super::ask::AskStep;
use super::{Action, App, AskFailure, Overlay, SchemaState, SessionState, Sheet};
use crate::connection::SchemaReading;

/// What the schema lets an ask from `ctrl-g` do now.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SchemaForAsk {
    /// It is read, or the connection says `schema = "off"` and the ask goes without it.
    Ready,
    OnItsWay,
    /// It will not come, and why.
    Missing(String),
}

impl App {
    /// Whether an open ask will need the schema: from its instruction until it is sent.
    pub(super) fn is_ask_before_sending(&self) -> bool {
        matches!(self.overlay, Some(Overlay::Ask(AskStep::Instruction(_) | AskStep::WaitingForSchema { .. })))
    }

    /// Ends the wait of an ask held for the schema once the schema's fate is known: sends it, or says why it did not go;
    /// an ask whose editor changed meanwhile is dropped and says so.
    pub(super) fn send_ask_waiting_for_schema(&mut self) -> Vec<Action> {
        let Some(Overlay::Ask(AskStep::WaitingForSchema { revision, .. })) = self.overlay else { return vec![] };
        if self.sheet().schema_for_ask() == SchemaForAsk::OnItsWay {
            return vec![];
        }
        let Some(Overlay::Ask(AskStep::WaitingForSchema { instruction, .. })) = self.overlay.take() else { return vec![] };
        if revision != self.sheet().editor.revision() {
            self.asks.failure = Some(AskFailure::EditorChanged);
            return vec![];
        }
        self.send_ask(instruction)
    }
}

impl Sheet {
    /// Whether an ask from `ctrl-g` goes now, waits for the schema, or cannot have it: a session opening or open brings it,
    /// a failed or closed one does not.
    pub(super) fn schema_for_ask(&self) -> SchemaForAsk {
        if self.connection.schema() == SchemaReading::Off {
            return SchemaForAsk::Ready;
        }
        match (&self.schema, &self.session) {
            (SchemaState::Read(_), _) => SchemaForAsk::Ready,
            (SchemaState::Failed(why), _) | (SchemaState::NotRead, SessionState::Failed(why)) => SchemaForAsk::Missing(why.clone()),
            (SchemaState::NotRead, SessionState::Closed) => SchemaForAsk::Missing("the session is closed".to_owned()),
            (SchemaState::Reading, _) | (SchemaState::NotRead, SessionState::Opening | SessionState::Reconnecting | SessionState::Open) => {
                SchemaForAsk::OnItsWay
            }
        }
    }
}

#[cfg(test)]
pub(in crate::tui) mod tests {
    use super::super::ask::tests::{ai_settings, ctrl_g, open_in_the_editor, press, step, type_in, waiting};
    use super::super::fixtures::{app_on, connection, shop, to_shop};
    use super::super::{Answer, Incoming, SessionId};
    use super::*;
    use crate::failure::{Failure, Lost};
    use crate::fixtures::{self, shop_catalog};
    use crossterm::event::KeyCode;
    use std::sync::Arc;

    /// The app on `shop` with an `[ai] command`, its session still opening, `text` in the editor.
    fn asking_while_opening(text: &str) -> App {
        let mut app = app_on(&connection("local", "read-only")).with_ai(Some(ai_settings()));
        app.sheet_mut().paste(text);
        app
    }

    /// Opens an ask and sends the instruction `ids`.
    fn send_ids(app: &mut App) -> Vec<Action> {
        ctrl_g(app);
        type_in(app, "ids");
        press(app, KeyCode::Enter)
    }

    /// The app holding the instruction `ids` about `SELECT * FROM orders`, its session open and its schema being read.
    pub(in crate::tui) fn held_for_the_schema() -> App {
        let mut app = asking_while_opening("SELECT * FROM orders");
        app.apply(to_shop(Incoming::Opened));
        assert_eq!(send_ids(&mut app), []);
        app
    }

    fn schema_read(app: &mut App) -> Vec<Action> {
        app.apply(to_shop(Incoming::Schema(Ok(Arc::new(shop_catalog())))))
    }

    pub(in crate::tui) fn schema_refused() -> Failure {
        Failure::Server { message: "permission denied for table pg_class".into(), position: None, code: None }
    }

    #[test]
    fn enter_before_the_schema_is_read_holds_the_ask_and_sends_nothing() {
        let app = held_for_the_schema();
        assert!(matches!(step(&app), Some(AskStep::WaitingForSchema { .. })), "{:?}", step(&app));
    }

    #[test]
    fn the_schema_arriving_sends_the_held_ask_with_its_tables() {
        let mut app = held_for_the_schema();
        let sent = schema_read(&mut app);
        let [Action::Ask { ask, prompt, .. }] = &*sent else { panic!("{sent:?}") };
        assert!(prompt.text().contains("ids") && prompt.text().contains("customers"), "{}", prompt.text());
        assert!(matches!(step(&app), Some(AskStep::Waiting { ask: waiting, .. }) if waiting == ask));
    }

    #[test]
    fn esc_while_held_sends_nothing_when_the_schema_arrives() {
        let mut app = held_for_the_schema();
        assert_eq!(press(&mut app, KeyCode::Esc), []);
        assert_eq!(schema_read(&mut app), []);
        assert_eq!(step(&app), None);
    }

    #[test]
    fn a_schema_for_another_session_does_not_send_the_held_ask() {
        let mut app = held_for_the_schema();
        let incoming = Incoming::Schema(Ok(Arc::new(shop_catalog())));
        assert_eq!(app.apply(Answer::Session { connection: shop(), session: SessionId::FIRST.next(), incoming }), []);
        assert!(matches!(step(&app), Some(AskStep::WaitingForSchema { .. })), "{:?}", step(&app));
    }

    #[test]
    fn a_held_ask_whose_editor_changed_is_dropped_and_says_why() {
        let mut app = held_for_the_schema();
        open_in_the_editor(&mut app, "SELECT 1");
        assert_eq!(schema_read(&mut app), []);
        assert_eq!((step(&app), app.ask_failure()), (None, Some(&AskFailure::EditorChanged)));
    }

    #[test]
    fn ctrl_g_while_the_session_opens_reads_the_schema_once_it_opens() {
        let mut app = asking_while_opening("SELECT 1");
        assert_eq!(ctrl_g(&mut app), []);
        assert_eq!(app.apply(to_shop(Incoming::Opened)), [Action::ReadSchema { connection: shop(), session: SessionId::FIRST }]);
    }

    #[test]
    fn ctrl_g_on_an_open_session_reads_the_schema() {
        let mut app = asking_while_opening("SELECT 1");
        app.apply(to_shop(Incoming::Opened));
        assert_eq!(ctrl_g(&mut app), [Action::ReadSchema { connection: shop(), session: SessionId::FIRST }]);
    }

    #[test]
    fn a_held_ask_reads_the_schema_of_the_session_its_sheet_reopened_on() {
        let mut app = held_for_the_schema();
        let connection = app.sheet().connection.clone();
        let session = SessionId::FIRST.next();
        app.sheet_mut().reopen(connection, session);
        let opened = app.apply(Answer::Session { connection: shop(), session, incoming: Incoming::Opened });
        assert_eq!(opened, [Action::ReadSchema { connection: shop(), session }]);
    }

    #[test]
    fn an_ask_sent_while_the_session_opens_goes_once_it_opened_and_the_schema_came() {
        let mut app = asking_while_opening("SELECT 1");
        assert_eq!(send_ids(&mut app), []);
        app.apply(to_shop(Incoming::Opened));
        assert!(matches!(&*schema_read(&mut app), [Action::Ask { .. }]));
    }

    #[test]
    fn an_ask_dropped_while_the_session_opens_reads_no_schema() {
        let mut app = asking_while_opening("SELECT 1");
        ctrl_g(&mut app);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.apply(to_shop(Incoming::Opened)), []);
    }

    #[test]
    fn a_failed_read_drops_the_held_ask_and_says_why() {
        let mut app = held_for_the_schema();
        assert_eq!(app.apply(to_shop(Incoming::Schema(Err(schema_refused())))), []);
        let why = AskFailure::NoSchema(schema_refused().to_string());
        assert_eq!((step(&app), app.ask_failure()), (None, Some(&why)));
    }

    #[test]
    fn a_session_that_does_not_open_drops_the_held_ask_and_says_why() {
        let mut app = asking_while_opening("SELECT 1");
        send_ids(&mut app);
        assert_eq!(app.apply(to_shop(Incoming::NotOpened(Failure::Login("connection refused".into())))), []);
        let why = AskFailure::NoSchema("connection refused".to_owned());
        assert_eq!((step(&app), app.ask_failure()), (None, Some(&why)));
    }

    #[test]
    fn a_session_lost_during_the_read_drops_the_held_ask_and_says_why() {
        let mut app = held_for_the_schema();
        assert_eq!(app.apply(to_shop(Incoming::Closed(Lost::LinkFailed("terminating connection".into())))), []);
        assert!(matches!(app.ask_failure(), Some(AskFailure::NoSchema(_))), "{:?}", app.ask_failure());
    }

    #[test]
    fn enter_on_a_closed_session_with_no_schema_sends_nothing_and_says_why() {
        let mut app = asking_while_opening("SELECT 1");
        app.sheet_mut().close();
        assert_eq!(send_ids(&mut app), []);
        let why = AskFailure::NoSchema("the session is closed".to_owned());
        assert_eq!((step(&app), app.ask_failure()), (None, Some(&why)));
    }

    #[test]
    fn enter_on_a_closed_session_with_its_schema_read_still_sends() {
        let mut app = waiting("SELECT 1", "ids");
        press(&mut app, KeyCode::Esc);
        app.sheet_mut().close();
        ctrl_g(&mut app);
        type_in(&mut app, "names");
        assert!(matches!(&*press(&mut app, KeyCode::Enter), [Action::Ask { .. }]));
    }

    #[test]
    fn a_connection_with_schema_off_sends_at_once_while_the_session_opens() {
        let off = fixtures::connection("tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nschema = \"off\"\n");
        let mut app = app_on(&off).with_ai(Some(ai_settings()));
        assert!(matches!(&*send_ids(&mut app), [Action::Ask { .. }]));
    }

    #[test]
    fn ctrl_g_after_a_failed_read_reads_the_schema_again() {
        let mut app = held_for_the_schema();
        app.apply(to_shop(Incoming::Schema(Err(schema_refused()))));
        assert_eq!(ctrl_g(&mut app), [Action::ReadSchema { connection: shop(), session: SessionId::FIRST }]);
    }
}
