//! `ctrl-g`: an instruction typed on the footer line, the `[ai] command` asked about the statement under the cursor,
//! and its answer, which `tab` puts in the editor; nothing of it ever runs by itself.
use super::diff::{self, DiffLine};
use super::editor::{Edited, Revision};
use super::idle_ghost::IdleAsk;
use super::save::typed_into;
use super::{Action, App, AskFailure, AskId, Overlay, Sheet};
use crate::act::{self, Act, Pane};
use crate::ai::{self, Instruction, LiteralPolicy, NotSent, Prompt};
use crate::config::AiSettings;
use crate::connection::AiAsking;
use crate::db::guard::{self, Write};
use crate::db::lex;
use crate::db::model::{Catalog, Statement};
use crossterm::event::KeyEvent;
use std::ops::Range;
use vtui::field::Field;

/// Where an ask stands while it takes the keys.
#[derive(Debug, PartialEq)]
pub(in crate::tui) enum AskStep {
    /// The instruction, as typed so far.
    Instruction(Field),
    /// The command runs on `ask`, asked about the editor at `revision`; `esc` drops it.
    Waiting { ask: AskId, revision: Revision },
    /// The answer, until `tab` takes it or `esc` drops it.
    Answered(Suggestion),
}

/// An answer, and the statement under the cursor it would replace.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::tui) struct Suggestion {
    /// The editor it was asked about; `replaces` fits that one only.
    revision: Revision,
    /// The editor bytes it replaces: the statement under the cursor, or none to go in at the cursor.
    replaces: Option<Range<usize>>,
    /// The answer's statements, one after the other.
    text: String,
    /// The statement it replaces, then the answer, line by line.
    pub(in crate::tui) lines: Vec<DiffLine>,
    /// Each write the guard reads among them, in order.
    pub(in crate::tui) writes: Vec<Write>,
}

/// What asking needs from the config, the id the next ask takes, and why the last one failed.
#[derive(Debug)]
pub(super) struct Asks {
    /// None when `[ai]` names no command, so asking is off.
    settings: Option<AiSettings>,
    next: AskId,
    /// Where the idle ghost's ask stands.
    pub(super) idle: IdleAsk,
    /// Why the last ask shows no answer; the status line says it until the next key.
    pub(super) failure: Option<AskFailure>,
}

impl Asks {
    pub(super) fn new(settings: Option<AiSettings>) -> Self {
        Self { settings, next: AskId::FIRST, idle: IdleAsk::default(), failure: None }
    }

    pub(super) fn settings(&self) -> Option<&AiSettings> {
        self.settings.as_ref()
    }

    pub(super) fn take_id(&mut self) -> AskId {
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
            Self::Waiting { .. } => Pane::Asking,
            Self::Answered(_) => Pane::Answer,
        }
    }
}

impl App {
    /// Opens the instruction line, or says why asking is off; the schema is read meanwhile, for the prompt's slice.
    pub(super) fn open_ask(&mut self) -> Vec<Action> {
        if let Some(why) = self.why_asking_is_off() {
            self.asks.failure = Some(why);
            return vec![];
        }
        self.overlay = Some(Overlay::Ask(AskStep::Instruction(Field::default())));
        self.sheet_mut().want_schema_while_open()
    }

