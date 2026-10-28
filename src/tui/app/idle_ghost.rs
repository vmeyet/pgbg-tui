//! The idle ghost: once the editor sat a second without a key, the `[ai] command` asked for the rest of the statement
//! under the cursor, shown faded after it as the completion ghost is, and taken with the same `→`.
//! The `--` lines right above the statement are the instruction; under them, the statement may still be empty.
//! The loop owns the second; the app only says whether one would ask.
use super::editor::Revision;
use super::ghost::Ghost;
use super::{Action, App, AskFailure, AskId, Focus, NoIdleGhost, Sheet};
use crate::ai::{Instruction, LiteralPolicy, Prompt};
use crate::config::AiSettings;
use crate::connection::{AiAsking, ConnectionName};
use crate::db::lex::{self, Placed, Token};
use crate::db::model::Statement;
use crate::query::Header;
use crate::secret;
use std::cmp::Ordering;

const NO_COMMENT: &str = "Complete the statement under the cursor.";
const KEEP_TYPED: &str = "Answer with the statement under the cursor whole, the text it holds kept exactly as it is, followed by \
                          the rest it needs.";

/// What the idle ask is about: the statement typed before the cursor, empty under a comment, and the text of each `--`
/// line right above it, none when there is none.
struct Question<'a> {
    typed: &'a str,
    comment: Vec<&'a str>,
}

impl Question<'_> {
    fn instruction(&self) -> Instruction {
        let asked = if self.comment.is_empty() { NO_COMMENT.to_owned() } else { self.comment.join("\n") };
        Instruction::new(format!("{asked}\n{KEEP_TYPED}"))
    }
}

/// One sheet's editor at one revision: each editor counts its own revisions, so the sheet tells two of them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EditorAt {
    sheet: ConnectionName,
    revision: Revision,
}

/// Where the idle ask stands, so one text is asked about once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum IdleAsk {
    #[default]
    NotAsked,
    /// The command runs on `ask`, asked about `editor`.
    Waiting { ask: AskId, editor: EditorAt },
    /// This editor was asked about, or could not be.
    Asked(EditorAt),
}

impl IdleAsk {
    pub(super) fn is_waiting_on(&self, ask: AskId) -> bool {
        matches!(self, Self::Waiting { ask: waiting, .. } if *waiting == ask)
    }

    fn covers(&self, editor: &EditorAt) -> bool {
        match self {
            Self::NotAsked => false,
            Self::Waiting { .. } => true,
            Self::Asked(asked) => asked == editor,
        }
    }

    /// Marks the ask waited on as asked, and gives its id; nothing when none was waited on.
    fn stop_waiting(&mut self) -> Option<AskId> {
        match std::mem::take(self) {
            Self::Waiting { ask, editor } => {
                *self = Self::Asked(editor);
                Some(ask)
            }
            other => {
                *self = other;
                None
            }
        }
    }
}

impl App {
    /// Whether the editor sitting idle now would ask for the rest of the statement under the cursor: `[ai] ghost` is on,
    /// the editor has the keys with no overlay over it, no completion ghost shows, and this text was not asked about.
    pub(in crate::tui) fn wants_idle_ghost(&self) -> bool {
        self.idle_settings().is_some()
            && self.shows_the_bare_editor()
            && !self.asks.idle.covers(&self.editor_on_screen())
            && self.sheet().question().is_some()
    }

    /// Asks for the rest of the statement under the cursor, once the editor sat idle; nothing when it would not ask.
    pub(in crate::tui) fn ask_idle_ghost(&mut self) -> Vec<Action> {
        let Some(settings) = self.idle_settings().filter(|_| self.wants_idle_ghost()) else { return vec![] };
        let settings = settings.without_passwords_of(&self.connections);
        let editor = self.editor_on_screen();
        let Some(prompt) = self.sheet().idle_prompt(settings.literals) else {
            self.asks.idle = IdleAsk::Asked(editor);
            return vec![];
        };
        let ask = self.asks.take_id();
        self.asks.idle = IdleAsk::Waiting { ask, editor };
        vec![Action::Ask { ask, prompt, settings }]
    }

    /// Kills the idle ask still running, since a key or a paste makes its answer stale.
    pub(super) fn drop_idle_ask(&mut self) -> Option<Action> {
        self.asks.idle.stop_waiting().map(Action::DropAsk)
    }

