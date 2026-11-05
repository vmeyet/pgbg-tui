//! The SQL editor pane: the text in its syntax colours, the cursor kept in view, the server's error position, and
//! whether the connection takes writes.
use super::app::wrap::{self, Ghost, InView, Row};
use super::app::{App, Focus};
use crate::connection::{ConnectionName, Tier, WritePolicy};
use crate::render;
use crate::syntax::{Painted, Spans};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use std::ops::Range;
use vtui::pane;
use vtui::text;
use vtui::theme::Theme;

/// The editor in `area`, scrolled so the cursor's line shows, with the terminal's cursor in it when it has the keys;
/// returns where it drew the text.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) -> Rect {
    let focused = app.focus() == Focus::Editor;
    let block = pane::framed(app.theme(), &title(app), focused).title(badge(app)).title(header_notes(app)).title(open_error(app));
    let inside = block.inner(area);
    f.render_widget(block, area);
    let sheet = app.sheet();
    let (before, _, _) = sheet.editor.split();
    let mark = Style::default().fg(app.theme().danger).add_modifier(Modifier::UNDERLINED);
    let size = inside.as_size();
    let window = Window { first: sheet.editor.first_row_in_view(size), rows: usize::from(size.height), columns: usize::from(size.width) };
    let ghost = sheet.ghost.as_ref().filter(|_| focused).map(|ghost| Ghost { at: before.len(), rest: &ghost.rest });
    let paint = Paint {
        colours: sheet.editor.colours(),
        mark: sheet.error_mark.clone(),
        mark_style: mark,
        selection: sheet.editor.selection(),
        ghost,
        theme: app.theme(),
    };
    let text = coloured(sheet.editor.text(), &paint, window);
    f.render_widget(Paragraph::new(text), inside);
    if app.editor_takes_keys()
        && let Some(cursor) = sheet.editor.cursor_in_view(size)
    {
        f.set_cursor_position(cursor_position(inside, cursor));
    }
    inside
}

/// The position of the terminal's cursor in the editor whose text is drawn in `inside`; its corner when it is out of view.
pub(super) fn cursor_at(app: &App, inside: Rect) -> Position {
    app.sheet().editor.cursor_in_view(inside.as_size()).map_or_else(|| inside.as_position(), |cursor| cursor_position(inside, cursor))
}

/// The screen position of `cursor`, kept inside `inside`.
fn cursor_position(inside: Rect, cursor: InView) -> Position {
    let offset = |at: usize| u16::try_from(at).unwrap_or(u16::MAX);
    let x = inside.x.saturating_add(offset(cursor.column)).min(inside.right().saturating_sub(1));
    Position { x, y: inside.y.saturating_add(offset(cursor.row)) }
}

/// The rows of the editor in view: `rows` of them from `first`, each `columns` wide.
#[derive(Clone, Copy, Debug)]
struct Window {
    first: Row,
    rows: usize,
    columns: usize,
}

/// How the text is painted: its colours per line, the bytes of `selection` reversed, the bytes of `mark` drawn on top in
/// `mark_style`, and the ghost.
struct Paint<'a> {
    colours: &'a [Spans],
    mark: Option<Range<usize>>,
    mark_style: Style,
    selection: Option<Range<usize>>,
    ghost: Option<Ghost<'a>>,
    theme: &'a Theme,
}

/// The rows of `text` in `window`, each token in its colour, the selection reversed and the mark on top; a mark at a
/// line's end draws a space there.
/// The ghost's lines past its first push the text under the cursor's line down, as many as the window holds.
fn coloured<'a>(text: &'a str, paint: &Paint<'_>, window: Window) -> Text<'a> {
    let mut line_start = 0;
    let mut rows = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        if rows.len() >= window.rows {
            break;
        }
        if index >= window.first.line {
            let room = window.rows - rows.len();
            rows.extend(rows_of_line(line, TextLine { index, start: line_start }, paint, window, room));
        }
        line_start += line.len() + 1;
    }
    Text::from(rows)
}

/// Where a line sits in the text: its index, and the byte it starts at.
#[derive(Clone, Copy)]
struct TextLine {
    index: usize,
    start: usize,
}

/// The rows `line` draws in `window`, at most `room`, with the ghost's later lines under it and the note of what did not
/// fit.
fn rows_of_line<'a>(line: &'a str, place: TextLine, paint: &Paint<'_>, window: Window, room: usize) -> Vec<Line<'a>> {
    let marked = paint.mark.as_ref().and_then(|mark| mark_within_line(mark, place.start, line.len()));
    let spans = paint.colours.get(place.index).map_or(&[][..], Vec::as_slice);
    let ghost = paint.ghost.and_then(|ghost| ghost.within_line(place.start, line.len()));
    let (ghost, below) = ghost.map(Ghost::first_line).unzip();
    let selected = paint.selection.as_ref().and_then(|selection| within_line(selection, place.start, line.len()));
    let skip = if place.index == window.first.line { window.first.row } else { 0 };
    let shown_ghost = ghost.filter(|ghost| !ghost.rest.is_empty());
    let painted = coloured_line(line, spans, Cuts { mark: marked, selected, ghost }, paint);
    let cuts = match shown_ghost {
        Some(ghost) => wrap::ghosted_rows(line, &painted.to_string(), ghost.at, window.columns),
        None => wrap::plain_rows(line, painted.spans.iter().map(|span| span.content.len()).sum(), window.columns),
    };
    let drawn = rows_within(painted, &cuts, skip, room);
    let is_ghost_cut = shown_ghost.is_some_and(|ghost| wrap::ghost_ends_past(&cuts, ghost, skip + room));
    let later = Below { room: room.saturating_sub(drawn.len()), style: Style::default().fg(paint.theme.faded), columns: window.columns };
    with_lines_below(drawn, &below.unwrap_or_default(), usize::from(is_ghost_cut), later)
}

