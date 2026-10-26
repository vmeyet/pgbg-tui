//! The connection form over the screen: one line per key, the one with the cursor marked, each refusal under its line.
use super::app::{App, Form, Overlay, PasswordFrom, Saving, Slot};
use super::theme;
use crate::connection::WritePolicy;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use vtui::pane;

const LABEL_WIDTH: usize = 16;
const FOOTER: &str = "⇥ next · ←→ choose · ⏎ saves · esc";

/// The form when it is open, with the terminal's cursor on the text line being typed.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(Overlay::Form(form)) = &app.overlay else { return };
    let mut lines: Vec<Line<'static>> = form.slots().into_iter().flat_map(|slot| slot_lines(app, form, slot)).collect();
    lines.push(Line::default());
    lines.extend(form.is_saving.then(|| Line::styled("saving…", Style::default().fg(app.theme.muted))));
    lines.extend(form.failure.as_ref().map(|why| Line::styled(format!("✗ {why}"), Style::default().fg(theme::failure(&app.theme)))));
    lines.push(Line::styled(FOOTER, Style::default().fg(app.theme.faded)));
    let area = centred(f.area(), lines.len() as u16 + 2);
    let block = pane::framed(&app.theme, &title(form), true);
    let inside = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines.clone()).block(block).style(Style::default().bg(app.theme.surface)), area);
    if let Some(at) = cursor(form, &lines, inside) {
        f.set_cursor_position(at);
    }
}

fn title(form: &Form) -> String {
    match form.saving {
        Saving::Add => "Add a connection".to_owned(),
        Saving::Edit => format!("Edit {}", form.name.text()),
    }
}

fn centred(area: Rect, height: u16) -> Rect {
    let width = (area.width * 3 / 5).clamp(40.min(area.width), area.width);
    let height = height.min(area.height);
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 3, width, height }
}

/// The line of `slot`, then why it was refused when it was.
fn slot_lines(app: &App, form: &Form, slot: Slot) -> Vec<Line<'static>> {
    let is_here = form.at == slot;
    let mark = Span::styled(if is_here { "▎" } else { " " }, Style::default().fg(app.theme.accent));
    let label_style = if is_here { Style::default().add_modifier(Modifier::BOLD) } else { Style::default().fg(app.theme.muted) };
    let label = Span::styled(format!("{:<LABEL_WIDTH$}", label(form, slot)), label_style);
    let line = Line::from(vec![mark, label, Span::raw(value(form, slot))]);
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
    }
}

fn value(form: &Form, slot: Slot) -> String {
    match slot {
        Slot::Name => form.name.text().to_owned(),
        Slot::Tier => format!("‹ {} ›", form.tier),
        Slot::Url => form.url.text().to_owned(),
        Slot::PasswordFrom => format!("‹ {} ›", password_from(form.password_from)),
        Slot::PasswordWhere => form.password_where.text().to_owned(),
        Slot::Password => "•".repeat(form.password.typed_count()),
        Slot::Tunnel => form.tunnel.text().to_owned(),
        Slot::Writes => format!("‹ {} ›", if form.writes == WritePolicy::Allowed { "allowed" } else { "read only" }),
    }
}

fn password_from(from: PasswordFrom) -> &'static str {
    match from {
        PasswordFrom::Nowhere => "none",
        PasswordFrom::Keychain => "keychain",
        PasswordFrom::Env => "environment",
        PasswordFrom::Command => "command",
    }
}

/// Where the terminal's cursor goes: after the text before it, on a text line that has the keys.
fn cursor(form: &Form, lines: &[Line<'_>], inside: Rect) -> Option<Position> {
    let field = match form.at {
        Slot::Name => &form.name,
        Slot::Url => &form.url,
        Slot::PasswordWhere => &form.password_where,
        Slot::Tunnel => &form.tunnel,
        Slot::Tier | Slot::PasswordFrom | Slot::Password | Slot::Writes => return None,
    };
    let row =
        form.slots().iter().take_while(|&&slot| slot != form.at).map(|&slot| 1 + usize::from(form.error(slot).is_some())).sum::<usize>();
    let (before, _, _) = field.split();
    let column = 1 + LABEL_WIDTH + Line::from(before).width();
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
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("form", terminal.backend());
    }
}
