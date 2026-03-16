//! Memory store types (kept minimal — only SqliteStore re-export).
//!
//! Explicit long-term memory now lives exclusively in `~/.gasket/memory/*.md` files.
//! The `MemoryStore` trait and `MemoryEntry` types have been removed to eliminate
//! the Split-Brain problem (dual SQLite + filesystem memory sources).
//!
//! SQLite is only used for machine-state: sessions, summaries, cron jobs, kv.
