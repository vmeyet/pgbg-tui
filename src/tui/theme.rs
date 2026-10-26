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
