//! The row's detail over the screen: each column's name and type, with its whole value below, wrapped, and JSON
//! indented and coloured.
//! Every piece of text is drawn through `render::printable`.
use super::app::{App, DetailFit, DetailScroll, DrawnDetail, DrawnRow, Overlay, Run};
use crate::act::{self, Pane};
use crate::db::model::{Column, Value};
use crate::render;
use crate::syntax::{self, Painted};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use vtui::theme::Theme;
use vtui::{pane, text};

/// The longest value drawn, so a 10 MB document costs no more than a page of it.
const VALUE_MAX_CHARS: usize = 64 * 1024;
const INDENT: &str = "  ";
const SHARE_OF_SCREEN_PCT: u16 = 80;

/// The detail when it is open; returns the line it started at, how far it can scroll, and its lines when it built
/// them.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) -> DetailFit {
    let Some(Overlay::Detail(detail)) = app.overlay() else { return DetailFit::default() };
    let Run::Done { run, result, grid, .. } = &app.sheet().run else { return DetailFit::default() };
    let Some(row) = result.rows().get(grid.row) else { return DetailFit::default() };
    let area = centred(f.area());
    let block = pane::framed(app.theme(), &format!("Row {} of {}", grid.row + 1, result.rows().len()), true);
    let [body, footer] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(block.inner(area));
    let of = DrawnRow { connection: app.sheet().name().clone(), run: *run, row: grid.row, width: body.width };
    let kept = app.drawn_detail().filter(|drawn| drawn.of == of);
    let built = kept.is_none().then(|| drawn(app.theme(), result.columns(), row, of));
    let Some(DrawnDetail { lines, column_starts, .. }) = kept.or(built.as_ref()) else { return DetailFit::default() };
    let last_scroll = lines.len().saturating_sub(usize::from(body.height));
    let wanted = match detail.scroll {
        DetailScroll::ToColumn(column) => column_starts.get(column).copied().unwrap_or_default(),
        DetailScroll::Lines(line) => line,
    };
    let first_line = wanted.min(last_scroll);
    let shown: Vec<Line<'static>> = lines.iter().skip(first_line).take(usize::from(body.height)).cloned().collect();
    f.render_widget(Clear, area);
    f.render_widget(block.style(Style::default().bg(app.theme().surface)), area);
    f.render_widget(Paragraph::new(shown), body);
    f.render_widget(Paragraph::new(Line::styled(hints(), Style::default().fg(app.theme().faded))), footer);
    DetailFit { first_line, last_scroll, drawn: built }
}

/// Every column of `row` as lines `of.width` columns wide.
fn drawn(theme: &Theme, columns: &[Column], row: &[Value], of: DrawnRow) -> DrawnDetail {
    let by_column: Vec<Vec<Line<'static>>> =
        columns.iter().zip(row).map(|(column, value)| column_lines(theme, column, value, of.width)).collect();
    let column_starts = by_column.iter().scan(0, |start, lines| Some(std::mem::replace(start, *start + lines.len()))).collect();
    DrawnDetail { of, lines: by_column.into_iter().flatten().collect(), column_starts }
}

fn centred(area: Rect) -> Rect {
    let width = share_of_screen(area.width);
    let height = share_of_screen(area.height);
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

/// `SHARE_OF_SCREEN_PCT` of `cells`, computed in a type wide enough for any screen.
fn share_of_screen(cells: u16) -> u16 {
    u16::try_from(u32::from(cells) * u32::from(SHARE_OF_SCREEN_PCT) / 100).unwrap_or(cells)
}

fn hints() -> String {
    act::footer(Pane::Detail).map(|hint| format!("{} {}", hint.keys, hint.does)).collect::<Vec<_>>().join(" · ")
}

/// The column's name and type, its value below, then a blank line.
fn column_lines(theme: &Theme, column: &Column, value: &Value, width: u16) -> Vec<Line<'static>> {
    let name = Span::styled(render::printable(column.name.as_str()).into_owned(), Style::default().add_modifier(Modifier::BOLD));
    let type_name = Span::styled(format!("  {}", render::printable(column.type_name.as_str())), Style::default().fg(theme.muted));
    let header = if column.type_name.as_str().is_empty() { Line::from(name) } else { Line::from(vec![name, type_name]) };
    let room = usize::from(width).saturating_sub(INDENT.len());
    let value_lines = value_spans(theme, value).into_iter().flat_map(|spans| text::wrap_line(&Line::from(spans), room));
    let indented = value_lines.map(|line| Line::from(std::iter::once(Span::raw(INDENT)).chain(line.spans).collect::<Vec<_>>()));
    std::iter::once(header).chain(indented).chain([Line::default()]).collect()
}

/// Each line of the value as coloured spans, then a note when the value was cut.
fn value_spans(theme: &Theme, value: &Value) -> Vec<Vec<Span<'static>>> {
    if *value == Value::Null {
        return vec![vec![Span::styled("NULL", Style::default().fg(theme.faded))]];
    }
    let text = value.pretty_prefix(VALUE_MAX_CHARS + 1);
    let is_cut = text.chars().nth(VALUE_MAX_CHARS).is_some();
    let kept: String = text.chars().take(VALUE_MAX_CHARS).collect();
    let is_json = matches!(value, Value::Json(_));
    let lines = kept.split('\n').map(|line| painted_spans(theme, line, &line_colours(line, is_json)));
    let note = is_cut.then(|| vec![Span::styled(format!("… the first {VALUE_MAX_CHARS} characters"), Style::default().fg(theme.muted))]);
    lines.chain(note).collect()
}

