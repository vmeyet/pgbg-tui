//! The release pgbg runs, and where `pgbg update` finds a newer one.
use std::sync::LazyLock;
use vtui::update::App;

/// pgbg as `vtui::update` sees it: its number, the commit `build.rs` embeds, its repo and its Homebrew formula.
pub(crate) const PGBG: App =
    App { version: env!("CARGO_PKG_VERSION"), commit: env!("GIT_HASH"), repo: env!("CARGO_PKG_REPOSITORY"), formula: "vmeyet/tap/pgbg" };

/// `0.1.0 (a1b2c3d)`, as `pgbg --version` shows it; the number alone when built without git history.
pub(crate) fn label() -> &'static str {
    static LABEL: LazyLock<String> = LazyLock::new(|| PGBG.label());
    &LABEL
}
