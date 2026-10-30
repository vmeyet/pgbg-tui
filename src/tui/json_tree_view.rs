//! The JSON of the grid's cell as a tree over the screen, framed as the row's detail: one value a line, indented, `▾`
//! before an open object or array and `▸` before a folded one, coloured as the grid colours JSON, the cursor's line lit.
use super::app::{App, Mark, Overlay, TreeLine};
use super::detail_view::{centred, painted_spans};
use crate::act::{self, Pane};
use crate::render;
use crate::syntax;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use vtui::pane;
use vtui::theme::Theme;

const INDENT: &str = "  ";

/// The tree when it is open, scrolled so the cursor's line shows.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(Overlay::JsonTree(tree)) = app.overlay() else { return };
    let theme = app.theme();
    let area = centred(f.area());
    let title = format!("{} ─ row {}", render::printable(tree.column.as_str()), render::grouped(tree.row));
    let block = pane::framed(theme, &title, true);
    let [body, footer] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(block.inner(area));
    let height = usize::from(body.height);
    let first = tree.at.saturating_sub(height.saturating_sub(1));
    let window = tree.lines_in(first, height);
    let lines: Vec<Line<'static>> = window.iter().zip(first..).map(|(line, at)| drawn(theme, line, at == tree.at)).collect();
    f.render_widget(Clear, area);
    f.render_widget(block.style(Style::default().bg(theme.surface)), area);
    f.render_widget(Paragraph::new(lines), body);
    f.render_widget(Paragraph::new(Line::styled(hints(), Style::default().fg(theme.faded))), footer);
}

fn hints() -> String {
    act::footer(Pane::JsonTree).map(|hint| format!("{} {}", hint.keys, hint.does)).collect::<Vec<_>>().join(" · ")
}

fn drawn(theme: &Theme, line: &TreeLine, is_cursor: bool) -> Line<'static> {
    let mark = match line.mark {
        Mark::Open => "▾ ",
        Mark::Folded => "▸ ",
        Mark::Leaf => "  ",
    };
    let lead = Span::styled(format!("{}{mark}", INDENT.repeat(line.depth)), Style::default().fg(theme.accent));
    let text = painted_spans(theme, &line.text, &syntax::highlight_json_line(&line.text));
    let drawn = Line::from(std::iter::once(lead).chain(text).collect::<Vec<_>>());
    if is_cursor { drawn.patch_style(Style::default().add_modifier(Modifier::REVERSED)) } else { drawn }
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::{on_the_result, press};
    use super::super::styled;
    use super::super::ui;
    use crate::db::model::{ColumnKind, JsonText, ResultSet, Value};
    use crate::fixtures::column;
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn in_the_tree() -> super::super::app::App {
        let json = JsonText::compacted(r#"{"id":7,"items":[{"sku":"TEA-01","count":2}],"gift":null}"#).unwrap();
        let mut app = on_the_result(ResultSet::whole(vec![column("data", ColumnKind::Json)], vec![vec![Value::Json(json)]]));
        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        app
    }

    fn render(app: &super::super::app::App) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(70, 14)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, app)).unwrap();
        terminal
    }

    #[test]
    fn a_folded_array_draws_in_the_grids_json_colours() {
        let mut app = in_the_tree();
        press(&mut app, KeyCode::Enter);
        insta::assert_snapshot!("json_tree_folded_styled", styled::runs(render(&app).backend().buffer()));
    }
}
