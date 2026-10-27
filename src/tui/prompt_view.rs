//! The prompt on the footer line: the path of a new query as it is typed, whether to open a query over unsaved text, or
//! whether to send a write.
use super::app::{App, Prompt};
use crate::act;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// The prompt over the footer in `area`, its keys after it.
pub(super) fn draw(f: &mut Frame<'_>, app: &App, prompt: &Prompt, area: Rect) {
    let theme = app.theme();
    let mut spans = match prompt {
        Prompt::SaveAs { path, error } => {
            let (before, under, after) = path.split();
            let mut spans = vec![
                Span::styled(" save as › ", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
                Span::raw(format!("{before}{under}{after}")),
                Span::styled("▌ ", Style::default().fg(theme.accent)),
            ];
            spans.extend(error.iter().map(|error| Span::styled(format!("{error} "), Style::default().fg(theme.danger))));
            spans
        }
        Prompt::OpenOverUnsaved(query) => {
            vec![Span::styled(format!(" the editor's text is not saved: open {} over it? ", query.path), Style::default().fg(theme.warn))]
        }
        Prompt::SendWrite(waiting) => {
            vec![Span::styled(format!(" {}? ", waiting.write), Style::default().fg(theme.danger).add_modifier(Modifier::BOLD))]
        }
    };
    let hints =
        act::footer(prompt.pane()).map(|hint| Span::styled(format!(" {} {} ", hint.keys, hint.does), Style::default().fg(theme.muted)));
    spans.extend(hints);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
