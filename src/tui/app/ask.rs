//! `ctrl-g`: an instruction typed on the footer line, the `[ai] command` asked about the statement under the cursor,
//! and its answer, shown until `esc` drops it; nothing of it ever runs by itself.
use super::save::typed_into;
use super::{Action, App, AskFailure, AskId, Overlay, Sheet};
use crate::act::{self, Act, Pane};
use crate::ai::{self, Instruction, LiteralPolicy, NotSent, Prompt};
use crate::config::AiSettings;
use crate::connection::AiAsking;
use crate::db::model::{Catalog, Statement};
use crossterm::event::KeyEvent;
use vtui::field::Field;

/// Where an ask stands while it takes the keys.
#[derive(Debug, PartialEq)]
pub(in crate::tui) enum AskStep {
    /// The instruction, as typed so far.
    Instruction(Field),
    /// The command runs on this ask; `esc` drops it.
    Waiting(AskId),
    /// The answer, until `esc` drops it.
    Answered(Vec<Statement>),
}

/// What asking needs from the config, the id the next ask takes, and why the last one failed.
#[derive(Debug)]
pub(super) struct Asks {
    /// None when `[ai]` names no command, so asking is off.
    settings: Option<AiSettings>,
    next: AskId,
    /// Why the last ask shows no answer; the status line says it until the next key.
    pub(super) failure: Option<AskFailure>,
}

impl Asks {
    pub(super) fn new(settings: Option<AiSettings>) -> Self {
        Self { settings, next: AskId::FIRST, failure: None }
    }

    fn take_id(&mut self) -> AskId {
        let id = self.next;
        self.next = id.next();
        id
    }
}

impl AskStep {
    /// The pane of the action table whose keys work while it is open.
    pub(in crate::tui) const fn pane(&self) -> Pane {
        match self {
            Self::Instruction(_) => Pane::Instruction,
            Self::Waiting(_) => Pane::Asking,
            Self::Answered(_) => Pane::Answer,
        }
    }
}

impl App {
    /// Opens the instruction line, or says why asking is off; the schema is read meanwhile, for the prompt's slice.
    pub(super) fn open_ask(&mut self) -> Vec<Action> {
        if self.sheet().connection.ai() == AiAsking::Off {
            self.asks.failure = Some(AskFailure::Off);
            return vec![];
        }
        if self.asks.settings.is_none() {
            self.asks.failure = Some(AskFailure::NoCommand);
            return vec![];
        }
        self.overlay = Some(Overlay::Ask(AskStep::Instruction(Field::default())));
        self.sheet_mut().want_schema_while_open()
    }

    /// Hands `key` to the open ask: `⏎` sends the instruction, `esc` drops the ask at any step; `⏎` on an answer does
    /// nothing, so taking it and running it are always two keys.
    pub(super) fn ask_key(&mut self, step: AskStep, key: KeyEvent) -> Vec<Action> {
        match (act::act_for(key, step.pane()), step) {
            (Some(Act::Quit), _) => self.quit(),
            (Some(Act::Close), AskStep::Waiting(ask)) => vec![Action::DropAsk(ask)],
            (Some(Act::Close), _) => vec![],
            (Some(Act::Open), AskStep::Instruction(instruction)) => self.send_ask(instruction),
            (_, AskStep::Instruction(mut instruction)) => {
                typed_into(&mut instruction, key);
                self.overlay = Some(Overlay::Ask(AskStep::Instruction(instruction)));
                vec![]
            }
            (_, step) => {
                self.overlay = Some(Overlay::Ask(step));
                vec![]
            }
        }
    }

