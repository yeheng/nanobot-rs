//! Memory storage
//!
//! SQLite is used exclusively for machine-state (sessions, summaries, cron, kv).
//! Explicit long-term memory lives in `~/.nanobot/memory/*.md` files (SSOT).

mod sqlite;
mod store;

pub use sqlite::{CronJobRow, SqliteStore};