    /// Hands `key` to the open ask: `⏎` sends the instruction, `esc` drops the ask at any step, `tab` takes an answer;
    /// `⏎` on an answer does nothing, so taking it and running it are always two keys.
    pub(super) fn ask_key(&mut self, step: AskStep, key: KeyEvent) -> Vec<Action> {
        match (act::act_for(key, step.pane()), step) {
            (Some(Act::Quit), _) => self.quit(),
            (Some(Act::Close), AskStep::Waiting { ask, .. }) => vec![Action::DropAsk(ask)],
            (Some(Act::Close), _) => vec![],
            (Some(Act::Open), AskStep::Instruction(instruction)) => self.send_ask(instruction),
            (Some(Act::Insert), AskStep::Answered(suggestion)) => {
                self.take_suggestion(suggestion);
                vec![]
            }
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
        if let Some(why) = self.why_asking_is_off() {
            self.asks.failure = Some(why);
            return vec![];
        }
        let Some(settings) = self.asks.settings.as_ref().filter(|_| !instruction.text().trim().is_empty()) else {
            self.overlay = Some(Overlay::Ask(AskStep::Instruction(instruction)));
            return vec![];
        };
        let settings = settings.without_passwords_of(&self.connections);
        match self.sheet().prompt(&Instruction::new(instruction.text()), settings.literals) {
            Err(not_sent) => {
                self.asks.failure = Some(AskFailure::NotSent(not_sent));
                vec![]
            }
            Ok(prompt) => {
                let ask = self.asks.take_id();
                self.overlay = Some(Overlay::Ask(AskStep::Waiting { ask, revision: self.sheet().editor.revision() }));
                vec![Action::Ask { ask, prompt, settings }]
            }
        }
    }

    /// Shows the answer to `ask` while the footer still waits on it; a late answer is dropped, and so is one whose
    /// editor changed meanwhile.
    pub(super) fn asked(&mut self, ask: AskId, answer: Result<Vec<Statement>, AskFailure>) -> Vec<Action> {
        if self.asks.idle.is_waiting_on(ask) {
            return self.show_idle_ghost(answer).into_iter().collect();
        }
        let Some(Overlay::Ask(AskStep::Waiting { ask: waiting, revision })) = self.overlay else { return vec![] };
        if waiting != ask {
            return vec![];
        }
        let answer = answer.and_then(|statements| self.sheet().suggestion(revision, &statements));
        match answer {
            Ok(suggestion) => self.overlay = Some(Overlay::Ask(AskStep::Answered(suggestion))),
            Err(failure) => {
                self.overlay = None;
                self.asks.failure = Some(failure);
            }
        }
        vec![]
    }

    /// Why `ctrl-g` asks nothing on the sheet on screen, as the config stands now.
    fn why_asking_is_off(&self) -> Option<AskFailure> {
        if self.sheet().connection.ai() == AiAsking::Off {
            return Some(AskFailure::Off);
        }
        self.asks.settings.is_none().then_some(AskFailure::NoCommand)
    }

    /// Puts `suggestion` in the editor, unless the editor changed since it was asked about.
    fn take_suggestion(&mut self, suggestion: Suggestion) {
        if suggestion.revision != self.sheet().editor.revision() {
            self.asks.failure = Some(AskFailure::EditorChanged);
            return;
        }
        self.sheet_mut().take_suggestion(suggestion);
    }

    pub(in crate::tui) fn ask_failure(&self) -> Option<&AskFailure> {
        self.asks.failure.as_ref()
    }
}

impl Sheet {
    /// The prompt for `instruction` about the statement under the cursor, with the schema read so far.
    pub(super) fn prompt(&self, instruction: &Instruction, literals: LiteralPolicy) -> Result<Prompt, NotSent> {
        let statement = self.range_under_cursor().map(|range| Statement::new(&self.editor.text()[range]));
        self.prompt_about(instruction, statement.as_ref(), literals)
    }

    /// The prompt for `instruction` about `statement`, with the schema read so far.
    pub(super) fn prompt_about(
        &self,
        instruction: &Instruction,
        statement: Option<&Statement>,
        literals: LiteralPolicy,
    ) -> Result<Prompt, NotSent> {
        let unread = Catalog::default();
        let catalog = self.catalog().unwrap_or(&unread);
        ai::prompt(&ai::PromptInput { instruction, statement, catalog, tier: self.connection.tier(), literals })
    }

    /// `statements` against the statement under the cursor, which they would replace, unless the editor is no longer
    /// at `revision`, the one they were asked about.
    fn suggestion(&self, revision: Revision, statements: &[Statement]) -> Result<Suggestion, AskFailure> {
        if revision != self.editor.revision() {
            return Err(AskFailure::EditorChanged);
        }
        let writes =
            statements.iter().filter_map(|statement| guard::write_in(statement.as_str(), &lex::tokens(statement.as_str()))).collect();
        let text = statements.iter().map(Statement::as_str).collect::<Vec<_>>().join(";\n");
        let replaces = self.range_under_cursor();
        let lines = diff::lines(replaces.clone().map_or("", |range| &self.editor.text()[range]), &text);
        Ok(Suggestion { revision, replaces, text, lines, writes })
    }

    /// Puts the answer in the editor as one paste, in place of the statement it was asked about.
    fn take_suggestion(&mut self, suggestion: Suggestion) {
        let edited = match suggestion.replaces {
            Some(range) => self.editor.replace(range, &suggestion.text),
            None => self.editor.paste(&suggestion.text),
        };
        if edited == Edited::Text {
            self.text_changed();
        }
    }
}

#[cfg(test)]
pub(in crate::tui) mod tests {
    use super::super::fixtures::{app_on, connection, shop, to_shop};
    use super::super::queries::tests::query;
    use super::super::{Answer, Incoming, QueryAnswer, Run};
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

    pub(in crate::tui) fn ai_settings() -> AiSettings {
        let env = ["PATH", "SHOP_PGPASSWORD"].map(str::to_owned).to_vec();
        AiSettings { command: vec!["llm".to_owned()], env, timeout: Duration::from_secs(60), literals: LiteralPolicy::Masked, ghost: false }
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
        assert!(matches!(step(&app), Some(AskStep::Waiting { ask: waiting, .. }) if waiting == ask));
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
    fn an_answer_to_an_older_ask_is_dropped_while_a_newer_one_waits() {
        let mut app = waiting("SELECT 1", "ids");
        press(&mut app, KeyCode::Esc);
        ctrl_g(&mut app);
        type_in(&mut app, "names");
        press(&mut app, KeyCode::Enter);
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(vec![Statement::new("SELECT 2")]) });
        assert!(matches!(step(&app), Some(AskStep::Waiting { ask, .. }) if *ask == AskId::FIRST.next()), "{:?}", step(&app));
    }

