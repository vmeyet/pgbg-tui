//! The figures of the grid's column over the screen, framed as the row's detail: distinct values, NULLs, the least and
//! the greatest, then the most common with how many rows hold each, every value raw and escaped.
use super::app::{App, ColumnStats, Overlay};
use crate::act::{self, Pane};
use crate::db::model::Value;
use crate::render;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use vtui::pane;

const WIDTH: u16 = 60;
const LABEL_WIDTH: usize = 12;
/// The frame's two sides, or its top and bottom.
const BORDERS: u16 = 2;

/// The figures when they are open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(Overlay::ColumnStats(stats)) = app.overlay() else { return };
    let theme = app.theme();
    let title = format!("{} ─ over the {} rows loaded", render::printable(stats.column.as_str()), render::grouped(stats.rows));
    let width = WIDTH.min(f.area().width);
    let mut shown = lines(app, stats, usize::from(width.saturating_sub(BORDERS)));
    let hints = act::footer(Pane::ColumnStats).map(|hint| format!("{} {}", hint.keys, hint.does)).collect::<Vec<_>>().join(" · ");
    shown.push(Line::styled(hints, Style::default().fg(theme.faded)));
    let area = centred(f.area(), width, shown.len());
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(shown).block(pane::framed(theme, &title, true)).style(Style::default().bg(theme.surface)), area);
}

fn lines(app: &App, stats: &ColumnStats, room: usize) -> Vec<Line<'static>> {
    let muted = Style::default().fg(app.theme().muted);
    let figure = |label: &str, text: String| Line::from(vec![Span::styled(format!("{label:<LABEL_WIDTH$}"), muted), Span::raw(text)]);
    let value = |value: Option<&Value>| value.map_or_else(|| "–".to_owned(), |value| render::cell_text(value, room));
    let mut lines = vec![
        figure("distinct", render::grouped(stats.distinct)),
        figure("NULL", render::grouped(stats.nulls)),
        figure("least", value(stats.least.as_ref())),
        figure("greatest", value(stats.greatest.as_ref())),
        Line::default(),
        Line::styled("most common", Style::default().add_modifier(Modifier::BOLD)),
    ];
    let widest = stats.common.iter().map(|counted| render::grouped(counted.rows).len()).max().unwrap_or(0);
    lines.extend(stats.common.iter().map(|counted| {
        let rows = format!("{:>widest$}× ", render::grouped(counted.rows));
        Line::from(vec![Span::styled(rows, muted), Span::raw(render::cell_text(&counted.value, room))])
    }));
    lines
}

/// `lines` inside a frame `width` wide, centred across `area` and a third of the way down.
fn centred(area: Rect, width: u16, lines: usize) -> Rect {
    let height = u16::try_from(lines).unwrap_or(u16::MAX).saturating_add(BORDERS).min(area.height);
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 3, width, height }
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::{on_the_result, press};
    use super::super::ui;
    use crate::db::model::{ColumnKind, ResultSet, Value};
    use crate::fixtures::column;
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn the_figures_show_over_the_screen_with_the_rows_they_are_counted_over() {
        let statuses = ["paid", "open", "paid", "", "paid", "void"];
        let rows = statuses.map(|status| vec![if status.is_empty() { Value::Null } else { Value::Text(status.into()) }]).into();
        let mut app = on_the_result(ResultSet::whole(vec![column("status", ColumnKind::Text)], rows));
        press(&mut app, KeyCode::Char('i'));
        let mut terminal = Terminal::new(TestBackend::new(70, 18)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, &app)).unwrap();
        insta::assert_snapshot!("column_stats", terminal.backend());
    }
}