    /// Asks with `instruction` unless it is blank, or says why the prompt is not sent.
    fn send_ask(&mut self, instruction: Field) -> Vec<Action> {
        let Some(settings) = self.asks.settings.as_ref().filter(|_| !instruction.text().trim().is_empty()) else {
            self.overlay = Some(Overlay::Ask(AskStep::Instruction(instruction)));
            return vec![];
        };
        let settings = settings.without_passwords_of(&self.connections);
        match self.sheet().prompt(&instruction, settings.literals) {
            Err(not_sent) => {
                self.asks.failure = Some(AskFailure::NotSent(not_sent));
                vec![]
            }
            Ok(prompt) => {
                let ask = self.asks.take_id();
                self.overlay = Some(Overlay::Ask(AskStep::Waiting(ask)));
                vec![Action::Ask { ask, prompt, settings }]
            }
        }
    }

    /// Shows the answer to `ask` while the footer still waits on it; a late answer is dropped.
    pub(super) fn asked(&mut self, ask: AskId, answer: Result<Vec<Statement>, AskFailure>) {
        if self.overlay != Some(Overlay::Ask(AskStep::Waiting(ask))) {
            return;
        }
        match answer {
            Ok(statements) => self.overlay = Some(Overlay::Ask(AskStep::Answered(statements))),
            Err(failure) => {
                self.overlay = None;
                self.asks.failure = Some(failure);
            }
        }
    }

    pub(in crate::tui) fn ask_failure(&self) -> Option<&AskFailure> {
        self.asks.failure.as_ref()
    }
}

impl Sheet {
    /// The prompt for `instruction` about the statement under the cursor, with the schema read so far.
    fn prompt(&self, instruction: &Field, literals: LiteralPolicy) -> Result<Prompt, NotSent> {
        let instruction = Instruction::new(instruction.text());
        let statement = self.range_under_cursor().map(|range| Statement::new(&self.editor.text()[range]));
        let unread = Catalog::default();
        let catalog = self.catalog().unwrap_or(&unread);
        ai::prompt(&ai::Ask { instruction: &instruction, statement: statement.as_ref(), catalog, tier: self.connection.tier(), literals })
    }
}

#[cfg(test)]
pub(in crate::tui) mod tests {
    use super::super::tests::{app_on, connection, to_shop};
    use super::super::{Answer, Incoming, Run};
    use super::*;
    use crate::fixtures::{self, shop_catalog};
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::sync::Arc;
    use std::time::Duration;

    fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code))
    }

    fn ctrl_g(app: &mut App) -> Vec<Action> {
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL))
    }

    fn type_in(app: &mut App, text: &str) {
        text.chars().for_each(|c| drop(press(app, KeyCode::Char(c))));
    }

    fn ai_settings() -> AiSettings {
        let env = ["PATH", "SHOP_PGPASSWORD"].map(str::to_owned).to_vec();
        AiSettings { command: vec!["llm".to_owned()], env, timeout: Duration::from_secs(60), literals: LiteralPolicy::Masked }
    }

    /// The app on `shop` with an `[ai] command`, its schema read, `text` in the editor.
    pub(in crate::tui) fn asking_on(text: &str) -> App {
        let mut app = app_on(&connection("local", "read-only")).with_ai(Some(ai_settings()));
        app.apply(to_shop(Incoming::Opened));
        app.apply(to_shop(Incoming::Schema(Ok(Arc::new(shop_catalog())))));
        type_in(&mut app, text);
        app
    }

    /// The app waiting on the first ask, sent with `instruction` about `text`.
    pub(in crate::tui) fn waiting(text: &str, instruction: &str) -> App {
        let mut app = asking_on(text);
        ctrl_g(&mut app);
        type_in(&mut app, instruction);
        press(&mut app, KeyCode::Enter);
        app
    }

    /// The app showing `answer` to the first ask about `text`.
    pub(in crate::tui) fn answered(text: &str, answer: &str) -> App {
        let mut app = waiting(text, "only the ids");
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(vec![Statement::new(answer)]) });
        app
    }

    fn step(app: &App) -> Option<&AskStep> {
        match app.overlay() {
            Some(Overlay::Ask(step)) => Some(step),
            _ => None,
        }
    }

    #[test]
    fn enter_sends_the_instruction_and_waits() {
        let mut app = asking_on("SELECT * FROM orders WHERE total > 10");
        ctrl_g(&mut app);
        type_in(&mut app, "only the ids");
        let sent = press(&mut app, KeyCode::Enter);
        let [Action::Ask { ask, prompt, .. }] = &*sent else { panic!("{sent:?}") };
        assert!(prompt.text().contains("only the ids") && prompt.text().contains("total > <number 1>"), "{}", prompt.text());
        assert_eq!(step(&app), Some(&AskStep::Waiting(*ask)));
    }

    #[test]
    fn an_ask_passes_no_variable_a_connection_added_since_names_as_its_password() {
        let mut app = asking_on("SELECT 1");
        let added = fixtures::named(
            "archive",
            "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\npassword = { env = \"SHOP_PGPASSWORD\" }\n",
        );
        app.connections.push(added);
        ctrl_g(&mut app);
        type_in(&mut app, "ids");
        let sent = press(&mut app, KeyCode::Enter);
        let [Action::Ask { settings, .. }] = &*sent else { panic!("{sent:?}") };
        assert_eq!(settings.env, ["PATH"]);
    }

    #[test]
    fn enter_on_a_blank_instruction_sends_nothing() {
        let mut app = asking_on("SELECT 1");
        ctrl_g(&mut app);
        type_in(&mut app, "  ");
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert!(matches!(step(&app), Some(AskStep::Instruction(_))));
    }

    #[test]
    fn a_statement_holding_a_secret_is_not_sent_and_says_why() {
        let mut app = asking_on("ALTER ROLE nina PASSWORD 'x'");
        ctrl_g(&mut app);
        type_in(&mut app, "rename nina");
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert_eq!((step(&app), app.ask_failure()), (None, Some(&AskFailure::NotSent(NotSent::HoldsSecret))));
    }

    #[test]
    fn ctrl_g_without_an_ai_command_says_so_and_sends_nothing() {
        let mut app = app_on(&connection("local", "read-only"));
        assert_eq!(ctrl_g(&mut app), []);
        assert_eq!((step(&app), app.ask_failure()), (None, Some(&AskFailure::NoCommand)));
    }

    #[test]
    fn ctrl_g_on_a_connection_with_ai_off_says_so_and_sends_nothing() {
        let off = fixtures::connection("tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nai = \"off\"\n");
        let mut app = app_on(&off).with_ai(Some(ai_settings()));
        assert_eq!(ctrl_g(&mut app), []);
        assert_eq!((step(&app), app.ask_failure()), (None, Some(&AskFailure::Off)));
    }

    #[test]
    fn esc_while_waiting_drops_the_ask() {
        let mut app = waiting("SELECT 1", "ids");
        assert_eq!(press(&mut app, KeyCode::Esc), [Action::DropAsk(AskId::FIRST)]);
        assert_eq!(step(&app), None);
    }

    #[test]
    fn an_answer_to_a_dropped_ask_is_dropped() {
        let mut app = waiting("SELECT 1", "ids");
        press(&mut app, KeyCode::Esc);
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(vec![Statement::new("SELECT 2")]) });
        assert_eq!((step(&app), app.sheet().editor.text()), (None, "SELECT 1"));
    }

    #[test]
    fn esc_drops_the_answer_and_keeps_the_editor() {
        let mut app = answered("SELECT 1", "SELECT 2");
        assert_eq!(press(&mut app, KeyCode::Esc), []);
        assert_eq!((step(&app), app.sheet().editor.text()), (None, "SELECT 1"));
    }

    #[test]
    fn enter_on_an_answer_neither_runs_nor_takes_it() {
        let mut app = answered("SELECT 1", "SELECT 2");
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert!(matches!(step(&app), Some(AskStep::Answered(_))));
        assert_eq!((app.sheet().editor.text(), &app.sheet().run), ("SELECT 1", &Run::Idle));
    }

    #[test]
    fn the_failure_clears_on_the_next_key() {
        let mut app = app_on(&connection("local", "read-only"));
        ctrl_g(&mut app);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.ask_failure(), None);
    }
}
