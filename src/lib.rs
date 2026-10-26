//! PostgreSQL in your terminal, read only by default: the library behind the `pgbg` binary.
mod act;
#[cfg_attr(not(test), expect(dead_code, reason = "a saved query's `-- chart:` header, the next step, reads a `ChartSpec`"))]
mod chart;
pub mod cli;
pub mod commands;
mod config;
mod connection;
mod db;
pub mod log;
mod query;
mod render;
mod secret;
pub mod signal;
mod syntax;
mod tui;
mod tunnel;
