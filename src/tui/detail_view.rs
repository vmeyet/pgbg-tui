//! The row's detail over the screen: each column's name on the left and its whole value beside it, wrapped under
//! itself, in the grid's format and colours, then the tables pointing to the row.
//! Every piece of text is drawn through `render::printable`.
use super::app::{
    App, Clock, Colouring, Count, Counted, Detail, DetailFit, DetailScroll, DrawnDetail, DrawnRow, GAP, Grid, Overlay, RowOf, Run, ShownRow,
};
use super::ink::{Ink, ink};
use super::palette_footer;
use crate::act::Pane;
use crate::db::model::{Column, Value};
use crate::format::{self, Format};
use crate::keys::{COUNTED_ROWS, Keys};
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
/// The widest column name drawn whole; a longer one is cut with `…`, so one long name never squeezes every value.
const LABEL_MAX_WIDTH: usize = 28;
const INDENT: &str = "  ";
const SHARE_OF_SCREEN_PCT: u16 = 80;
/// The narrowest popup, so a value keeps room on a small terminal; a screen narrower than it is taken whole.
const MIN_WIDTH: u16 = 60;
const POINTING_HEADING: &str = "Pointing to it ";

/// The detail when it is open; returns the line it started at, how far it can scroll, and its lines when it built
/// them.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) -> DetailFit {
    let Some(Overlay::Detail(detail)) = app.overlay() else { return DetailFit::default() };
    let Some(shown) = app.shown_row(detail) else { return DetailFit::default() };
    let area = centred(f.area());
    let block = pane::framed(app.theme(), &title(app, &shown), true);
    let [body, footer] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(block.inner(area));
    let of = DrawnRow { connection: app.sheet().name().clone(), row: shown.of, width: body.width };
    let kept = app.drawn_detail().filter(|drawn| drawn.of == of);
    let built = kept.is_none().then(|| drawn(app, detail, &shown, of));
    let Some(DrawnDetail { lines, column_starts, .. }) = kept.or(built.as_ref()) else { return DetailFit::default() };
    let pointing = pointing_lines(app.theme(), &detail.referrers, body.width);
    let last_scroll = (lines.len() + pointing.len()).saturating_sub(usize::from(body.height));
    let wanted = match detail.scroll {
        DetailScroll::ToColumn(column) => column_starts.get(column).copied().unwrap_or(lines.len()),
        DetailScroll::Lines(line) => line,
    };
    let first_line = wanted.min(last_scroll);
    let targets = shown.targets();
    let on_key = match targets.get(detail.key) {
        Some(target) => column_starts.get(target.first_column()).copied(),
        None => Some(lines.len() + 1 + detail.key - targets.len()),
    };
    let lit = |(at, line): (usize, &Line<'static>)| {
        if Some(at) == on_key { line.clone().patch_style(Style::default().add_modifier(Modifier::REVERSED)) } else { line.clone() }
    };
    let every_line = lines.iter().chain(&pointing).enumerate();
    let visible: Vec<Line<'static>> = every_line.skip(first_line).take(usize::from(body.height)).map(lit).collect();
    f.render_widget(Clear, area);
    f.render_widget(block.style(Style::default().bg(app.theme().surface)), area);
    f.render_widget(Paragraph::new(visible), body);
    f.render_widget(Paragraph::new(footer_line(app.theme(), detail.note.as_deref(), footer.width)), footer);
    DetailFit { first_line, last_scroll, drawn: built }
}

/// A faint rule headed `Pointing to it`, then the tables whose keys point to the row, each with how many of its rows do
/// on the right; none when no table does.
fn pointing_lines(theme: &Theme, referrers: &[Counted], width: u16) -> Vec<Line<'static>> {
    if referrers.is_empty() {
        return vec![];
    }
    let width = usize::from(width);
    let rule = "─".repeat(width.saturating_sub(POINTING_HEADING.len()));
    let heading = Line::from(vec![
        Span::styled(POINTING_HEADING, Style::default().fg(theme.muted).add_modifier(Modifier::BOLD)),
        Span::styled(rule, faded(theme)),
    ]);
    let line = |counted: &Counted| {
        let count = count_text(&counted.count);
        let columns: Vec<&str> = counted.referrer.pairs.iter().map(|pair| pair.column.as_str()).collect();
        let room = width.saturating_sub(INDENT.len() + render::width(&count));
        let key = text::truncate(&render::printable(&format!("{}.{}", counted.referrer.table, columns.join(", "))), room - GAP.len());
        Line::from(vec![
            Span::raw(format!("{INDENT}{}", render::padded(&key, room))),
            Span::styled(count, Style::default().fg(theme.muted)),
        ])
    };
    std::iter::once(heading).chain(referrers.iter().map(line)).collect()
}

