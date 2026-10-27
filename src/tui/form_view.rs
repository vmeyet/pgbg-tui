//! The connection form over the screen: one line per key, the one with the cursor marked, each refusal under its line.
use super::app::{App, Form, Overlay, PasswordFrom, Saving, Slot};
use super::theme;
use crate::connection::{Tls, WritePolicy};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use vtui::field::Field;
use vtui::pane;

const LABEL_WIDTH: usize = 16;
/// The mark before each line.
const MARK_WIDTH: usize = 1;
const FOOTER: &str = "⇥ next · ←→ choose · ⏎ saves · esc";

/// The form when it is open, with the terminal's cursor on the text line being typed.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(Overlay::Form(form)) = &app.overlay else { return };
    let width = form_width(f.area());
    let block = pane::framed(&app.theme, &title(form), true);
    let room = usize::from(block.inner(Rect::new(0, 0, width, 0)).width).saturating_sub(MARK_WIDTH + LABEL_WIDTH);
    let mut lines: Vec<Line<'static>> = form.slots().into_iter().flat_map(|slot| slot_lines(app, form, slot, room)).collect();
    lines.push(Line::default());
    lines.extend(form.is_saving().then(|| Line::styled("saving…", Style::default().fg(app.theme.muted))));
    lines.extend(form.failure.as_ref().map(|why| Line::styled(format!("✗ {why}"), Style::default().fg(theme::failure(&app.theme)))));
    lines.push(Line::styled(FOOTER, Style::default().fg(app.theme.faded)));
    let area = centred(f.area(), width, lines.len() as u16 + 2);
    let inside = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines.clone()).block(block).style(Style::default().bg(app.theme.surface)), area);
    if let Some(at) = cursor(form, &lines, inside, room) {
        f.set_cursor_position(at);
    }
}

fn title(form: &Form) -> String {
    match form.saving {
        Saving::Add => "Add a connection".to_owned(),
        Saving::Edit => format!("Edit {}", form.name.text()),
    }
}

fn form_width(area: Rect) -> u16 {
    (area.width * 3 / 5).clamp(40.min(area.width), area.width)
}

fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let height = height.min(area.height);
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 3, width, height }
}

/// The line of `slot`, its text in `room` columns, then why it was refused when it was.
fn slot_lines(app: &App, form: &Form, slot: Slot, room: usize) -> Vec<Line<'static>> {
    let is_here = form.at == slot;
    let mark = Span::styled(if is_here { "▎" } else { " " }, Style::default().fg(app.theme.accent));
    let label_style = if is_here { Style::default().add_modifier(Modifier::BOLD) } else { Style::default().fg(app.theme.muted) };
    let label = Span::styled(format!("{:<LABEL_WIDTH$}", label(form, slot)), label_style);
    let line = Line::from(vec![mark, label, Span::raw(value(form, slot, room))]);
    let refusal = form.error(slot).map(|why| Line::styled(format!("  ✗ {why}"), Style::default().fg(theme::failure(&app.theme))));
    std::iter::once(line).chain(refusal).collect()
}

fn label(form: &Form, slot: Slot) -> &'static str {
    match (slot, form.password_from) {
        (Slot::Name, _) => "name",
        (Slot::Tier, _) => "tier",
        (Slot::Url, _) => "url",
        (Slot::PasswordFrom, _) => "password from",
        (Slot::PasswordWhere, PasswordFrom::Env) => "variable",
        (Slot::PasswordWhere, PasswordFrom::Command) => "command",
        (Slot::PasswordWhere, _) => "keychain item",
        (Slot::Password, _) => "password",
        (Slot::Tunnel, _) => "tunnel",
        (Slot::Writes, _) => "writes",
        (Slot::Tls, _) => "tls",
        (Slot::StatementTimeout, _) => "timeout",
    }
}

fn value(form: &Form, slot: Slot, room: usize) -> String {
    match slot {
        Slot::Name => shown(form, slot, &form.name, room),
        Slot::Tier => format!("‹ {} ›", form.tier),
        Slot::Url => shown(form, slot, &form.url, room),
        Slot::PasswordFrom => format!("‹ {} ›", password_from(form.password_from)),
        Slot::PasswordWhere => shown(form, slot, &form.password_where, room),
        Slot::Password => "•".repeat(form.password.typed_count()),
        Slot::Tunnel => shown(form, slot, &form.tunnel, room),
        Slot::Writes => format!("‹ {} ›", if form.writes == WritePolicy::Allowed { "allowed" } else { "read only" }),
        Slot::Tls => format!("‹ {} ›", tls(form.tls)),
        Slot::StatementTimeout if form.statement_timeout.text().is_empty() && form.at != slot => "the tier's".to_owned(),
        Slot::StatementTimeout => shown(form, slot, &form.statement_timeout, room),
    }
}

/// The text of `field`, from where it scrolled to on the line with the cursor.
fn shown(form: &Form, slot: Slot, field: &Field, room: usize) -> String {
    let start = if form.at == slot { scrolled(field, room) } else { 0 };
    field.text()[start..].to_owned()
}

/// Where the text in view starts, so that the cursor stays within `room` columns.
fn scrolled(field: &Field, room: usize) -> usize {
    let (before, _, _) = field.split();
    let fits = |at: &usize| Line::from(&before[*at..]).width() < room;
    before.char_indices().map(|(at, _)| at).find(fits).unwrap_or(before.len())
}

fn password_from(from: PasswordFrom) -> &'static str {
    match from {
        PasswordFrom::Nowhere => "none",
        PasswordFrom::Keychain => "keychain",
        PasswordFrom::Env => "environment",
        PasswordFrom::Command => "command",
    }
}

fn tls(tls: Option<Tls>) -> &'static str {
    match tls {
        None => "as the url's sslmode says",
        Some(Tls::On) => "on",
        Some(Tls::Off) => "off: in clear, on purpose",
    }
}

/// Where the terminal's cursor goes: after the text before it in view, on a text line that has the keys.
fn cursor(form: &Form, lines: &[Line<'_>], inside: Rect, room: usize) -> Option<Position> {
    let field = form.field()?;
    let row =
        form.slots().iter().take_while(|&&slot| slot != form.at).map(|&slot| 1 + usize::from(form.error(slot).is_some())).sum::<usize>();
    let (before, _, _) = field.split();
    let column = MARK_WIDTH + LABEL_WIDTH + Line::from(&before[scrolled(field, room)..]).width();
    let is_shown = row < lines.len();
    is_shown.then(|| Position { x: (inside.x + column as u16).min(inside.right().saturating_sub(1)), y: inside.y + row as u16 })
}

#[cfg(test)]
mod tests {
    use super::super::app::picker::tests::{ctrl, on_local, press};
    use super::super::ui;
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn the_form_shows_each_key_and_why_a_line_is_refused() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'a');
        press(&mut app, "my shop");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("form", terminal.backend());
    }

    #[test]
    fn a_line_longer_than_the_form_scrolls_so_the_cursor_stays_in_view() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'a');
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        press(&mut app, "postgres://nina@db.example.com/shop?sslmode=require&application_name=pgbg&connect_timeout=5");
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("form_scrolled", terminal.backend());
    }
}
