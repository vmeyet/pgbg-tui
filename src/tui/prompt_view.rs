//! The prompt on the footer line: the path of a new query as it is typed, whether to open a query over unsaved text,
//! whether to send a write and how many rows it would touch, whether to delete a query, or the grid's filter.
use super::app::{App, Opening, Prompt};
use super::field_line;
use super::ui::{draw_with_hints, footer_label, muted_hints};
use crate::act::Pane;
use crate::render;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use vtui::field::Field;

/// The prompt over the footer in `area`, followed by its keys.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, prompt: &Prompt, area: Rect) {
    let theme = app.theme();
    let spans = match prompt {
        Prompt::SaveAs { path, error } => {
            let error = error.iter().map(|error| Span::styled(format!("{error} "), Style::default().fg(theme.danger)));
            let after = error.chain(muted_hints(app, prompt.pane()));
            return field_line::draw(f, footer_label(app, " save as › "), path, after, area);
        }
        Prompt::OpenOverUnsaved(opening) => {
            let what = match opening {
                Opening::Saved(query) => format!("open {}", query.path),
                Opening::New => "start a new query".to_owned(),
            };
            vec![Span::styled(format!(" the editor's text is not saved: {what} over it? "), Style::default().fg(theme.warn))]
        }
        Prompt::SendWrite(waiting) => {
            let question = format!(" {}? ", render::printable(&waiting.write.to_string()));
            let rows = waiting.rows.to_string();
            let mut spans = vec![Span::styled(question, Style::default().fg(theme.danger).add_modifier(Modifier::BOLD))];
            spans.extend((!rows.is_empty()).then(|| Span::raw(format!("{} ", render::printable(&rows)))));
            spans
        }
        Prompt::DeleteQuery(query) => {
            vec![Span::styled(format!(" delete {}? ", query.path), Style::default().fg(theme.danger).add_modifier(Modifier::BOLD))]
        }
    };
    draw_with_hints(f, app, spans, prompt.pane(), area);
}

/// The grid's filter over the footer in `area`, as it is typed, followed by its keys.
pub(super) fn draw_row_filter(f: &mut Frame<'_>, app: &App, filter: &Field, area: Rect) {
    field_line::draw(f, footer_label(app, " filter › "), filter, muted_hints(app, Pane::RowFilter), area);
}

#[cfg(test)]
mod tests {
    use super::super::app::ask::tests::type_in;
    use super::super::app::fixtures::{app_on, connection, control, press, pressed, shop, to_shop, typed};
    use super::super::app::queries::tests::{asked_to_delete_daily, query};
    use super::super::app::{Answer, App, Incoming, LookId, QueryAnswer};
    use super::super::ui;
    use crate::db::model::{ColumnKind, ResultSet, Value};
    use crate::fixtures::{column, orders};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;

    fn render(app: &App, width: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, 6)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, app)).unwrap();
        terminal
    }

    fn save_as(path: &str) -> App {
        let mut app = typed("SELECT 1");
        app.handle_key(control('s'));
        type_in(&mut app, path);
        app
    }

    fn cursor_on(app: &App) -> Position {
        let terminal = render(app, 60);
        assert!(terminal.backend().cursor_visible());
        terminal.backend().cursor_position()
    }

    #[test]
    fn the_save_as_cursor_follows_the_arrows_into_the_path() {
        let mut app = save_as("orders/");
        press(&mut app, KeyCode::Left);
        assert_eq!(cursor_on(&app), Position { x: 18, y: 4 });
    }

    #[test]
    fn the_row_filter_cursor_follows_the_arrows_into_the_filter() {
        let mut app = pressed(orders(), "/na");
        press(&mut app, KeyCode::Left);
        assert_eq!(cursor_on(&app), Position { x: 12, y: 4 });
    }

    #[test]
    fn the_refusal_of_a_path_still_shows_after_it() {
        let mut app = save_as("../orders");
        press(&mut app, KeyCode::Enter);
        insta::assert_snapshot!("save_as_refused", render(&app, 120).backend());
    }

    #[test]
    fn the_save_as_prompt_escapes_a_control_character_of_the_open_querys_path() {
        let mut app = app_on(&connection("local", "read-only"));
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: shop(), query: query("da\x1b[2Kily.sql"), text: "SELECT 1".into() }));
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT));
        let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("save_as_escaped", terminal.backend());
    }

    #[test]
    fn the_question_escapes_a_control_character_and_a_reversing_mark_in_a_name() {
        let mut app = app_on(&connection("production", "allowed"));
        let text = "DELETE FROM \"orders\x1b[2K\u{202e}sredro\"".into();
        app.apply(Answer::Queries(QueryAnswer::Opened { connection: shop(), query: query("orders/purge.sql"), text }));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("send_write_escaped", terminal.backend());
    }

    #[test]
    fn the_delete_question_names_the_query_and_its_keys() {
        insta::assert_snapshot!("delete_query", render(&asked_to_delete_daily(), 80).backend());
    }

    #[test]
    fn the_question_says_how_many_rows_the_write_would_touch() {
        let mut app = app_on(&connection("production", "allowed"));
        "DELETE FROM orders".chars().for_each(|c| _ = app.handle_key(KeyEvent::from(KeyCode::Char(c))));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let counted = ResultSet::whole(vec![column("count", ColumnKind::Int)], vec![vec![Value::Int(1_732)]]);
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(counted) }));
        let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("send_write_counted", terminal.backend());
    }
}
