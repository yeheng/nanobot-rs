//! Memory embedding and metadata tables (DEPRECATED).
//!
//! These tables are no longer used by application code.
//! `memory_metadata` was never read or written.
//! `memory_embeddings` was write-only (no SELECTs).
//!
//! The function is preserved as a no-op to maintain the migration call chain.
//! Existing tables in live databases are left untouched.

use sqlx::SqlitePool;

/// No-op — dead tables are no longer created for new databases.
pub async fn run_schema(_pool: &SqlitePool) -> anyhow::Result<()> {
    Ok(())
}
