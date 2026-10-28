//! Where the last frame drew each pane, so a click or `⇧` and an arrow find the pane they mean.
use super::Focus;
use ratatui::layout::{Position, Rect};

/// Where each pane was drawn in the last frame; a pane left out has no area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) struct Panes {
    pub(in crate::tui) sidebar: Option<Rect>,
    pub(in crate::tui) params: Option<Rect>,
    pub(in crate::tui) editor: Rect,
    pub(in crate::tui) result: Rect,
}

/// A side of the pane that has the keys, where `⇧` and an arrow look for the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Side {
    Left,
    Right,
    Above,
    Below,
}

impl Panes {
    /// The pane drawn at `at`.
    pub(super) fn under(self, at: Position) -> Option<Focus> {
        self.drawn().find_map(|(focus, area)| area.contains(at).then_some(focus))
    }

    /// The nearest pane on `side` of `from` that shares rows or columns with it; on a tie, the first in `tab` order.
    pub(super) fn toward(self, from: Focus, side: Side) -> Option<Focus> {
        let here = self.area(from)?;
        let beyond = self.drawn().filter(|&(_, there)| side.is_beyond(here, there));
        beyond.min_by_key(|&(_, there)| side.gap(here, there)).map(|(focus, _)| focus)
    }

    pub(super) const fn has_sidebar(self) -> bool {
        self.sidebar.is_some()
    }

    const fn area(self, focus: Focus) -> Option<Rect> {
        match focus {
            Focus::Editor => Some(self.editor),
            Focus::Params => self.params,
            Focus::Result => Some(self.result),
            Focus::Queries => self.sidebar,
        }
    }

    /// The panes drawn, in `tab` order.
    fn drawn(self) -> impl Iterator<Item = (Focus, Rect)> {
        Focus::ORDER.into_iter().filter_map(move |focus| self.area(focus).map(|area| (focus, area)))
    }
}

impl Side {
    /// Whether `there` lies past this side of `here`, on rows or columns they share.
    const fn is_beyond(self, here: Rect, there: Rect) -> bool {
        let share_rows = there.top() < here.bottom() && here.top() < there.bottom();
        let share_columns = there.left() < here.right() && here.left() < there.right();
        match self {
            Self::Left => share_rows && there.right() <= here.left(),
            Self::Right => share_rows && there.left() >= here.right(),
            Self::Above => share_columns && there.bottom() <= here.top(),
            Self::Below => share_columns && there.top() >= here.bottom(),
        }
    }

    /// The cells between `here` and `there`, a pane past this side of it.
    const fn gap(self, here: Rect, there: Rect) -> u16 {
        match self {
            Self::Left => here.left().saturating_sub(there.right()),
            Self::Right => there.left().saturating_sub(here.right()),
            Self::Above => here.top().saturating_sub(there.bottom()),
            Self::Below => there.top().saturating_sub(here.bottom()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::every_pane;
    use super::*;

    #[test]
    fn right_of_the_sidebar_is_the_editor_first_in_tab_order() {
        assert_eq!(every_pane().toward(Focus::Queries, Side::Right), Some(Focus::Editor));
    }

    #[test]
    fn left_of_any_stacked_pane_is_the_sidebar() {
        for from in [Focus::Params, Focus::Editor, Focus::Result] {
            assert_eq!(every_pane().toward(from, Side::Left), Some(Focus::Queries), "{from:?}");
        }
    }

    #[test]
    fn above_the_result_is_the_nearest_pane_the_editor() {
        assert_eq!(every_pane().toward(Focus::Result, Side::Above), Some(Focus::Editor));
    }

    #[test]
    fn above_the_editor_is_the_bar_when_drawn_else_nothing() {
        assert_eq!(every_pane().toward(Focus::Editor, Side::Above), Some(Focus::Params));
        assert_eq!(Panes { params: None, ..every_pane() }.toward(Focus::Editor, Side::Above), None);
    }

    #[test]
    fn below_the_editor_is_the_result() {
        assert_eq!(every_pane().toward(Focus::Editor, Side::Below), Some(Focus::Result));
    }

    #[test]
    fn past_the_edge_of_the_screen_is_nothing() {
        assert_eq!(every_pane().toward(Focus::Queries, Side::Left), None);
        assert_eq!(every_pane().toward(Focus::Result, Side::Below), None);
    }

    #[test]
    fn a_click_finds_the_pane_under_it() {
        assert_eq!(every_pane().under(Position::new(40, 12)), Some(Focus::Result));
        assert_eq!(every_pane().under(Position::new(5, 3)), Some(Focus::Queries));
        assert_eq!(Panes::default().under(Position::new(5, 3)), None);
    }
}
