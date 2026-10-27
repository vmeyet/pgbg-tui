//! pgbg's colours on top of vtui's palette: the tiers, so a production connection never looks like a
//! local one.
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

/// What failed: an error, a dead tunnel, a refused statement. It borrows the mention role, so an error never reads
/// as the red of a production frame, and a production frame never reads as an error.
pub(super) fn failure(theme: &Theme) -> Color {
    theme.mention
}