fn count_text(count: &Count) -> String {
    match count {
        Count::Queued | Count::Counting(_) => "…".to_owned(),
        Count::Rows(rows) if *rows > COUNTED_ROWS => format!("{COUNTED_ROWS}+"),
        Count::Rows(rows) => rows.to_string(),
        Count::NotCounted => "—".to_owned(),
        Count::Failed(_) => "!".to_owned(),
    }
}

/// `Row 2 of 4` on the grid's row, else the tables of the trail that led to the row shown.
fn title(app: &App, shown: &ShownRow<'_>) -> String {
    match (shown.of, &app.sheet().run) {
        (RowOf::Grid { row, .. }, Run::Done { result, .. }) => format!("Row {} of {}", row + 1, result.rows().len()),
        _ => shown.trail.iter().map(|table| render::printable(table)).collect::<Vec<_>>().join(" › "),
    }
}

/// Why the last key followed nothing, else the keys.
fn footer_line(theme: &Theme, note: Option<&str>, columns: u16) -> Line<'static> {
    match note {
        Some(note) => Line::styled(render::printable(note).into_owned(), Style::default().fg(theme.warn)),
        None => Line::styled(palette_footer::fitted(Pane::Detail, usize::from(columns)), faded(theme)),
    }
}

/// How the grid reads a column: its format, and how its values are coloured.
#[derive(Clone, Copy, Debug)]
struct Look {
    format: Format,
    colouring: Colouring,
}

impl Look {
    const RAW: Self = Self { format: Format::Raw, colouring: Colouring::Plain };
}

/// Each column's look: the grid's on the grid's row, and the one a new grid opens in on a row a key led to.
fn looks(app: &App, detail: &Detail) -> Vec<Look> {
    let opened;
    let grid = match (detail.trail.last(), &app.sheet().run) {
        (Some(followed), _) => {
            let formats = format::opening(followed.result.columns(), &[], app.format_mode());
            opened = Grid::new(&followed.result, formats, Keys::default(), app.clock());
            &opened
        }
        (None, Run::Done { grid, .. }) => grid,
        (None, _) => return vec![],
    };
    grid.formats.iter().zip(&grid.colourings).map(|(&format, &colouring)| Look { format, colouring }).collect()
}

/// The terminal columns of the names on the left, and of the values beside them.
#[derive(Clone, Copy, Debug)]
struct Widths {
    label: usize,
    value: usize,
}

impl Widths {
    /// Room for the longest name, at most `LABEL_MAX_WIDTH` and a third of `width`; the rest goes to the values.
    fn of(columns: &[Column], width: u16) -> Self {
        let width = usize::from(width);
        let longest = columns.iter().map(|column| render::width(&render::printable(column.name.as_str()))).max().unwrap_or(0);
        let label = longest.min(LABEL_MAX_WIDTH).min(width / 3);
        Self { label, value: width.saturating_sub(label + GAP.len()) }
    }
}

/// Every column of the row shown as lines `of.width` columns wide, a key's column naming the table it points to.
fn drawn(app: &App, detail: &Detail, shown: &ShownRow<'_>, of: DrawnRow) -> DrawnDetail {
    let (theme, looks, widths) = (app.theme(), looks(app, detail), Widths::of(shown.columns, of.width));
    let column = |(at, (column, value)): (usize, (&Column, &Value))| {
        let mut value = value_spans(theme, value, looks.get(at).copied().unwrap_or(Look::RAW), app.clock());
        let key = shown.points_to(at).map(|target| Span::styled(format!("{GAP}→ {}", render::printable(&target.table)), faded(theme)));
        if let (Some(last), Some(key)) = (value.last_mut(), key) {
            last.push(key);
        }
        column_lines(label(theme, column, shown.is_primary(at), widths.label), value, widths)
    };
    let by_column: Vec<Vec<Line<'static>>> = shown.columns.iter().zip(shown.row).enumerate().map(column).collect();
    let column_starts = by_column.iter().scan(0, |start, lines| Some(std::mem::replace(start, *start + lines.len()))).collect();
    DrawnDetail { of, lines: by_column.into_iter().flatten().collect(), column_starts }
}

/// The column's name cut and padded to `width`, muted, bold for a primary key.
fn label(theme: &Theme, column: &Column, is_primary: bool, width: usize) -> Span<'static> {
    let style = Style::default().fg(theme.muted);
    let style = if is_primary { style.add_modifier(Modifier::BOLD) } else { style };
    Span::styled(render::padded(&text::truncate(&render::printable(column.name.as_str()), width), width), style)
}

