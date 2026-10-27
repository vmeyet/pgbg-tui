//! The SQL editor pane: the text in its syntax colours, the cursor kept in view, the server's error position, and whether the connection takes writes.
use super::app::{App, Focus};
use crate::connection::{ConnectionName, Tier, WritePolicy};
use crate::syntax::{Painted, Spans};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use std::ops::Range;
use vtui::pane;
use vtui::theme::Theme;

/// The editor in `area`, scrolled so the cursor's line shows, with the terminal's cursor on it when it has the keys.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, area: Rect) {
    let focused = app.focus() == Focus::Editor;
    let block = pane::framed(app.theme(), &title(app), focused).title(badge(app)).title(header_notes(app)).title(open_error(app));
    let inside = block.inner(area);
    f.render_widget(block, area);
    let sheet = app.sheet();
    let (before, _, _) = sheet.editor.split();
    let Place { row, column, scroll } = place(app, inside);
    let mark = Style::default().fg(app.theme().danger).add_modifier(Modifier::REVERSED);
    let window = Window { first: scroll, rows: usize::from(inside.height) };
    let ghost = sheet.ghost.as_ref().filter(|_| focused).map(|ghost| Ghost { at: before.len(), rest: &ghost.rest });
    let text = coloured(
        sheet.editor.text(),
        Paint { colours: sheet.editor.colours(), mark: sheet.error_mark, mark_style: mark, ghost, theme: app.theme() },
        window,
    );
    f.render_widget(Paragraph::new(text), inside);
    if !focused {
        return;
    }
    f.set_cursor_position(cursor_position(inside, column, row - scroll));
}

/// The line and column of the cursor in the text, and the first line in view of `inside`, so the cursor's line shows.
struct Place {
    row: usize,
    column: usize,
    scroll: usize,
}

fn place(app: &App, inside: Rect) -> Place {
    let (before, _, _) = app.sheet().editor.split();
    let row = before.matches('\n').count();
    let column = Line::from(before.rsplit('\n').next().unwrap_or_default()).width();
    Place { row, column, scroll: row.saturating_sub(usize::from(inside.height.saturating_sub(1))) }
}

/// Where the terminal's cursor stands in the editor pane drawn in `area`.
pub(super) fn cursor_at(app: &App, area: Rect) -> Position {
    let inside = Block::bordered().inner(area);
    let Place { row, column, scroll } = place(app, inside);
    cursor_position(inside, column, row - scroll)
}

/// Where the cursor at `column` of the `row`th line in view lands, kept inside `inside`.
fn cursor_position(inside: Rect, column: usize, row: usize) -> Position {
    let offset = |at: usize| u16::try_from(at).unwrap_or(u16::MAX);
    let x = inside.x.saturating_add(offset(column)).min(inside.right().saturating_sub(1));
    Position { x, y: inside.y.saturating_add(offset(row)) }
}

/// The lines of the editor in view.
#[derive(Clone, Copy, Debug)]
struct Window {
    first: usize,
    rows: usize,
}

/// How the text is painted: its colours per line, the character at byte `mark` in `mark_style` on top, and the ghost.
#[derive(Clone, Copy)]
struct Paint<'a> {
    colours: &'a [Spans],
    mark: Option<usize>,
    mark_style: Style,
    ghost: Option<Ghost<'a>>,
    theme: &'a Theme,
}

/// The rest of a name, drawn faded at byte `at` of the text, where the cursor is.
#[derive(Clone, Copy)]
struct Ghost<'a> {
    at: usize,
    rest: &'a str,
}

/// The lines of `text` in `window`, each token in its colour, the mark on top; a mark on a line's end draws a space there.
fn coloured<'a>(text: &'a str, paint: Paint<'_>, window: Window) -> Text<'a> {
    let mut line_start = 0;
    let mut lines = Vec::new();
    for (index, line) in text.split('\n').enumerate().take(window.first + window.rows) {
        if index >= window.first {
            let marked =
                paint.mark.and_then(|at| at.checked_sub(line_start)).filter(|&at| at <= line.len()).map(|at| marked_range(line, at));
            let spans = paint.colours.get(index).map_or(&[][..], Vec::as_slice);
            let ghost = paint
                .ghost
                .and_then(|ghost| Some(Ghost { at: ghost.at.checked_sub(line_start)?, ..ghost }))
                .filter(|ghost| ghost.at <= line.len());
            lines.push(coloured_line(line, spans, Cuts { mark: marked, ghost }, paint));
        }
        line_start += line.len() + 1;
    }
    Text::from(lines)
}

/// The bytes of the character at `at`; empty at the line's end.
fn marked_range(line: &str, at: usize) -> Range<usize> {
    at..at + line[at..].chars().next().map_or(0, char::len_utf8)
}

/// What is cut into one line: the mark, and the ghost, by bytes of that line.
struct Cuts<'a> {
    mark: Option<Range<usize>>,
    ghost: Option<Ghost<'a>>,
}

