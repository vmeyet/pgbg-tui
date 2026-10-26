//! PostgreSQL in your terminal, read only by default: the library behind the `pgbg` binary.
mod act;
pub mod cli;
pub mod commands;
mod config;
mod connection;
mod db;
pub mod log;
mod query;
mod render;
mod secret;
mod syntax;
mod tui;
mod tunnel;