/// `label` beside the first line of `value`; each line of `value` wrapped within the values' width, under itself.
fn column_lines(label: Span<'static>, value: Vec<Vec<Span<'static>>>, widths: Widths) -> Vec<Line<'static>> {
    let under = Span::raw(" ".repeat(widths.label));
    let heads = std::iter::once(label).chain(std::iter::repeat(under));
    let wrapped = value.into_iter().flat_map(|spans| text::wrap_line(&Line::from(spans), widths.value));
    heads.zip(wrapped).map(|(head, line)| Line::from([head, Span::raw(GAP)].into_iter().chain(line.spans).collect::<Vec<_>>())).collect()
}

/// Each line of the value in its column's format and the grid's colours, the raw text faded after a format that
/// changed it, then a note when the value was cut.
fn value_spans(theme: &Theme, value: &Value, look: Look, clock: &Clock) -> Vec<Vec<Span<'static>>> {
    let mut raw = value.pretty_prefix(VALUE_MAX_CHARS + 1);
    let cut_at = raw.char_indices().nth(VALUE_MAX_CHARS).map(|(at, _)| at);
    if let Some(at) = cut_at {
        raw.truncate(at);
    }
    let formatted = format::formatted(value, look.format, clock).filter(|formatted| *formatted != raw);
    let lines = match formatted {
        Some(formatted) => {
            let raw = Span::styled(format!("{GAP}{}", render::printable(&raw)), faded(theme));
            vec![inked(theme, &formatted, value, look.colouring).into_iter().chain([raw]).collect()]
        }
        None => raw.split('\n').map(|line| inked(theme, line, value, look.colouring)).collect(),
    };
    let note = cut_at.map(|_| vec![Span::styled(format!("… the first {VALUE_MAX_CHARS} characters"), Style::default().fg(theme.muted))]);
    lines.into_iter().chain(note).collect()
}

/// One line of a value in the colours the grid gives it, each piece escaped on its own.
fn inked(theme: &Theme, line: &str, value: &Value, colouring: Colouring) -> Vec<Span<'static>> {
    let piece = |text: &str, style: Style| Span::styled(render::printable(text).into_owned(), style);
    let spans = match ink(line, value, colouring, theme) {
        Ink::Whole(style) => vec![piece(line, style)],
        Ink::Split { at, head, tail } => vec![piece(&line[..at], head), piece(&line[at..], tail)],
        Ink::Json => painted_spans(theme, line, &syntax::highlight_json_line(line)),
    };
    spans.into_iter().filter(|span| !span.content.is_empty()).collect()
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
    spans
}

fn faded(theme: &Theme) -> Style {
    Style::default().fg(theme.faded)
}

