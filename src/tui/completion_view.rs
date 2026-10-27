//! The `ctrl-space` list in a popup at the editor's cursor: each name and what it names, the pick highlighted.
use super::app::{App, Completions, Overlay};
use super::editor_view;
use crate::complete::Kind;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

/// The names the popup shows at once; the list scrolls to keep the pick in view.
const SHOWN_ROWS: usize = 8;
/// Two borders and a column between a name and its kind.
const FRAME_COLUMNS: usize = 3;
const BORDER_ROWS: usize = 2;

/// The list when it is open, under the cursor of the editor drawn in `editor`, or over it when no room is left below.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, editor: Rect) {
    let Some(Overlay::Completion(completions)) = &app.overlay else { return };
    let area = popup_area(f.area(), editor_view::cursor_at(app, editor), completions);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::default().fg(app.theme.accent));
    let inside = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines(app, completions, usize::from(inside.width))), inside);
}

fn popup_area(screen: Rect, cursor: Position, completions: &Completions) -> Rect {
    let widest = completions.candidates.iter().map(|candidate| Span::raw(&candidate.name).width() + kind_word(candidate.kind).len());
    let width = to_cells(widest.max().unwrap_or(0) + FRAME_COLUMNS).min(screen.width);
    let height = to_cells(completions.candidates.len().min(SHOWN_ROWS) + BORDER_ROWS).min(screen.height);
    let below = cursor.y.saturating_add(1);
    let y = if below.saturating_add(height) <= screen.bottom() { below } else { cursor.y.saturating_sub(height) };
    let x = cursor.x.min(screen.right().saturating_sub(width));
    Rect { x, y, width, height }
}

/// The names in view, each with its kind pushed to the right, the pick reversed.
fn lines(app: &App, completions: &Completions, room: usize) -> Vec<Line<'static>> {
    let first = completions.selected.saturating_sub(SHOWN_ROWS - 1);
    let shown = completions.candidates.iter().enumerate().skip(first).take(SHOWN_ROWS);
    let line = |(at, candidate): (usize, &crate::complete::Candidate)| {
        let kind = kind_word(candidate.kind);
        let gap = room.saturating_sub(Span::raw(&candidate.name).width() + kind.len());
        let style = if at == completions.selected { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
        let name = Span::styled(format!("{}{}", candidate.name, " ".repeat(gap)), style);
        Line::from(vec![name, Span::styled(kind, style.fg(app.theme.faded))])
    };
    shown.map(line).collect()
}

fn kind_word(kind: Kind) -> &'static str {
    match kind {
        Kind::Table => "table",
        Kind::Column => "column",
        Kind::Keyword => "keyword",
    }
}

fn to_cells(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::super::app::tests::typed;
    use super::super::ui;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn the_list_opens_under_the_cursor_with_the_pick_first() {
        let mut app = typed("SELECT o");
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("completion_list", terminal.backend());
    }
}
