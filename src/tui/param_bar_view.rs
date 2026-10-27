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

/// Where the field with the keys stands in the bar's line, in columns: its name, the cursor, and past its value.
struct Focused {
    start: usize,
    cursor: usize,
    end: usize,
}

impl Focused {
    /// The columns the line scrolls by in `width`, so the field shows from its name when it fits, and the cursor always.
    fn scroll(&self, width: usize) -> usize {
        let shows_field = self.end.saturating_sub(width).min(self.start);
        let shows_cursor = (self.cursor + 1).saturating_sub(width);
        shows_field.max(shows_cursor)
    }
}

/// The bar in `area`: a name, then its value, else dimmed what it falls back to; scrolled sideways so the field with
/// the keys shows, with the terminal's cursor in it.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) {
    let sheet = app.sheet();
    let focused_name = if app.focus() == Focus::Params { sheet.param_at() } else { None };
    let mut spans = vec![];
    let mut focused = None;
    for name in sheet.param_names() {
        let is_focused = focused_name.as_ref() == Some(&name);
        let start = Line::from(spans.clone()).width();
        spans.push(Span::styled(format!(" {name} "), Style::default().fg(app.theme().muted)));
        let field = sheet.bar.field(&name).filter(|field| !field.text().is_empty());
        let cursor = Line::from(spans.clone()).width() + field.map_or(0, before_cursor_width);
        spans.push(match field {
            Some(field) => Span::styled(render::printable(field.text()).into_owned(), value_style(app, is_focused)),
            None => Span::styled(fallback_text(app, &name), Style::default().fg(app.theme().faded).add_modifier(Modifier::ITALIC)),
        });
        spans.push(Span::raw(" "));
        if is_focused {
            focused = Some(Focused { start, cursor, end: Line::from(spans.clone()).width() });
        }
    }
    let scroll = focused.as_ref().map_or(0, |focused| focused.scroll(usize::from(area.width)));
    f.render_widget(Paragraph::new(Line::from(spans)).scroll((0, to_cells(scroll))), area);
    if let Some(focused) = focused {
        let x = area.x.saturating_add(to_cells(focused.cursor - scroll)).min(area.right().saturating_sub(1));
        f.set_cursor_position(Position { x, y: area.y });
    }
}

fn to_cells(columns: usize) -> u16 {
    u16::try_from(columns).unwrap_or(u16::MAX)
}

fn value_style(app: &App, is_focused: bool) -> Style {
    let style = Style::default().add_modifier(Modifier::UNDERLINED);
    if is_focused { style.fg(app.theme().accent) } else { style }
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
    use ratatui::backend::{Backend as _, TestBackend};
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

    #[test]
    fn the_bar_scrolls_sideways_so_the_field_with_the_keys_shows() {
        let mut app = typed(STATEMENT);
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        "nina, who placed the orders of last week".chars().for_each(|c| drop(app.handle_key(KeyEvent::from(KeyCode::Char(c)))));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        "late".chars().for_each(|c| drop(app.handle_key(KeyEvent::from(KeyCode::Char(c)))));
        let mut terminal = render(&app, 60, 12);
        assert_eq!(terminal.backend_mut().get_cursor_position().unwrap(), Position { x: 58, y: 1 });
        insta::assert_snapshot!("param_bar_scrolled", terminal.backend());
    }
}