    #[test]
    fn an_answer_to_a_dropped_ask_is_dropped() {
        let mut app = waiting("SELECT 1", "ids");
        press(&mut app, KeyCode::Esc);
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(vec![Statement::new("SELECT 2")]) });
        assert_eq!((step(&app), app.sheet().editor.text()), (None, "SELECT 1"));
    }

    #[test]
    fn tab_puts_the_answer_in_place_of_the_statement_under_the_cursor() {
        let mut app = answered("SELECT 1;\nSELECT * FROM orders", "SELECT id FROM orders");
        assert_eq!(press(&mut app, KeyCode::Tab), []);
        assert_eq!((step(&app), app.sheet().editor.text()), (None, "SELECT 1;\nSELECT id FROM orders"));
    }

    #[test]
    fn tab_with_no_statement_under_the_cursor_puts_the_answer_at_the_cursor() {
        let mut app = answered("", "SELECT id FROM orders");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.sheet().editor.text(), "SELECT id FROM orders");
    }

    #[test]
    fn a_taken_answer_drops_its_control_characters_as_a_paste_does() {
        let mut app = answered("SELECT 1", "SELECT '\u{1b}[2J' \u{202e}FROM orders");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.sheet().editor.text(), "SELECT '[2J' FROM orders");
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
    fn a_taken_write_asks_before_it_runs() {
        let mut app = app_on(&connection("production", "allowed")).with_ai(Some(ai_settings()));
        ctrl_g(&mut app);
        type_in(&mut app, "drop the old ones");
        press(&mut app, KeyCode::Enter);
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(vec![Statement::new("DELETE FROM orders")]) });
        press(&mut app, KeyCode::Tab);
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert!(matches!(app.overlay(), Some(Overlay::Prompt(_))), "{:?}", app.overlay());
    }

    #[test]
    fn an_answer_that_writes_names_its_write() {
        let app = answered("SELECT 1", "DELETE FROM orders WHERE id = 7");
        let Some(AskStep::Answered(suggestion)) = step(&app) else { panic!("no answer") };
        assert_eq!(suggestion.writes.iter().map(ToString::to_string).collect::<Vec<_>>(), ["DELETE from orders"]);
    }

    #[test]
    fn an_answer_names_each_of_its_writes() {
        let mut app = waiting("SELECT 1", "ids");
        let writes =
            vec![Statement::new("DELETE FROM orders"), Statement::new("SELECT 1"), Statement::new("UPDATE customers SET name = 'x'")];
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(writes) });
        let Some(AskStep::Answered(suggestion)) = step(&app) else { panic!("no answer") };
        let named: Vec<String> = suggestion.writes.iter().map(ToString::to_string).collect();
        assert_eq!(named, ["DELETE from orders", "UPDATE customers"]);
    }

    #[test]
    fn several_reads_are_no_write() {
        let mut app = waiting("SELECT 1", "ids");
        let reads = vec![Statement::new("SELECT 1"), Statement::new("SELECT 2")];
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(reads) });
        let Some(AskStep::Answered(suggestion)) = step(&app) else { panic!("no answer") };
        assert_eq!((suggestion.text.as_str(), suggestion.writes.is_empty()), ("SELECT 1;\nSELECT 2", true));
    }

    fn open_in_the_editor(app: &mut App, text: &str) {
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: shop(), query: query("customers.sql"), text: text.into() }));
    }

    #[test]
    fn an_answer_that_comes_after_the_editor_changed_is_dropped_and_says_why() {
        let mut app = waiting("SELECT * FROM orders WHERE total > 10", "only the ids");
        open_in_the_editor(&mut app, "SELECT 1");
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(vec![Statement::new("SELECT id FROM orders")]) });
        assert_eq!((step(&app), app.ask_failure()), (None, Some(&AskFailure::EditorChanged)));
    }

    #[test]
    fn tab_on_an_answer_whose_editor_changed_since_drops_it_and_says_why() {
        let mut app = answered("SELECT * FROM orders WHERE total > 10", "SELECT id FROM orders");
        open_in_the_editor(&mut app, "SELECT 1");
        press(&mut app, KeyCode::Tab);
        assert_eq!((app.sheet().editor.text(), app.ask_failure()), ("SELECT 1", Some(&AskFailure::EditorChanged)));
    }

    #[test]
    fn enter_on_an_instruction_sends_nothing_once_the_connection_says_ai_off() {
        let mut app = asking_on("SELECT 1");
        ctrl_g(&mut app);
        type_in(&mut app, "ids");
        app.sheet_mut().connection = fixtures::connection("tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nai = \"off\"\n");
        assert_eq!(press(&mut app, KeyCode::Enter), []);
        assert_eq!((step(&app), app.ask_failure()), (None, Some(&AskFailure::Off)));
    }

    #[test]
    fn the_failure_clears_on_the_next_key() {
        let mut app = app_on(&connection("local", "read-only"));
        ctrl_g(&mut app);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.ask_failure(), None);
    }
}
