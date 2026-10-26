//! The tier colours on top of vtui's palette, so a production connection never looks like a local one.
use crate::connection::Tier;
use ratatui::style::Color;
use vtui::theme::Theme;

/// Local green, staging amber, production red.
pub(super) fn tier(theme: &Theme, tier: Tier) -> Color {
    match tier {
        Tier::Local => theme.success,
        Tier::Staging => theme.warn,
        Tier::Production => theme.danger,
    }
}