/// `line` cut at `cuts`, past the first `skip` rows and at most `room`, each part in its style.
fn rows_within<'a>(line: Line<'a>, cuts: &[Range<usize>], skip: usize, room: usize) -> Vec<Line<'a>> {
    if cuts.len() == 1 && skip == 0 {
        return vec![line];
    }
    let starts: Vec<usize> = line.spans.iter().scan(0, |end, span| Some(std::mem::replace(end, *end + span.content.len()))).collect();
    cuts.iter().skip(skip).take(room).map(|row| Line::from(spans_within(&line.spans, &starts, row))).collect()
}

/// The parts of `spans`, which start at the bytes `starts`, that fall in the bytes `row` of their joined text.
fn spans_within(spans: &[Span<'_>], starts: &[usize], row: &Range<usize>) -> Vec<Span<'static>> {
    let first = starts.partition_point(|&start| start <= row.start).saturating_sub(1);
    let in_row = spans.iter().zip(starts).skip(first).take_while(|&(_, &start)| start < row.end);
    let part = |(span, &start): (&Span<'_>, &usize)| {
        let bytes = row.start.max(start) - start..row.end.min(start + span.content.len()) - start;
        Span::styled(span.content[bytes].to_owned(), span.style)
    };
    in_row.map(part).collect()
}

/// How the ghost's later lines fit under its first: the rows left, their style, and the pane's width.
#[derive(Clone, Copy)]
struct Below {
    room: usize,
    style: Style,
    columns: usize,
}

/// `rows`, then each of `lines` cut into rows, as many as the room holds; the last row says how many lines did not fit,
/// `already_cut` of them above these.
fn with_lines_below<'a>(mut rows: Vec<Line<'a>>, lines: &[&str], already_cut: usize, below: Below) -> Vec<Line<'a>> {
    let Below { mut room, style, columns } = below;
    let mut more = already_cut;
    for (index, text) in lines.iter().enumerate() {
        if room == 0 {
            more += lines.len() - index;
            break;
        }
        let printable = render::printable(text).into_owned();
        let cuts = wrap::cuts_of(&printable, columns);
        more += usize::from(cuts.len() > room);
        let cut = rows_within(Line::from(Span::styled(printable, style)), &cuts, 0, room);
        room -= cut.len();
        rows.extend(cut);
    }
    if more > 0
        && let Some(last) = rows.pop()
    {
        rows.push(with_note(last, more, style, columns));
    }
    rows
}

/// `row` with the note of `more` lines that did not fit at its end, its own end cut for the note when a pane `columns`
/// wide has no room for both.
fn with_note(row: Line<'_>, more: usize, style: Style, columns: usize) -> Line<'_> {
    let unit = if more == 1 { "line" } else { "lines" };
    let note = format!("  +{more} {unit}");
    let fits = columns == 0 || row.width() + note.len() <= columns;
    let mut fitted = if fits { row } else { text::truncate_line(&row, columns.saturating_sub(note.len())) };
    fitted.push_span(Span::styled(note, style));
    fitted
}

/// The part of `mark` that falls in the line starting at byte `line_start`; an empty mark stays empty, at the line's end.
fn mark_within_line(mark: &Range<usize>, line_start: usize, line_len: usize) -> Option<Range<usize>> {
    if mark.is_empty() {
        let at = mark.start.checked_sub(line_start).filter(|&at| at <= line_len)?;
        return Some(at..at);
    }
    within_line(mark, line_start, line_len)
}

/// The part of `range`, bytes of the whole text, that falls in the line starting at byte `line_start`, in bytes of that
/// line; `None` when none does.
fn within_line(range: &Range<usize>, line_start: usize, line_len: usize) -> Option<Range<usize>> {
    let start = range.start.saturating_sub(line_start);
    let end = range.end.checked_sub(line_start)?.min(line_len);
    (start < end).then_some(start..end)
}

/// What is inserted into one line, at byte positions of that line: the mark, the selection and the ghost.
struct Cuts<'a> {
    mark: Option<Range<usize>>,
    selected: Option<Range<usize>>,
    ghost: Option<Ghost<'a>>,
}