/// The line as runs of one style each, in one pass over its spans, then the mark cut in on top and the ghost in after the
/// cursor.
fn coloured_line<'a>(line: &'a str, spans: &[Painted], cuts: Cuts<'_>, paint: Paint<'_>) -> Line<'a> {
    let runs = token_runs(line, spans, paint.theme);
    let runs = match &cuts.mark {
        Some(mark) => runs.iter().flat_map(|run| cut_by_mark(run, mark, paint.mark_style)).collect(),
        None => runs,
    };
    let runs: Vec<Run> = match cuts.ghost {
        Some(ghost) => runs.iter().flat_map(|run| cut_by_mark(run, &(ghost.at..ghost.at), run.style)).collect(),
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
        pieces.insert(index, Span::styled(ghost.rest.to_owned(), Style::default().fg(paint.theme.faded)));
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

/// The whole line as runs: each span in its token's colour, the gaps between them plain; spans that do not fit the
/// line or its characters are skipped, as stale colours can be.
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

/// `run` split where `mark` starts and ends, the marked part in `mark_style`.
fn cut_by_mark(run: &Run, mark: &Range<usize>, mark_style: Style) -> [Run; 3] {
    let clamp = |at: usize| at.clamp(run.bytes.start, run.bytes.end);
    let (start, end) = (clamp(mark.start), clamp(mark.end));
    [
        Run { bytes: run.bytes.start..start, style: run.style },
        Run { bytes: start..end, style: mark_style },
        Run { bytes: end..run.bytes.end, style: run.style },
    ]
}

/// `Editor`, then the name the header gives, or else the path of the saved query in it.
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

/// The connection the header suggests when it is not the one on screen, loud when it is a production one, then the
/// first header line pgbg could not read. The statement still runs on the connection on screen.
fn header_notes(app: &App) -> Line<'static> {
    let sheet = app.sheet();
    let elsewhere = sheet.header.connection.as_ref().filter(|&named| named != sheet.name());
    let suggestion = elsewhere.map(|named| suggested(app, named));
    let warning =
        sheet.header.warnings.first().map(|warning| Span::styled(format!(" ⚠ {warning} "), Style::default().fg(app.theme().warn)));
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
    use super::super::app::tests::{typed, with_focus};
    use super::*;
    use crate::syntax;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::{Backend as _, TestBackend};

    const EVERY_LINE: Window = Window { first: 0, rows: usize::MAX / 2 };

    fn marked_text(text: &str, mark: usize) -> Vec<String> {
        let style = Style::default().add_modifier(Modifier::REVERSED);
        let shown = |span: &Span<'_>| if span.style == style { format!("[{}]", span.content) } else { span.content.to_string() };
        let theme = Theme::default();
        let text = coloured(text, Paint { colours: &[], mark: Some(mark), mark_style: style, ghost: None, theme: &theme }, EVERY_LINE);
        text.lines.iter().map(|line| line.spans.iter().map(shown).collect()).collect()
    }

    fn painted(text: &str, mark: Option<usize>) -> String {
        let mark_style = Style::default().fg(Theme::default().danger).add_modifier(Modifier::REVERSED);
        let (colours, theme) = (syntax::highlight(text), Theme::default());
        let text = coloured(text, Paint { colours: &colours, mark, mark_style, ghost: None, theme: &theme }, EVERY_LINE);
        let piece = |span: &Span<'_>| format!("{:?}/{:?} {:?}", span.style.fg, span.style.add_modifier, span.content);
        text.lines.iter().map(|line| line.spans.iter().map(piece).collect::<Vec<_>>().join("\n")).collect::<Vec<_>>().join("\n---\n")
    }

    #[test]
    fn the_marked_character_is_drawn_in_the_mark_style() {
        assert_eq!(marked_text("SELECT *\nFROM ordrs\nLIMIT 1", 14), ["SELECT *", "FROM [o]rdrs", "LIMIT 1"]);
    }

    #[test]
    fn a_mark_at_the_end_of_a_line_or_the_text_draws_a_space() {
        assert_eq!(marked_text("SELECT\nFROM", 6), ["SELECT[ ]", "FROM"]);
        assert_eq!(marked_text("SELECT", 6), ["SELECT[ ]"]);
    }

    fn ghosted(text: &str, at: usize, rest: &str) -> Vec<String> {
        let theme = Theme::default();
        let ghost = Some(Ghost { at, rest });
        let shown =
            |span: &Span<'_>| if span.style.fg == Some(theme.faded) { format!("<{}>", span.content) } else { span.content.to_string() };
        let text = coloured(text, Paint { colours: &[], mark: None, mark_style: Style::default(), ghost, theme: &theme }, EVERY_LINE);
        text.lines.iter().map(|line| line.spans.iter().map(shown).collect()).collect()
    }

    #[test]
    fn the_ghost_draws_faded_at_the_cursor_on_its_line_only() {
        assert_eq!(ghosted("SELECT *\nFROM cu\nLIMIT 1", 16, "stomers"), ["SELECT *", "FROM cu<stomers>", "LIMIT 1"]);
        assert_eq!(ghosted("SELECT o. FROM orders o", 9, "id"), ["SELECT o.<id> FROM orders o"]);
    }

    #[test]
    fn the_ghost_shows_only_while_the_editor_has_the_keys() {
        let app = with_focus(typed("sel"), Focus::Result);
        let mut terminal = Terminal::new(TestBackend::new(30, 5)).unwrap();
        terminal.draw(|f| draw(f, &app, f.area())).unwrap();
        assert!(!terminal.backend().to_string().contains("select"), "{}", terminal.backend());
    }

    #[test]
    fn the_error_mark_draws_over_a_coloured_token() {
        insta::assert_snapshot!("coloured_marked", painted("SELECT id, 'paid', 10 -- recent\nFROM ordrs", Some(37)));
    }

    #[test]
    fn the_cursor_stays_in_view_past_65_536_lines() {
        let mut app = typed("");
        let text: String = (1..=70_000).map(|line| format!("{line}\n")).collect();
        app.paste(&(text + "last"));
        let mut terminal = Terminal::new(TestBackend::new(30, 5)).unwrap();
        terminal.draw(|f| draw(f, &app, f.area())).unwrap();
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
        terminal.draw(|f| draw(f, &app, f.area())).unwrap();
        assert_eq!(terminal.backend_mut().get_cursor_position().unwrap(), Position { x: 3, y: 3 });
        insta::assert_snapshot!("editor_scrolled", terminal.backend());
    }
}
