//! The parameter bar above the editor: each `{{name}}` of the statement under the cursor and the value it runs with.
use super::app::{App, Focus};
use crate::query::params::{Fallback, ParamName};
use crate::render;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use vtui::field::Field;

const REQUIRED: &str = "required";
const OPTIONAL: &str = "NULL";

/// The bar in `area`: a name, then its value, else dimmed what it falls back to; the terminal's cursor in the field
/// with the keys.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) {
    let sheet = app.sheet();
    let focused = if app.focus == Focus::Params { sheet.param_at() } else { None };
    let mut spans = vec![];
    let mut cursor = None;
    for name in sheet.param_names() {
        let is_focused = focused.as_ref() == Some(&name);
        spans.push(Span::styled(format!(" {name} "), Style::default().fg(app.theme.muted)));
        let field = sheet.bar.field(&name).filter(|field| !field.text().is_empty());
        if is_focused {
            cursor = Some(Line::from(spans.clone()).width() + field.map_or(0, before_cursor_width));
        }
        spans.push(match field {
            Some(field) => Span::styled(render::printable(field.text()).into_owned(), value_style(app, is_focused)),
            None => Span::styled(fallback_text(app, &name), Style::default().fg(app.theme.faded).add_modifier(Modifier::ITALIC)),
        });
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    if let Some(column) = cursor {
        let x = area.x.saturating_add(u16::try_from(column).unwrap_or(u16::MAX)).min(area.right().saturating_sub(1));
        f.set_cursor_position(Position { x, y: area.y });
    }
}

fn value_style(app: &App, is_focused: bool) -> Style {
    let style = Style::default().add_modifier(Modifier::UNDERLINED);
    if is_focused { style.fg(app.theme.accent) } else { style }
}

fn before_cursor_width(field: &Field) -> usize {
    let (before, _, _) = field.split();
    Line::from(render::printable(before).into_owned()).width()
}

/// What an empty field runs with, as its header says: the default, NULL, or nothing.
fn fallback_text(app: &App, name: &ParamName) -> String {
    let spec = app.sheet().header.params.iter().rev().find(|spec| spec.name == *name);
    match spec.map(|spec| &spec.fallback) {
        Some(Fallback::Default(default)) => render::printable(default).into_owned(),
        Some(Fallback::Optional) => OPTIONAL.to_owned(),
        Some(Fallback::Required) | None => REQUIRED.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::app::tests::typed;
    use super::super::styled;
    use super::super::ui;
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    const STATEMENT: &str = "-- param: since date = 2024-01-01\n-- param: note text optional\nSELECT * FROM orders\n\
                             WHERE day > {{since}} AND owner = {{owner}} AND note = {{note}}";

    fn render(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, app)).unwrap();
        terminal
    }

    #[test]
    fn the_bar_shows_each_parameter_above_the_editor_with_what_an_empty_one_runs_with() {
        insta::assert_snapshot!("param_bar", render(&typed(STATEMENT), 80, 12).backend());
    }

    #[test]
    fn the_field_with_the_keys_shows_its_value_lit() {
        let mut app = typed(STATEMENT);
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        "nina".chars().for_each(|c| drop(app.handle_key(KeyEvent::from(KeyCode::Char(c)))));
        let terminal = render(&app, 80, 12);
        let screen = terminal.backend().buffer();
        let mut bar = Buffer::empty(Rect::new(0, 0, 80, 1));
        (0..80).for_each(|x| bar[(x, 0)] = screen[(x, 1)].clone());
        insta::assert_snapshot!("param_bar_focused", styled::runs(&bar));
    }
}
