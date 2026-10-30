//! The keys under a popup, `⏎ opens · esc closes`, cut to whole hints that fit its width.
use crate::act::{self, Hint, Pane};
use ratatui::layout::Rect;

const INDENT: &str = "  ";
const SEPARATOR: &str = " · ";

/// The footer of a palette over `screen` while `pane` has the keys: `  ↑↓ pick · ⏎ opens · esc closes`.
pub(super) fn text(pane: Pane, screen: Rect) -> String {
    format!("{INDENT}{}", fitted(pane, popup_inner_width(screen).saturating_sub(INDENT.len())))
}

/// The hints of `pane` that fit in `columns`, joined by ` · `, the way out kept longest.
pub(super) fn fitted(pane: Pane, columns: usize) -> String {
    let room = columns + SEPARATOR.chars().count();
    let width = |hint: Hint| written(hint).chars().count() + SEPARATOR.chars().count();
    act::footer_within(pane, room, width).into_iter().map(written).collect::<Vec<_>>().join(SEPARATOR)
}

fn written(hint: Hint) -> String {
    format!("{} {}", hint.keys, hint.does)
}

/// The columns a line has inside the frame `vtui::palette::view::draw` opens over `screen`: three fifths of it, 40 at
/// least, less a border and a column of padding on each side.
fn popup_inner_width(screen: Rect) -> usize {
    let width = (screen.width * 3 / 5).clamp(40.min(screen.width), screen.width);
    usize::from(width.saturating_sub(4))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(width: u16) -> Rect {
        Rect { x: 0, y: 0, width, height: 16 }
    }

    #[test]
    fn a_wide_palette_shows_every_hint() {
        assert_eq!(text(Pane::TableFinder, screen(120)), "  ⏎ opens · ^r preview · ⇥ insert · ↑↓ pick · esc closes");
    }

    #[test]
    fn a_narrow_palette_keeps_esc_and_drops_the_last_hints_whole() {
        assert_eq!(text(Pane::TableFinder, screen(60)), "  ⏎ opens · ^r preview · esc closes");
    }
}