/// `SHARE_OF_SCREEN_PCT` of the screen, and at least `MIN_WIDTH` columns wide.
fn centred(area: Rect) -> Rect {
    let width = share_of_screen(area.width).max(area.width.min(MIN_WIDTH));
    let height = share_of_screen(area.height);
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

/// `SHARE_OF_SCREEN_PCT` of `cells`, computed in a type wide enough for any screen.
fn share_of_screen(cells: u16) -> u16 {
    u16::try_from(u32::from(cells) * u32::from(SHARE_OF_SCREEN_PCT) / 100).unwrap_or(cells)
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::{on_the_keyed_orders, to_shop, typed};
    use super::super::app::{Fit, Incoming, LookId, RunId};
    use super::super::styled;
    use super::*;
    use crate::db::model::{ColumnKind, JsonText, ResultSet};
    use crate::fixtures::{column, keyed_customer, orders};
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
        let columns = orders.columns().iter().cloned().chain([column("note", ColumnKind::Text)]);
        let rows = orders.rows().iter().map(|row| row.iter().cloned().chain([Value::Text(note.into())]).collect());
        ResultSet::whole(columns.collect(), rows.collect())
    }

    /// A row of every kind the grid colours, a long name and a long text among them.
    fn of_every_kind() -> ResultSet {
        let columns = vec![
            column("id", ColumnKind::Int),
            column("paid", ColumnKind::Bool),
            column("reference", ColumnKind::Uuid),
            column("placed_at", ColumnKind::TimestampTz),
            column("items", ColumnKind::Json),
            column("instructions_for_the_courier_on_arrival", ColumnKind::Text),
            column("customer_id", ColumnKind::Int),
        ];
        let row = vec![
            Value::Int(7),
            Value::Bool(true),
            Value::Uuid(uuid::Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
            Value::TimestampTz(chrono::DateTime::UNIX_EPOCH),
            Value::Json(JsonText::of(&serde_json::json!({"sku": "TEA-01", "count": 2}))),
            Value::Text("Leave the parcel at the door of the shop and ring twice, the bell is weak.".into()),
            Value::Null,
        ];
        ResultSet::whole(columns, vec![row.clone(), row])
    }

    #[test]
    fn each_name_sits_on_the_left_and_its_value_wraps_beside_it() {
        insta::assert_snapshot!("detail", render(&detail_of(a_note()), 40, 20).0.backend());
    }

    #[test]
    fn a_long_name_is_cut_and_every_value_lines_up_after_the_names() {
        insta::assert_snapshot!("detail_every_kind", render(&detail_of(of_every_kind()), 100, 24).0.backend());
    }

    #[test]
    fn each_value_takes_the_colours_the_grid_gives_it() {
        insta::assert_snapshot!(
            "detail_every_kind_styled",
            styled::runs(render(&detail_of(of_every_kind()), 100, 24).0.backend().buffer())
        );
    }

    #[test]
    fn a_narrow_terminal_keeps_a_third_for_the_names_and_the_rest_for_the_values() {
        insta::assert_snapshot!("detail_narrow", render(&detail_of(of_every_kind()), 60, 24).0.backend());
    }

    #[test]
    fn a_column_in_another_format_shows_it_then_its_raw_value_faded() {
        let mut app = detail_of(of_every_kind());
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        (0..2).for_each(|_| _ = app.handle_key(KeyEvent::from(KeyCode::Char('l'))));
        app.handle_key(KeyEvent::from(KeyCode::Char('f')));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let runs = styled::runs(render(&app, 100, 24).0.backend().buffer());
        assert!(runs.contains("0a1b2c3d\"\nIndexed(240)/NONE \"  0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d\""), "{runs}");
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
        assert_eq!(fit.last_scroll, 1);
    }

    #[test]
    fn a_scrolled_detail_starts_further_down() {
        let mut app = detail_of(a_note());
        app.fit(Fit { detail: DetailFit { first_line: 0, last_scroll: 1, ..DetailFit::default() }, ..Fit::default() });
        (0..3).for_each(|_| _ = app.handle_key(KeyEvent::from(KeyCode::Char('j'))));
        insta::assert_snapshot!("detail_scrolled", render(&app, 40, 10).0.backend());
    }

    #[test]
    fn the_detail_opened_on_a_column_starts_at_it() {
        let mut app = detail_of(a_note());
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        (0..3).for_each(|_| _ = app.handle_key(KeyEvent::from(KeyCode::Char('l'))));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let (_, fit) = render(&app, 40, 10);
        assert_eq!((fit.first_line, fit.last_scroll), (1, 1));
        let (terminal, fit) = render(&app, 40, 30);
        assert_eq!(fit.first_line, 0, "{:?}", terminal.backend());
    }

    #[test]
    fn the_popup_takes_its_share_of_a_screen_wider_than_820_columns() {
        assert_eq!(centred(Rect::new(0, 0, 1000, 50)), Rect::new(100, 5, 800, 40));
    }

    /// The keyed orders with `⏎` pressed on order 7's customer.
    fn on_a_key() -> App {
        let mut app = on_the_keyed_orders();
        app.handle_key(KeyEvent::from(KeyCode::Char('l')));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app
    }

    #[test]
    fn a_followed_row_shows_the_trail_that_led_to_it_where_its_keys_point_and_what_points_to_it() {
        let mut app = on_a_key();
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) }));
        insta::assert_snapshot!("detail_followed", render(&app, 60, 14).0.backend());
    }

    #[test]
    fn the_tables_pointing_to_the_row_show_with_their_counts() {
        let mut app = on_the_keyed_orders();
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let count = ResultSet::whole(vec![column("count", ColumnKind::Int)], vec![vec![Value::Int(1001)]]);
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(count) }));
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        insta::assert_snapshot!("detail_pointing", render(&app, 60, 16).0.backend());
    }

    #[test]
    fn the_key_under_the_cursor_is_lit() {
        let mut app = on_a_key();
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(keyed_customer()) }));
        let runs = styled::runs(render(&app, 60, 14).0.backend().buffer());
        assert!(runs.contains("REVERSED \"user_id\""), "{runs}");
    }

    #[test]
    fn the_name_of_a_primary_key_is_bold() {
        let mut app = on_the_keyed_orders();
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let runs = styled::runs(render(&app, 60, 16).0.backend().buffer());
        assert!(runs.contains("BOLD \"id ") && !runs.contains("BOLD \"total"), "{runs}");
    }

    #[test]
    fn a_key_pointing_to_no_row_says_so_on_the_footer() {
        let mut app = on_a_key();
        app.apply(to_shop(Incoming::LookedUp { look: LookId::FIRST, rows: Ok(ResultSet::whole(vec![], vec![])) }));
        let drawn = render(&app, 60, 14).0.backend().to_string();
        assert!(drawn.contains("no row of customers has this key"), "{drawn}");
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
