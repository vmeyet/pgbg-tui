//! One module per subcommand, each with a `run` the binary calls.
/// `pgbg connections`.
pub mod connections;
/// `pgbg run`.
pub mod run;

use std::path::Path;

/// Each key of the config this pgbg ignores, on stderr.
fn warn_of_ignored_keys(warnings: &[String], path: &Path) {
    if warnings.is_empty() {
        return;
    }
    for warning in warnings {
        eprintln!("! {warning}");
    }
    eprintln!("  in {}", path.display());
}