    /// Shows the rest of the statement the idle ask's `answer` gives, while the editor is still the one it asked about;
    /// when it gives none, says why for the log, since nobody asked for it and so nothing on screen says it.
    pub(super) fn show_idle_ghost(&mut self, answer: Result<Vec<Statement>, AskFailure>) -> Option<Action> {
        self.asks.idle.stop_waiting()?;
        let is_current = self.asks.idle.covers(&self.editor_on_screen()) && self.shows_the_bare_editor() && self.idle_settings().is_some();
        if !is_current {
            return None;
        }
        let why = match answer.map(|statements| self.sheet().rest_in(&statements)) {
            Ok(Some(ghost)) => {
                self.sheet_mut().ghost = Some(ghost);
                return None;
            }
            Ok(None) => NoIdleGhost::NotContinued,
            Err(failure) => NoIdleGhost::Failed(failure),
        };
        Some(Action::LogNoIdleGhost(why))
    }

    fn editor_on_screen(&self) -> EditorAt {
        EditorAt { sheet: self.sheet().name().clone(), revision: self.sheet().editor.revision() }
    }

    fn idle_settings(&self) -> Option<&AiSettings> {
        let settings = self.asks.settings().filter(|settings| settings.ghost)?;
        (self.sheet().connection.ai() != AiAsking::Off).then_some(settings)
    }

    fn shows_the_bare_editor(&self) -> bool {
        self.overlay.is_none() && self.focus == Focus::Editor && self.sheet().ghost.is_none()
    }
}

impl Sheet {
    /// What the idle ask is about: the statement under the cursor when the cursor ends it, or else an empty one when
    /// the cursor sits on a blank line right under a `--` line; none when it would not ask.
    fn question(&self) -> Option<Question<'_>> {
        let text = self.editor.text();
        let tokens = self.editor.lexed().tokens();
        let (before, _, _) = self.editor.split();
        let cursor = before.len();
        let Some(range) = self.range_under_cursor() else { return empty_question(text, tokens, cursor) };
        let typed = || {
            let first = self.editor.lexed().tokens_in(range.start..cursor).first().map_or(cursor, |placed| placed.range.start);
            Question { typed: &text[range.start..cursor], comment: comment_above(text, tokens, first) }
        };
        match cursor.cmp(&range.end) {
            Ordering::Less => (range.start <= cursor && is_blank(&text[cursor..range.end])).then(typed),
            _ if is_blank(&text[range.end..cursor]) => Some(typed()),
            _ => empty_question(text, tokens, cursor),
        }
    }

    /// The prompt asking for the rest of the statement under the cursor; none when nothing is asked or it cannot be
    /// sent, as when its comment holds a secret: the comment leaves the machine as typed.
    fn idle_prompt(&self, literals: LiteralPolicy) -> Option<Prompt> {
        let question = self.question()?;
        if question.comment.iter().any(|line| secret::holds_secret(line)) {
            return None;
        }
        self.prompt_about(&question.instruction(), Some(&Statement::new(question.typed)), literals).ok()
    }

    /// The ghost of what the first of `statements` adds after the statement typed before the cursor, when it starts
    /// with that statement's tokens.
    fn rest_in(&self, statements: &[Statement]) -> Option<Ghost> {
        let question = self.question()?;
        let rest = rest_after(question.typed, statements.first()?.as_str())?;
        let (before, _, _) = self.editor.split();
        Ghost::at_cursor(before.len(), &rest)
    }
}

/// An empty statement at byte `cursor` of `text`, when its line is blank and `--` lines sit right above it.
fn empty_question<'a>(text: &'a str, tokens: &[Placed], cursor: usize) -> Option<Question<'a>> {
    let rest_of_line = text[cursor..].split('\n').next().unwrap_or_default();
    let comment = comment_above(text, tokens, cursor);
    (is_blank(rest_of_line) && !comment.is_empty()).then_some(Question { typed: "", comment })
}

