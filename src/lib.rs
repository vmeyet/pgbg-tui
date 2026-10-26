//! PostgreSQL in your terminal, read only by default: the library behind the `pgbg` binary.
pub mod cli;
pub mod commands;
mod config;
mod connection;
#[expect(dead_code, reason = "`pgbg run`, the next step of the roadmap, is its first caller")]
mod db;
pub mod log;
mod render;
mod secret;