/// The line as runs of one style each, built in one pass over its spans, then the selection reversed, the mark drawn on
/// top and the ghost after the cursor.
fn coloured_line<'a>(line: &'a str, spans: &[Painted], cuts: Cuts<'_>, paint: &Paint<'_>) -> Line<'a> {
    let runs = token_runs(line, spans, paint.theme);
    let runs = match &cuts.selected {
        Some(selected) => runs.iter().flat_map(|run| cut(run, selected, run.style.add_modifier(Modifier::REVERSED))).collect(),
        None => runs,
    };
    let runs = match &cuts.mark {
        Some(mark) => runs.iter().flat_map(|run| cut(run, mark, run.style.patch(paint.mark_style))).collect(),
        None => runs,
    };
    let runs: Vec<Run> = match cuts.ghost {
        Some(ghost) => runs.iter().flat_map(|run| cut(run, &(ghost.at..ghost.at), run.style)).collect(),
        None => runs,
    };
    let runs = runs.into_iter().filter(|run| !run.bytes.is_empty());
    let mut pieces: Vec<Span<'a>> = runs.map(|run| run.span(line)).collect();
    if let Some(ghost) = cuts.ghost {
        let at = pieces.iter().scan(0, |end, piece| {
            *end += piece.content.len();
            Some(*end)
        });
        let index = at.take_while(|&end| end <= ghost.at).count();
        pieces.insert(index, Span::styled(ghost.shown().into_owned(), Style::default().fg(paint.theme.faded)));
    }
    if cuts.mark.is_some_and(|range| range.is_empty()) {
        pieces.push(Span::styled(" ", paint.mark_style));
    }
    Line::from(pieces)
}

/// Bytes of a line drawn in one style.
struct Run {
    bytes: Range<usize>,
    style: Style,
}

impl Run {
    fn span<'a>(&self, line: &'a str) -> Span<'a> {
        Span::styled(&line[self.bytes.clone()], self.style)
    }
}

/// The whole line as runs: each span in its token's colour, and the gaps between them plain.
/// Spans that do not fit the line or its characters are skipped, since colours can be stale.
fn token_runs(line: &str, spans: &[Painted], theme: &Theme) -> Vec<Run> {
    let fits = |painted: &&Painted| {
        painted.bytes.end <= line.len() && line.is_char_boundary(painted.bytes.start) && line.is_char_boundary(painted.bytes.end)
    };
    let mut runs = Vec::with_capacity(2 * spans.len() + 1);
    let mut at = 0;
    for painted in spans.iter().filter(fits) {
        if painted.bytes.start < at {
            continue;
        }
        runs.push(Run { bytes: at..painted.bytes.start, style: Style::default() });
        runs.push(Run { bytes: painted.bytes.clone(), style: Style::default().fg(theme.syntax.colour(painted.token)) });
        at = painted.bytes.end;
    }
    runs.push(Run { bytes: at..line.len(), style: Style::default() });
    runs.retain(|run| !run.bytes.is_empty());
    runs
}

/// `run` split where `range` starts and ends, its part inside drawn in `style`.
fn cut(run: &Run, range: &Range<usize>, style: Style) -> [Run; 3] {
    let clamp = |at: usize| at.clamp(run.bytes.start, run.bytes.end);
    let (start, end) = (clamp(range.start), clamp(range.end));
    [
        Run { bytes: run.bytes.start..start, style: run.style },
        Run { bytes: start..end, style },
        Run { bytes: end..run.bytes.end, style: run.style },
    ]
}

/// `Editor`, then the name the header gives, or else the path of the saved query in the editor.
fn title(app: &App) -> String {
    let sheet = app.sheet();
    let path = sheet.opened.as_ref().map(|query| query.path.to_string());
    let unsaved = if sheet.is_dirty { " *" } else { "" };
    let named = sheet.header.name.clone().or(path);
    named.map_or_else(|| "Editor".to_owned(), |named| format!("Editor · {named}{unsaved}"))
}

fn suggested(app: &App, named: &ConnectionName) -> Span<'static> {
    let is_production = app.connections().iter().any(|known| known.name() == named && known.tier() == Tier::Production);
    if is_production {
        return Span::styled(
            format!(" ⚠ written for production {named} "),
            Style::default().fg(app.theme().danger).add_modifier(Modifier::BOLD),
        );
    }
    Span::styled(format!(" written for {named} "), Style::default().fg(app.theme().muted))
}

/// The connection the header suggests when it is not the one on screen, in a warning style when it is a production
/// one, then the first header line pgbg could not read.
/// The statement still runs on the connection on screen.
fn header_notes(app: &App) -> Line<'static> {
    let sheet = app.sheet();
    let elsewhere = sheet.header.connection.as_ref().filter(|&named| named != sheet.name());
    let suggestion = elsewhere.map(|named| suggested(app, named));
    let warning = sheet.header.warnings.first().map(String::as_str).or_else(|| sheet.sent_chart_warning());
    let warning = warning.map(|warning| Span::styled(format!(" ⚠ {warning} "), Style::default().fg(app.theme().warn)));
    Line::from(suggestion.into_iter().chain(warning).collect::<Vec<_>>())
}

/// Why the last query asked for could not be opened, in `danger`.
fn open_error(app: &App) -> Span<'static> {
    let error = app.sheet().query_error.as_ref().map(|error| format!(" ✗ {error} "));
    Span::styled(error.unwrap_or_default(), Style::default().fg(app.theme().danger))
}