/// The text of each `--` line right above the line holding byte `at` of `text`, top first, below the last of `tokens`
/// before it and without the `key:` lines of the file's header; none when that line holds more than blanks before `at`.
fn comment_above<'a>(text: &'a str, tokens: &[Placed], at: usize) -> Vec<&'a str> {
    let line_start = text[..at].rfind('\n').map_or(0, |newline| newline + 1);
    if !is_blank(&text[line_start..at]) {
        return vec![];
    }
    let token_end = tokens[..tokens.partition_point(|placed| placed.range.end <= line_start)].last().map(|placed| placed.range.end);
    let floor = token_end.map_or(0, |end| text[end..line_start].find('\n').map_or(line_start, |newline| end + newline + 1));
    let above = &text[floor..line_start];
    let mut comments: Vec<&str> = above.lines().rev().map_while(Header::comment_in).collect();
    let is_header = floor == 0 && comments.len() == above.lines().count();
    comments.reverse();
    let is_header_key = |comment: &&str| is_header && Header::is_key_line(comment);
    comments.into_iter().map(str::trim).filter(|comment| !comment.is_empty() && !is_header_key(comment)).collect()
}

fn is_blank(text: &str) -> bool {
    text.trim_matches(lex::is_blank).is_empty()
}

/// What `answer` adds after `typed`, when its tokens start with those of `typed`: blanks, comments and the case of words
/// aside, so a reformatted answer still fits, and the last typed token may be the start of a longer one.
/// A rest after a typed token begins with one space where the answer has blanks; after typed blanks, with none.
/// When nothing is typed, the rest is the whole answer from its first token.
fn rest_after(typed: &str, answer: &str) -> Option<String> {
    let (typed_tokens, answer_tokens) = (lex::tokens(typed), lex::tokens(answer));
    let Some((last, before)) = typed_tokens.split_last() else {
        return answer_tokens.first().map(|first| answer[first.range.start..].to_owned());
    };
    let reached = answer_tokens.get(before.len())?;
    let same_before = before.iter().zip(&answer_tokens).all(|(mine, theirs)| is_same(mine, typed, theirs, answer));
    let ends_on_token = last.range.end == typed.len();
    let last_typed = &typed[last.range.clone()];
    let reached_text = &answer[reached.range.clone()];
    let fits = if ends_on_token { starts_with(reached_text, last_typed, &last.token) } else { is_same(last, typed, reached, answer) };
    if !same_before || !fits {
        return None;
    }
    let after = &answer[reached.range.start + last_typed.len()..];
    let trimmed = after.trim_start_matches(lex::is_blank);
    let gap = if ends_on_token && trimmed.len() < after.len() { " " } else { "" };
    Some(format!("{gap}{trimmed}"))
}

fn is_same(mine: &Placed, typed: &str, theirs: &Placed, answer: &str) -> bool {
    let text = &answer[theirs.range.clone()];
    text.len() == mine.range.len() && starts_with(text, &typed[mine.range.clone()], &mine.token)
}

/// Whether the token `text` starts with `typed`, a token of kind `kind`; PostgreSQL reads a word without regard to ASCII case.
fn starts_with(text: &str, typed: &str, kind: &Token) -> bool {
    match text.get(..typed.len()) {
        Some(start) if *kind == Token::Word => start.eq_ignore_ascii_case(typed),
        Some(start) => start == typed,
        None => false,
    }
}

#[cfg(test)]
pub(in crate::tui) mod tests {
    use super::super::ask::tests::ai_settings;
    use super::super::fixtures::{app_on, connection, to_shop};
    use super::super::picker::tests::{ctrl, on_local, press};
    use super::super::{Answer, Incoming, Overlay};
    use super::*;
    use crate::ai::CommandError;
    use crate::complete::Index;
    use crate::fixtures::{self, shop_catalog};
    use crate::tui::app::AskStep;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::Arc;
    use std::time::Duration;

