//! pgbg's colours on top of vtui's palette: the tiers, so a production connection never looks like a
//! local one, and the SQL tokens.
use crate::connection::Tier;
use crate::syntax::Token;
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

/// vtui's palette has no code colours, so each token borrows the role closest to it.
pub(super) fn token(theme: &Theme, token: Token) -> Color {
    match token {
        Token::Keyword => theme.accent,
        Token::String => theme.success,
        Token::Comment => theme.faded,
        Token::Number | Token::Constant => theme.warn,
        Token::Type => theme.code,
        Token::Function => theme.link,
        Token::Punctuation => theme.muted,
    }
}