fn line_colours(line: &str, is_json: bool) -> Vec<Painted> {
    if is_json { syntax::highlight_json_line(line) } else { vec![] }
}

/// `line` split at its painted pieces, each piece escaped on its own and drawn in its token's colour.
fn painted_spans(theme: &Theme, line: &str, painted: &[Painted]) -> Vec<Span<'static>> {
    let mut spans = vec![];
    let mut at = 0;
    for piece in painted {
        spans.push(Span::raw(render::printable(&line[at..piece.bytes.start]).into_owned()));
        let colour = Style::default().fg(theme.syntax.colour(piece.token));
        spans.push(Span::styled(render::printable(&line[piece.bytes.clone()]).into_owned(), colour));
        at = piece.bytes.end;
    }
    spans.push(Span::raw(render::printable(&line[at..]).into_owned()));
    spans.into_iter().filter(|span| !span.content.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::{to_shop, typed};
    use super::super::app::{Fit, Incoming, RunId};
    use super::super::styled;
    use super::*;
    use crate::db::model::{ColumnKind, JsonText, ResultSet, TypeName};
    use crate::fixtures::{column, orders};
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::Duration;

    fn detail_of(result: ResultSet) -> App {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result, took: Duration::from_millis(38) }));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        app.handle_key(KeyEvent::from(KeyCode::Char('j')));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app
    }

    fn render(app: &App, width: u16, height: u16) -> (Terminal<TestBackend>, DetailFit) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut fit = DetailFit::default();
        terminal.draw(|f| fit = draw(f, app)).unwrap();
        (terminal, fit)
    }

    fn a_note() -> ResultSet {
        let note = "Leave at the door.\nRing twice: the bell is \u{202e}broken";
        let orders = orders();
        let columns =
            orders.columns().iter().cloned().chain([Column { type_name: TypeName::new("text"), ..column("note", ColumnKind::Text) }]);
        let rows = orders.rows().iter().map(|row| row.iter().cloned().chain([Value::Text(note.into())]).collect());
        ResultSet::whole(columns.collect(), rows.collect())
    }

    #[test]
    fn each_column_shows_its_name_type_and_whole_value_wrapped() {
        insta::assert_snapshot!("detail", render(&detail_of(a_note()), 40, 20).0.backend());
    }

    #[test]
    fn a_value_from_the_database_cannot_reverse_the_detail() {
        let drawn = render(&detail_of(a_note()), 40, 20).0.backend().to_string();
        assert!(drawn.contains("{202e}broken") && !drawn.contains('\u{202e}'), "{drawn}");
    }

    #[test]
    fn json_is_indented_and_coloured() {
        let items = Value::Json(JsonText::of(&serde_json::json!({"sku": "TEA-01", "count": 2, "gift": null})));
        let result = ResultSet::whole(vec![column("items", ColumnKind::Json)], vec![vec![items.clone()], vec![items]]);
        insta::assert_snapshot!("detail_json_styled", styled::runs(render(&detail_of(result), 40, 12).0.backend().buffer()));
    }

    #[test]
    fn a_detail_taller_than_the_popup_says_how_far_it_scrolls() {
        let (_, fit) = render(&detail_of(a_note()), 40, 10);
        assert_eq!(fit.last_scroll, 9);
    }

    #[test]
    fn a_scrolled_detail_starts_further_down() {
        let mut app = detail_of(a_note());
        app.fit(Fit { detail: DetailFit { first_line: 0, last_scroll: 9, ..DetailFit::default() }, ..Fit::default() });
        (0..3).for_each(|_| _ = app.handle_key(KeyEvent::from(KeyCode::Char('j'))));
        insta::assert_snapshot!("detail_scrolled", render(&app, 40, 10).0.backend());
    }

    #[test]
    fn the_detail_opened_on_a_column_starts_at_it() {
        let mut app = detail_of(a_note());
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        (0..3).for_each(|_| _ = app.handle_key(KeyEvent::from(KeyCode::Char('l'))));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let (_, fit) = render(&app, 40, 20);
        assert_eq!((fit.first_line, fit.last_scroll), (1, 1));
        let (terminal, fit) = render(&app, 40, 30);
        assert_eq!(fit.first_line, 0, "{:?}", terminal.backend());
    }

    #[test]
    fn the_popup_takes_its_share_of_a_screen_wider_than_820_columns() {
        assert_eq!(centred(Rect::new(0, 0, 1000, 50)), Rect::new(100, 5, 800, 40));
    }

    #[test]
    fn a_frame_that_changes_nothing_builds_no_lines_and_a_new_width_builds_them_again() {
        let mut app = detail_of(a_note());
        let (_, first) = render(&app, 40, 20);
        assert!(first.drawn.is_some());
        app.fit(Fit { detail: first, ..Fit::default() });
        assert!(render(&app, 40, 20).1.drawn.is_none());
        assert!(render(&app, 50, 20).1.drawn.is_some());
    }
}