    fn type_in(app: &mut App, text: &str) {
        let key = |c| if c == '\n' { KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT) } else { KeyEvent::from(KeyCode::Char(c)) };
        text.chars().for_each(|c| drop(app.handle_key(key(c))));
    }

    fn prompt_text(app: &mut App) -> String {
        let sent = app.ask_idle_ghost();
        let [Action::Ask { prompt, .. }] = &*sent else { panic!("{sent:?}") };
        prompt.text().to_owned()
    }

    fn with_ghost_on(connection: &crate::connection::Connection, text: &str) -> App {
        let mut app = app_on(connection).with_ai(Some(AiSettings { ghost: true, ..ai_settings() }));
        let catalog = Arc::new(shop_catalog());
        let index = Arc::new(Index::new(&catalog));
        app.apply(to_shop(Incoming::Opened));
        app.apply(to_shop(Incoming::Schema(Ok(Arc::clone(&catalog)))));
        app.apply(to_shop(Incoming::Index { catalog, index }));
        type_in(&mut app, text);
        app
    }

    /// The app with `[ai] ghost = true` on `shop`, its schema read, `text` in the editor.
    pub(in crate::tui) fn idle_on(text: &str) -> App {
        with_ghost_on(&connection("local", "read-only"), text)
    }

    /// The app showing the idle ghost `answer` gave for `text`.
    pub(in crate::tui) fn ghosted(text: &str, answer: &str) -> App {
        let mut app = idle_on(text);
        app.ask_idle_ghost();
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(vec![Statement::new(answer)]) });
        app
    }

    fn rest(app: &App) -> Option<&str> {
        app.sheet().ghost.as_ref().map(|ghost| ghost.rest.as_str())
    }

    #[test]
    fn a_second_idle_asks_for_the_rest_of_the_statement_under_the_cursor() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        let sent = app.ask_idle_ghost();
        let [Action::Ask { ask: AskId::FIRST, prompt, .. }] = &*sent else { panic!("{sent:?}") };
        assert!(prompt.text().contains("SELECT * FROM orders WHERE"), "{}", prompt.text());
    }

    #[test]
    fn the_answer_shows_the_rest_after_the_cursor() {
        let app = ghosted("SELECT * FROM orders WHERE ", "SELECT * FROM orders WHERE total > 10");
        assert_eq!(rest(&app), Some("total > 10"));
    }

    #[test]
    fn the_whole_rest_shows_across_its_lines() {
        let app = ghosted("SELECT * ", "SELECT * FROM orders\nWHERE total > 10");
        assert_eq!(rest(&app), Some("FROM orders\nWHERE total > 10"));
    }

    #[test]
    fn tab_takes_the_idle_ghost() {
        let mut app = ghosted("SELECT * ", "SELECT * FROM orders\nWHERE total > 10");
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.sheet().editor.split(), ("SELECT * FROM orders\nWHERE total > 10", "", ""));
    }

    #[test]
    fn right_takes_every_line_of_the_rest() {
        let mut app = ghosted("SELECT * ", "SELECT * FROM orders\nWHERE total > 10");
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.sheet().editor.split(), ("SELECT * FROM orders\nWHERE total > 10", "", ""));
    }

    #[test]
    fn without_a_comment_above_the_instruction_is_to_complete_the_statement() {
        assert!(prompt_text(&mut idle_on("SELECT * FROM orders WHERE ")).contains(&format!("Instruction:\n{NO_COMMENT}\n")));
    }

    #[test]
    fn the_comment_lines_right_above_the_statement_are_the_instruction() {
        let text = prompt_text(&mut idle_on("-- the big ones\n-- paid only\nSELECT * FROM orders WHERE "));
        assert!(text.contains("Instruction:\nthe big ones\npaid only\n"), "{text}");
    }

    #[test]
    fn a_header_line_above_the_statement_is_no_instruction() {
        let text = prompt_text(&mut idle_on("-- name: big orders\nSELECT * FROM orders WHERE "));
        assert!(text.contains(&format!("Instruction:\n{NO_COMMENT}\n")), "{text}");
    }

    #[test]
    fn a_blank_line_under_a_comment_asks_for_a_whole_statement() {
        let text = prompt_text(&mut idle_on("SELECT 1;\n-- the ten biggest orders\n"));
        assert!(text.contains("Instruction:\nthe ten biggest orders\n") && text.contains("cursor:\n(none)\n"), "{text}");
    }

    #[test]
    fn under_a_comment_the_whole_answer_shows() {
        let app = ghosted("-- the ten biggest orders\n", "-- biggest\nSELECT * FROM orders\nORDER BY total DESC LIMIT 10");
        assert_eq!(rest(&app), Some("SELECT * FROM orders\nORDER BY total DESC LIMIT 10"));
    }

    #[test]
    fn a_blank_line_under_no_comment_asks_nothing() {
        assert!(!idle_on("SELECT 1;\n\n").wants_idle_ghost());
    }

    #[test]
    fn a_blank_line_inside_a_statement_under_a_comment_asks_nothing() {
        let mut app = idle_on("-- the big ones\n\nSELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Up));
        assert!(!app.wants_idle_ghost());
    }

    #[test]
    fn a_comment_holding_a_secret_is_never_sent() {
        let mut app = idle_on("-- set the password of nina\n");
        assert_eq!(app.ask_idle_ghost(), []);
    }

    #[test]
    fn an_answer_laid_out_on_other_lines_still_shows_the_rest() {
        let app = ghosted("SELECT * FROM orders o JOIN ", "SELECT *\nFROM orders o\nJOIN customers c ON c.id = o.customer_id");
        assert_eq!(rest(&app), Some("customers c ON c.id = o.customer_id"));
    }

    #[test]
    fn an_answer_that_changes_what_is_typed_shows_nothing() {
        assert_eq!(rest(&ghosted("SELECT * FROM orders WHERE ", "SELECT id FROM orders WHERE total > 10")), None);
    }

    #[test]
    fn the_case_of_words_and_comments_do_not_keep_the_answer_from_fitting() {
        assert_eq!(
            rest_after("select * from orders ", "-- the open ones\nSELECT * FROM orders WHERE total > 10").as_deref(),
            Some("WHERE total > 10")
        );
    }

    #[test]
    fn a_literal_changed_by_the_answer_keeps_it_from_fitting() {
        assert_eq!(
            rest_after("SELECT * FROM orders WHERE note = 'Paid' ", "SELECT * FROM orders WHERE note = 'paid' AND total > 10"),
            None
        );
    }

    #[test]
    fn the_last_typed_word_may_be_the_start_of_a_longer_one() {
        assert_eq!(rest_after("SELECT * FROM ord", "SELECT * FROM orders").as_deref(), Some("ers"));
    }

    #[test]
    fn the_last_typed_word_followed_by_a_blank_must_be_whole() {
        assert_eq!(rest_after("SELECT * FROM ord ", "SELECT * FROM orders"), None);
    }

    #[test]
    fn a_rest_on_the_next_line_after_a_typed_word_shows_after_one_space() {
        assert_eq!(rest_after("SELECT *", "SELECT *\nFROM orders").as_deref(), Some(" FROM orders"));
    }

    #[test]
    fn with_nothing_typed_the_rest_is_the_answer_from_its_first_token() {
        assert_eq!(rest_after("", "-- by total\nSELECT * FROM orders").as_deref(), Some("SELECT * FROM orders"));
    }

    fn comment_at_end(text: &str) -> Vec<&str> {
        comment_above(text, &lex::tokens(text), text.len())
    }

    #[test]
    fn a_comment_above_ends_at_the_first_line_that_is_no_comment() {
        assert_eq!(comment_at_end("-- all\nSELECT 1;\n-- the open\n  -- orders\n"), ["the open", "orders"]);
    }

    #[test]
    fn a_line_with_text_before_the_point_has_no_comment_above() {
        assert_eq!(comment_at_end("-- the open orders\nSELECT "), Vec::<&str>::new());
    }

    #[test]
    fn a_comment_line_inside_a_string_is_no_comment_above() {
        assert_eq!(comment_at_end("SELECT 'a\n-- the open orders\n'\n"), Vec::<&str>::new());
    }

    #[test]
    fn a_key_line_out_of_the_file_header_is_a_comment_above() {
        assert_eq!(comment_at_end("SELECT 1;\n-- note: paid only\n"), ["note: paid only"]);
    }

    #[test]
    fn an_answer_shorter_than_what_is_typed_does_not_fit() {
        assert_eq!(rest_after("SELECT * FROM orders WHERE ", "SELECT * FROM orders"), None);
    }

    #[test]
    fn a_failed_idle_ask_is_logged() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.ask_idle_ghost();
        let timed_out = || AskFailure::Command(CommandError::TimedOut(Duration::from_secs(30)));
        let logged = app.apply(Answer::Asked { ask: AskId::FIRST, answer: Err(timed_out()) });
        assert_eq!(logged, [Action::LogNoIdleGhost(NoIdleGhost::Failed(timed_out()))]);
    }

    #[test]
    fn an_answer_that_does_not_fit_is_logged() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.ask_idle_ghost();
        let logged = app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(vec![Statement::new("SELECT id FROM customers")]) });
        assert_eq!(logged, [Action::LogNoIdleGhost(NoIdleGhost::NotContinued)]);
    }

    #[test]
    fn right_takes_the_idle_ghost() {
        let mut app = ghosted("SELECT * FROM orders WHERE ", "SELECT * FROM orders WHERE total > 10");
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.sheet().editor.split(), ("SELECT * FROM orders WHERE total > 10", "", ""));
    }

    #[test]
    fn a_key_kills_the_idle_ask() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.ask_idle_ghost();
        assert_eq!(app.handle_key(KeyEvent::from(KeyCode::Left)), [Action::DropAsk(AskId::FIRST)]);
    }

    #[test]
    fn a_paste_kills_the_idle_ask() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.ask_idle_ghost();
        assert_eq!(app.paste("id"), [Action::DropAsk(AskId::FIRST)]);
    }

    #[test]
    fn an_answer_after_a_key_shows_nothing() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.ask_idle_ghost();
        app.handle_key(KeyEvent::from(KeyCode::Backspace));
        app.apply(Answer::Asked { ask: AskId::FIRST, answer: Ok(vec![Statement::new("SELECT * FROM orders WHERE total > 10")]) });
        assert_eq!(rest(&app), None);
    }

    #[test]
    fn a_key_drops_the_idle_ghost() {
        let mut app = ghosted("SELECT * FROM orders WHERE ", "SELECT * FROM orders WHERE total > 10");
        app.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(rest(&app), None);
    }

    #[test]
    fn the_completion_ghost_wins_and_nothing_is_asked() {
        let mut app = idle_on("SELECT * FROM cu");
        assert_eq!((app.wants_idle_ghost(), app.ask_idle_ghost()), (false, vec![]));
    }

    #[test]
    fn the_idle_ghost_is_off_by_default() {
        let mut app = app_on(&connection("local", "read-only")).with_ai(Some(ai_settings()));
        type_in(&mut app, "SELECT * FROM orders WHERE ");
        assert_eq!((app.wants_idle_ghost(), app.ask_idle_ghost()), (false, vec![]));
    }

    #[test]
    fn a_connection_with_ai_off_never_asks() {
        let off = fixtures::connection("tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nai = \"off\"\n");
        let mut app = with_ghost_on(&off, "SELECT * FROM orders WHERE ");
        assert_eq!((app.wants_idle_ghost(), app.ask_idle_ghost()), (false, vec![]));
    }

    #[test]
    fn the_same_text_is_asked_about_once() {
        let mut app = ghosted("SELECT * FROM orders WHERE ", "SELECT * FROM orders WHERE total > 10");
        app.handle_key(KeyEvent::from(KeyCode::Left));
        app.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.ask_idle_ghost(), []);
    }

    #[test]
    fn another_sheet_at_the_same_revision_still_asks() {
        let mut app = on_local().with_ai(Some(AiSettings { ghost: true, ..ai_settings() }));
        type_in(&mut app, "SELECT * FROM orders WHERE ");
        let [Action::Ask { ask, .. }] = &*app.ask_idle_ghost() else { panic!("no ask") };
        app.apply(Answer::Asked { ask: *ask, answer: Ok(vec![]) });
        ctrl(&mut app, 'o');
        press(&mut app, "staging");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        type_in(&mut app, "SELECT * FROM orders WHERE ");
        assert!(app.wants_idle_ghost());
    }

    #[test]
    fn nothing_is_asked_while_ctrl_g_is_open() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(matches!(app.overlay(), Some(Overlay::Ask(AskStep::Instruction(_)))));
        assert_eq!(app.ask_idle_ghost(), []);
    }

    #[test]
    fn nothing_is_asked_in_the_middle_of_a_statement() {
        let mut app = idle_on("SELECT * FROM orders WHERE ");
        app.handle_key(KeyEvent::from(KeyCode::Home));
        assert_eq!(app.ask_idle_ghost(), []);
    }

    #[test]
    fn a_statement_that_cannot_be_sent_is_not_asked_again() {
        let mut app = idle_on("SELECT * FROM orders WHERE note = 'pa");
        assert_eq!(app.ask_idle_ghost(), []);
        assert!(!app.wants_idle_ghost());
    }
}
