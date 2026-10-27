//! PostgreSQL in your terminal, read only by default: the library behind the `pgbg` binary.
mod act;
mod chart;
pub mod cli;
pub mod commands;
mod complete;
mod config;
mod connection;
mod db;
mod export;
#[cfg(test)]
mod fixtures;
mod format;
mod groups;
mod history;
pub mod log;
mod query;
mod render;
mod secret;
pub mod signal;
mod syntax;
mod tui;
mod tunnel;
