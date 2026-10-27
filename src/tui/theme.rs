//! pgbg's colours on top of vtui's palette: the tiers, so a production connection never looks like a local one, and
//! the production colour, so a production frame never reads as an error drawn in `danger`.
//! vtui's tokens read through `Deref`, so a view writes `theme.danger` and `theme.production` alike.
use crate::connection::Tier;
use ratatui::style::Color;
use std::ops::Deref;
use vtui::theme::mix;

/// Where production leans from `danger`, so it stays red enough to warn and reads apart from an error.
const MAGENTA: u32 = 0xff00ff;
/// How far production leans toward [`MAGENTA`].
const PRODUCTION_PCT: u32 = 45;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::tui) struct Theme {
    ui: vtui::theme::Theme,
    /// The frame and the dot of a production connection. A palette of ANSI names cannot mix, so it takes magenta.
    pub(in crate::tui) production: Color,
}

impl Deref for Theme {
    type Target = vtui::theme::Theme;

    fn deref(&self) -> &Self::Target {
        &self.ui
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::over(vtui::theme::Theme::default())
    }
}

impl Theme {
    /// A palette that does not know its ground takes the terminal's; one with an RGB ground keeps its own.
    pub(super) fn with_ground(self, ground: u32) -> Self {
        Self::over(self.ui.with_ground(ground))
    }

    /// Local green, staging amber, production in its own colour.
    pub(super) fn tier(&self, tier: Tier) -> Color {
        match tier {
            Tier::Local => self.success,
            Tier::Staging => self.warn,
            Tier::Production => self.production,
        }
    }

    fn over(ui: vtui::theme::Theme) -> Self {
        let production = hex(ui.danger).map_or(Color::Magenta, |danger| mix(danger, MAGENTA, PRODUCTION_PCT));
        Self { ui, production }
    }
}

/// `0xRRGGBB` of an RGB colour; `None` for an ANSI name, whose value only the terminal knows.
fn hex(colour: Color) -> Option<u32> {
    match colour {
        Color::Rgb(r, g, b) => Some((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_palette() -> impl Iterator<Item = Theme> {
        vtui::theme::Theme::NAMES.into_iter().map(|name| Theme::over(vtui::theme::Theme::named(name).unwrap()))
    }

    #[test]
    fn production_never_takes_the_colour_of_an_error() {
        for theme in every_palette() {
            assert_ne!(theme.production, theme.danger, "{}", theme.name);
        }
    }

    #[test]
    fn production_never_takes_the_colour_of_another_tier() {
        for theme in every_palette() {
            assert_ne!(theme.tier(Tier::Production), theme.tier(Tier::Staging), "{}", theme.name);
            assert_ne!(theme.tier(Tier::Production), theme.tier(Tier::Local), "{}", theme.name);
        }
    }
}