fn badge(app: &App) -> Span<'static> {
    match app.sheet().connection.writes() {
        WritePolicy::ReadOnly => Span::styled(" READ ONLY ", Style::default().fg(app.theme().muted)),
        WritePolicy::Allowed => Span::styled(" WRITES ", Style::default().fg(app.theme().danger).add_modifier(Modifier::BOLD)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::{failed_run, typed, with_focus};
    use super::super::app::idle_ghost::tests::ghosted as idle_ghosted;
    use super::super::styled;
    use super::*;
    use crate::syntax;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::{Backend as _, TestBackend};
    use ratatui::layout::Size;

    const TWO_ROWS: Size = Size { width: 28, height: 2 };
    const EVERY_LINE: Window = Window { first: Row { line: 0, row: 0 }, rows: usize::MAX / 2, columns: 0 };

    /// The lines drawn with each reversed run in brackets.
    fn reversed_runs(text: &str, mark: Option<Range<usize>>, selection: Option<Range<usize>>) -> Vec<String> {
        reversed_rows(text, mark, selection, EVERY_LINE)
    }

    /// The rows of `window` drawn with each reversed run in brackets.
    fn reversed_rows(text: &str, mark: Option<Range<usize>>, selection: Option<Range<usize>>, window: Window) -> Vec<String> {
        let style = Style::default().add_modifier(Modifier::REVERSED);
        let shown = |span: &Span<'_>| if span.style == style { format!("[{}]", span.content) } else { span.content.to_string() };
        let theme = Theme::default();
        let paint = Paint { colours: &[], mark, mark_style: style, selection, ghost: None, theme: &theme };
        coloured(text, &paint, window).lines.iter().map(|line| line.spans.iter().map(shown).collect()).collect()
    }

    fn marked_text(text: &str, mark: Range<usize>) -> Vec<String> {
        reversed_runs(text, Some(mark), None)
    }

    #[test]
    fn the_selection_is_drawn_reversed_across_its_lines() {
        assert_eq!(reversed_runs("SELECT id\nFROM orders", None, Some(7..14)), ["SELECT [id]", "[FROM] orders"]);
    }

    fn painted(text: &str, mark: Option<Range<usize>>) -> String {
        let mark_style = Style::default().fg(Theme::default().danger).add_modifier(Modifier::UNDERLINED);
        let (colours, theme) = (syntax::highlight(text), Theme::default());
        let text = coloured(text, &Paint { colours: &colours, mark, mark_style, selection: None, ghost: None, theme: &theme }, EVERY_LINE);
        let piece = |span: &Span<'_>| format!("{:?}/{:?} {:?}", span.style.fg, span.style.add_modifier, span.content);
        text.lines.iter().map(|line| line.spans.iter().map(piece).collect::<Vec<_>>().join("\n")).collect::<Vec<_>>().join("\n---\n")
    }

    #[test]
    fn the_marked_token_is_drawn_in_the_mark_style() {
        assert_eq!(marked_text("SELECT *\nFROM ordrs\nLIMIT 1", 14..19), ["SELECT *", "FROM [ordrs]", "LIMIT 1"]);
    }

    #[test]
    fn a_token_over_two_lines_is_marked_on_both() {
        assert_eq!(marked_text("SELECT 'a\nb'::int", 7..12), ["SELECT ['a]", "[b']::int"]);
    }

    #[test]
    fn a_selected_token_stays_reversed_under_the_mark() {
        let theme = Theme::default();
        let mark_style = Style::default().fg(theme.danger).add_modifier(Modifier::UNDERLINED);
        let paint = Paint { colours: &[], mark: Some(7..12), mark_style, selection: Some(7..12), ghost: None, theme: &theme };
        let text = coloured("SELECT ordrs", &paint, EVERY_LINE);
        assert_eq!(text.lines[0].spans[1].style, mark_style.add_modifier(Modifier::REVERSED));
    }

    #[test]
    fn a_marked_token_with_multi_byte_characters_is_cut_on_their_edges() {
        assert_eq!(marked_text("SELECT prénom, id", 7..14), ["SELECT [prénom], id"]);
    }

    #[test]
    fn a_mark_at_the_end_of_a_line_or_the_text_draws_a_space() {
        assert_eq!(marked_text("SELECT\nFROM", 6..6), ["SELECT[ ]", "FROM"]);
        assert_eq!(marked_text("SELECT", 6..6), ["SELECT[ ]"]);
    }

    fn ghosted_in(text: &str, at: usize, rest: &str, window: Window) -> Vec<String> {
        let theme = Theme::default();
        let ghost = Some(Ghost { at, rest });
        let shown =
            |span: &Span<'_>| if span.style.fg == Some(theme.faded) { format!("<{}>", span.content) } else { span.content.to_string() };
        let text = coloured(
            text,
            &Paint { colours: &[], mark: None, mark_style: Style::default(), selection: None, ghost, theme: &theme },
            window,
        );
        text.lines.iter().map(|line| line.spans.iter().map(shown).collect()).collect()
    }

    fn ghosted(text: &str, at: usize, rest: &str) -> Vec<String> {
        ghosted_in(text, at, rest, EVERY_LINE)
    }

    #[test]
    fn the_ghost_draws_faded_at_the_cursor_on_its_line_only() {
        assert_eq!(ghosted("SELECT *\nFROM cu\nLIMIT 1", 16, "stomers"), ["SELECT *", "FROM cu<stomers>", "LIMIT 1"]);
        assert_eq!(ghosted("SELECT o. FROM orders o", 9, "id"), ["SELECT o.<id> FROM orders o"]);
    }

    #[test]
    fn a_ghost_draws_its_control_characters_escaped() {
        assert_eq!(ghosted("SELECT ", 7, "'\u{1b}[31m'"), ["SELECT <'\\u{1b}[31m'>"]);
    }

    #[test]
    fn a_ghost_draws_the_control_characters_of_its_later_lines_escaped() {
        assert_eq!(ghosted("SELECT ", 7, "*\nFROM \u{202e}orders"), ["SELECT <*>", "<FROM \\u{202e}orders>"]);
    }

    #[test]
    fn the_lines_of_the_ghost_after_its_first_push_the_text_below_down() {
        assert_eq!(
            ghosted("SELECT *\n\nSELECT 1", 8, " FROM orders\nWHERE paid"),
            ["SELECT *< FROM orders>", "<WHERE paid>", "", "SELECT 1"]
        );
    }

    #[test]
    fn the_last_row_of_a_cut_ghost_says_how_many_lines_did_not_fit() {
        let shown = ghosted_in("SELECT *", 8, " FROM orders\nWHERE paid\nORDER BY id\nLIMIT 10", Window { rows: 2, ..EVERY_LINE });
        assert_eq!(shown, ["SELECT *< FROM orders>", "<WHERE paid><  +2 lines>"]);
    }

    #[test]
    fn a_ghost_on_the_last_row_says_how_many_lines_did_not_fit_on_it() {
        let shown = ghosted_in("SELECT *", 8, " FROM orders\nWHERE paid", Window { rows: 1, ..EVERY_LINE });
        assert_eq!(shown, ["SELECT *< FROM orders><  +1 line>"]);
    }

    #[test]
    fn the_ghost_shows_only_while_the_editor_has_the_keys() {
        let app = with_focus(typed("sel"), Focus::Result);
        let mut terminal = Terminal::new(TestBackend::new(30, 5)).unwrap();
        terminal.draw(|f| _ = draw(f, &app, f.area())).unwrap();
        assert!(!terminal.backend().to_string().contains("select"), "{}", terminal.backend());
    }

    #[test]
    fn the_idle_ghost_draws_faded_after_the_cursor() {
        let mut app = idle_ghosted("SELECT * FROM orders WHERE ", "SELECT * FROM orders WHERE total > 10");
        app.recolour();
        let mut terminal = Terminal::new(TestBackend::new(44, 3)).unwrap();
        terminal.draw(|f| _ = draw(f, &app, f.area())).unwrap();
        insta::assert_snapshot!("idle_ghost", styled::runs(terminal.backend().buffer()));
    }

    fn drawn_idle_ghost(height: u16) -> String {
        let mut app = idle_ghosted("SELECT * ", "SELECT * FROM orders\nWHERE total > 10\nORDER BY total DESC\nLIMIT 10");
        app.recolour();
        let mut terminal = Terminal::new(TestBackend::new(30, height)).unwrap();
        terminal.draw(|f| _ = draw(f, &app, f.area())).unwrap();
        styled::runs(terminal.backend().buffer())
    }

    #[test]
    fn every_line_of_the_idle_ghost_draws_under_the_cursor() {
        insta::assert_snapshot!("idle_ghost_lines", drawn_idle_ghost(6));
    }

    #[test]
    fn an_idle_ghost_taller_than_the_editor_says_how_many_lines_did_not_fit() {
        insta::assert_snapshot!("idle_ghost_cut", drawn_idle_ghost(4));
    }

    #[test]
    fn the_error_mark_draws_over_a_coloured_token() {
        insta::assert_snapshot!("coloured_marked", painted("SELECT id, 'paid', 10 -- recent\nFROM ordrs", Some(37..42)));
    }

    fn drawn(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| _ = draw(f, app, f.area())).unwrap();
        terminal
    }

    #[test]
    fn the_failing_token_is_underlined_in_the_danger_colour() {
        let mut app = failed_run("SELECT id\nFROM ordrs\nLIMIT 1", 16);
        app.recolour();
        insta::assert_snapshot!("error_underline", styled::runs(drawn(&app, 30, 5).backend().buffer()));
    }

    #[test]
    fn a_failing_parameter_is_underlined_whole() {
        let mut app = failed_run("SELECT {{since}}::dat", 8);
        app.recolour();
        insta::assert_snapshot!("error_underline_parameter", styled::runs(drawn(&app, 30, 3).backend().buffer()));
    }

    /// A statement of `lines` lines whose failing token is on the first, the cursor on the last.
    fn failed_far_above_the_cursor(lines: usize) -> App {
        let padding = "\n, id".repeat(lines - 2);
        failed_run(&format!("SELECT ordrs{padding}\nFROM orders"), 8)
    }

    #[test]
    fn a_failing_token_above_the_view_scrolls_into_it_with_the_cursor_left_out() {
        let app = failed_far_above_the_cursor(8);
        let mut terminal = drawn(&app, 30, 4);
        assert_eq!(terminal.backend_mut().get_cursor_position().unwrap(), Position { x: 0, y: 0 });
        insta::assert_snapshot!("error_underline_scrolled", styled::runs(terminal.backend().buffer()));
    }

    #[test]
    fn revealing_the_failing_token_leaves_the_cursor_where_it_was() {
        let text = "SELECT ordrs\n, id\n, total\nFROM orders";
        let app = failed_run(text, 8);
        assert_eq!(app.sheet().editor.split().0, text);
    }

    #[test]
    fn a_failing_token_in_view_with_the_cursor_keeps_both_and_the_scroll() {
        let app = failed_run("SELECT ordrs\nFROM orders", 8);
        let mut terminal = drawn(&app, 30, 4);
        assert_eq!(terminal.backend_mut().get_cursor_position().unwrap(), Position { x: 13, y: 2 });
        assert_eq!(app.sheet().editor.first_row_in_view(TWO_ROWS).line, 0);
    }

    #[test]
    fn moving_the_cursor_scrolls_back_to_it_and_keeps_the_mark() {
        let mut app = failed_far_above_the_cursor(8);
        assert_eq!(app.sheet().editor.first_row_in_view(TWO_ROWS).line, 0);
        app.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(app.sheet().editor.first_row_in_view(TWO_ROWS).line, 6);
        assert_eq!(app.sheet().error_mark, Some(7..12));
    }

    fn cursor(terminal: &mut Terminal<TestBackend>) -> Position {
        terminal.backend_mut().get_cursor_position().unwrap()
    }

    /// The rows inside the frame and its padding; the second cell of a wide character shows as a space.
    fn rows_drawn(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        let row = |y: u16| (2..buffer.area.width - 2).map(|x| buffer[(x, y)].symbol()).collect::<String>().trim_end().to_owned();
        (1..buffer.area.height - 1).map(row).collect()
    }

    #[test]
    fn a_line_wider_than_the_pane_goes_on_to_the_next_row() {
        let app = typed("SELECT id FROM orders WHERE total > 10");
        let mut terminal = drawn(&app, 24, 5);
        assert_eq!(rows_drawn(&terminal), ["SELECT id FROM", "orders WHERE total", "> 10"]);
        assert_eq!(cursor(&mut terminal), Position { x: 6, y: 3 });
    }

    #[test]
    fn the_popup_finds_the_cursor_on_a_wrapped_row() {
        let app = typed("SELECT id FROM orders WHERE total > 10");
        let mut terminal = drawn(&app, 24, 5);
        assert_eq!(cursor_at(&app, Rect::new(2, 1, 20, 3)), cursor(&mut terminal));
    }

    #[test]
    fn wrapping_changes_no_text() {
        let text = "SELECT id FROM orders WHERE total > 10";
        let app = typed(text);
        drawn(&app, 12, 6);
        assert_eq!(app.sheet().editor.text(), text);
    }

    #[test]
    fn a_cursor_after_the_last_cell_of_a_row_is_at_the_start_of_the_next() {
        let mut terminal = drawn(&typed("SELECT id FROM order"), 24, 5);
        assert_eq!(rows_drawn(&terminal), ["SELECT id FROM order", "", ""]);
        assert_eq!(cursor(&mut terminal), Position { x: 2, y: 2 });
    }

    #[test]
    fn a_cursor_on_a_later_row_counts_a_decomposed_letter_as_it_draws() {
        let mut terminal = drawn(&typed("SELECT 'Zoe\u{308}' AS nina"), 16, 5);
        assert_eq!(rows_drawn(&terminal), ["SELECT", "'Zoe\u{308}' AS", "nina"]);
        assert_eq!(cursor(&mut terminal), Position { x: 6, y: 3 });
    }

    #[test]
    fn a_wide_word_moves_to_the_next_row_and_the_cursor_follows() {
        let mut terminal = drawn(&typed("SELECT '日本語'"), 14, 5);
        assert_eq!(rows_drawn(&terminal), ["SELECT", "'日 本 語 '", ""]);
        assert_eq!(cursor(&mut terminal), Position { x: 10, y: 2 });
    }

    #[test]
    fn a_wide_character_wider_than_the_room_left_in_a_long_word_starts_the_next_row() {
        let mut terminal = drawn(&typed("'abcdefghi日本'"), 14, 5);
        assert_eq!(rows_drawn(&terminal), ["'abcdefghi", "日 本 '", ""]);
        assert_eq!(cursor(&mut terminal), Position { x: 7, y: 2 });
    }

    #[test]
    fn typing_at_the_end_of_a_full_row_puts_the_cursor_on_the_empty_row() {
        let mut terminal = drawn(&typed("SELECT id FROM nin"), 22, 5);
        assert_eq!(rows_drawn(&terminal), ["SELECT id FROM nin", "", ""]);
        assert_eq!(cursor(&mut terminal), Position { x: 2, y: 2 });
    }

    #[test]
    fn a_word_that_outgrows_its_row_moves_down_whole_with_the_cursor_after_it() {
        let mut app = typed("SELECT id FROM nin");
        app.handle_key(KeyEvent::from(KeyCode::Char('a')));
        let mut terminal = drawn(&app, 22, 5);
        assert_eq!(rows_drawn(&terminal), ["SELECT id FROM", "nina", ""]);
        assert_eq!(cursor(&mut terminal), Position { x: 6, y: 2 });
    }

    /// `SELECT id FROM nina` drawn over two rows, after `lefts` presses of `←`.
    fn cursor_moved_left(lefts: usize) -> Position {
        let mut app = typed("SELECT id FROM nina");
        (0..lefts).for_each(|_| _ = app.handle_key(KeyEvent::from(KeyCode::Left)));
        cursor(&mut drawn(&app, 22, 5))
    }

    #[test]
    fn a_cursor_on_the_whitespace_at_a_break_is_drawn_on_its_cell() {
        assert_eq!(cursor_moved_left(5), Position { x: 16, y: 1 });
    }

    #[test]
    fn a_cursor_after_the_whitespace_at_a_break_is_drawn_at_the_start_of_the_next_row() {
        assert_eq!(cursor_moved_left(4), Position { x: 2, y: 2 });
    }

    #[test]
    fn the_rows_above_a_long_line_scroll_away_to_keep_the_cursor() {
        let mut terminal = drawn(&typed(&"orders ".repeat(10)), 14, 4);
        assert_eq!(rows_drawn(&terminal), ["orders", "orders"]);
        assert_eq!(cursor(&mut terminal), Position { x: 9, y: 2 });
    }

    #[test]
    fn a_selection_stays_reversed_on_both_rows_it_crosses() {
        assert_eq!(reversed_rows("customers", None, Some(4..8), Window { columns: 6, ..EVERY_LINE }), ["cust[om]", "[er]s"]);
    }

    #[test]
    fn a_selection_over_a_break_covers_its_whitespace() {
        assert_eq!(reversed_rows("FROM orders", None, Some(3..7), Window { columns: 8, ..EVERY_LINE }), ["FRO[M ]", "[or]ders"]);
    }

    #[test]
    fn a_full_line_keeps_an_empty_row_above_the_next_line() {
        assert_eq!(reversed_rows("orders\nid", None, None, Window { columns: 6, ..EVERY_LINE }), ["orders", "", "id"]);
    }

    #[test]
    fn the_space_of_a_mark_at_the_end_of_a_line_adds_no_row_when_it_fits() {
        assert_eq!(reversed_rows("order\nid", Some(5..5), None, Window { columns: 6, ..EVERY_LINE }), ["order[ ]", "id"]);
    }

    #[test]
    fn only_the_rows_of_the_window_are_drawn() {
        let window = Window { first: Row { line: 0, row: 1 }, rows: 1, columns: 4 };
        assert_eq!(reversed_rows("customers\nid", None, None, window), ["omer"]);
    }

    #[test]
    fn the_mark_is_drawn_on_both_rows_its_token_crosses_and_the_whitespace_between() {
        let mut app = failed_run("SELECT 'paid orders' AS nina", 8);
        app.recolour();
        let terminal = drawn(&app, 16, 6);
        let buffer = terminal.backend().buffer();
        assert_eq!(rows_drawn(&terminal), ["SELECT", "'paid", "orders' AS", "nina"]);
        assert!(buffer[(2, 2)].modifier.contains(Modifier::UNDERLINED));
        assert!(buffer[(7, 2)].modifier.contains(Modifier::UNDERLINED));
        assert!(buffer[(2, 3)].modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn the_mark_is_drawn_on_both_rows_of_a_word_cut_where_the_pane_ends() {
        let mut app = failed_run("SELECT customers_orders_total", 8);
        app.recolour();
        let terminal = drawn(&app, 16, 5);
        let buffer = terminal.backend().buffer();
        assert_eq!(rows_drawn(&terminal), ["SELECT", "customers_or", "ders_total"]);
        assert!(buffer[(13, 2)].modifier.contains(Modifier::UNDERLINED));
        assert!(buffer[(2, 3)].modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn a_failing_token_on_a_row_above_the_view_scrolls_into_it_with_the_cursor_left_out() {
        let mut terminal = drawn(&failed_run("SELECT ordrs, id, total, nina FROM orders", 8), 14, 3);
        assert_eq!(rows_drawn(&terminal), ["ordrs,"]);
        assert_eq!(cursor(&mut terminal), Position { x: 0, y: 0 });
    }

    #[test]
    fn a_ghost_goes_on_to_the_next_row() {
        let shown = ghosted_in("SELECT * FROM cu", 16, "stomers", Window { columns: 18, ..EVERY_LINE });
        assert_eq!(shown, ["SELECT * FROM cu<st>", "<omers>"]);
    }

    #[test]
    fn a_ghost_with_a_space_past_the_pane_keeps_the_word_it_completes_on_the_cursors_row() {
        let shown = ghosted_in("FROM cu", 7, "stomers AND nina", Window { columns: 14, ..EVERY_LINE });
        assert_eq!(shown, ["FROM cu<stomers>", "< AND nina>"]);
    }

    #[test]
    fn a_ghost_inside_a_word_that_moved_down_stays_at_the_cursor() {
        let shown = ghosted_in("FROM custom", 7, "x", Window { columns: 8, ..EVERY_LINE });
        assert_eq!(shown, ["FROM ", "cu<x>stom"]);
    }

    #[test]
    fn the_later_lines_of_a_ghost_wrap_and_count_their_rows() {
        let shown = ghosted_in("id", 2, " x\ncustomers_total_id\nid", Window { rows: 3, columns: 14, ..EVERY_LINE });
        assert_eq!(shown, ["id< x>", "<customers_tota>", "<l_id><  +1 line>"]);
    }

    #[test]
    fn the_note_replaces_a_row_too_narrow_for_both() {
        let shown = ghosted_in("id", 2, " x\ncustomers\nid", Window { rows: 3, columns: 5, ..EVERY_LINE });
        assert_eq!(shown, ["id< x>", "<custo>", "<  +1 line>"]);
    }

    #[test]
    fn a_later_line_of_a_ghost_that_fills_its_row_adds_no_empty_row() {
        let shown = ghosted_in("id", 2, " x\nabcde\nf", Window { columns: 5, ..EVERY_LINE });
        assert_eq!(shown, ["id< x>", "<abcde>", "<f>"]);
    }

    #[test]
    fn a_ghost_that_overflows_after_a_whitespace_starts_at_the_cursor_on_its_row() {
        let mut app = idle_ghosted("SELECT id FROM ", "SELECT id FROM orders");
        app.recolour();
        let mut terminal = drawn(&app, 20, 4);
        assert_eq!(rows_drawn(&terminal), ["SELECT id FROM o", "rders"]);
        assert_eq!(cursor(&mut terminal), Position { x: 17, y: 1 });
    }

    #[test]
    fn the_note_of_a_cut_ghost_shows_on_a_full_row() {
        let shown = ghosted_in("id", 2, " x\nabcdefghijklmn\nf", Window { rows: 2, columns: 14, ..EVERY_LINE });
        assert_eq!(shown, ["id< x>", "<abcd…><  +1 line>"]);
    }

    #[test]
    fn a_one_line_ghost_past_the_last_row_says_so() {
        let rest = "x".repeat(40);
        let shown = ghosted_in("id", 2, &rest, Window { rows: 2, columns: 20, ..EVERY_LINE });
        assert_eq!(shown, [format!("id<{}>", "x".repeat(18)), format!("<{}…><  +1 line>", "x".repeat(10))]);
    }

    #[test]
    fn a_one_line_ghost_past_the_last_row_counts_the_lines_after_it_too() {
        let rest = format!("{}\nLIMIT 1", "x".repeat(40));
        let shown = ghosted_in("id", 2, &rest, Window { rows: 1, columns: 20, ..EVERY_LINE });
        assert_eq!(shown, [format!("id<{}…><  +2 lines>", "x".repeat(7))]);
    }

    #[test]
    fn the_text_after_a_ghost_past_the_last_row_needs_no_note() {
        let long = format!("id {}", "z".repeat(40));
        let shown = ghosted_in(&long, 2, "x", Window { rows: 1, columns: 20, ..EVERY_LINE });
        assert_eq!(shown, ["id<x> "]);
    }

    #[test]
    fn a_ghost_that_wraps_leaves_the_cursor_where_it_was() {
        let mut app = idle_ghosted("SELECT * ", "SELECT * FROM orders WHERE total > 10 ORDER BY total DESC");
        app.recolour();
        let mut terminal = drawn(&app, 16, 4);
        assert_eq!(cursor(&mut terminal), Position { x: 11, y: 1 });
    }

    #[test]
    fn the_cursor_stays_in_view_past_65_536_lines() {
        let mut app = typed("");
        let text: String = (1..=70_000).map(|line| format!("{line}\n")).collect();
        app.paste(&(text + "last"));
        let mut terminal = Terminal::new(TestBackend::new(30, 5)).unwrap();
        terminal.draw(|f| _ = draw(f, &app, f.area())).unwrap();
        assert_eq!(terminal.backend_mut().get_cursor_position().unwrap(), Position { x: 6, y: 3 });
        assert!(terminal.backend().to_string().contains("last"), "{}", terminal.backend());
    }

    #[test]
    fn the_cursor_follows_the_text_and_the_editor_scrolls_to_keep_it() {
        let mut app = typed("SELECT");
        for _ in 0..4 {
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        }
        app.handle_key(KeyEvent::from(KeyCode::Char('1')));
        let mut terminal = Terminal::new(TestBackend::new(30, 5)).unwrap();
        terminal.draw(|f| _ = draw(f, &app, f.area())).unwrap();
        assert_eq!(terminal.backend_mut().get_cursor_position().unwrap(), Position { x: 3, y: 3 });
        insta::assert_snapshot!("editor_scrolled", terminal.backend());
    }
}
