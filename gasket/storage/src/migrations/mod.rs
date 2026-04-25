//! Database schema migrations.
//!
//! Base table creation via `run_schema` (idempotent, backward compatible).
//! Column additions via defensive `column_exists` checks.
//! The `./migrations` directory is reserved for future use.

use sqlx::SqlitePool;

pub mod cron;
pub mod kv;
pub mod maintenance;
pub mod memory;
pub mod session;

/// Run all migrations on an existing pool.
pub async fn run_all(pool: &SqlitePool) -> anyhow::Result<()> {
    // Run schema migrations first (table creation, idempotent via CREATE TABLE IF NOT EXISTS)
    session::run_schema(pool).await?;
    memory::run_schema(pool).await?;
    cron::run_schema(pool).await?;
    maintenance::run_schema(pool).await?;
    kv::run_schema(pool).await?;

    // Run incremental migrations (column additions) with defensive checks
    run_incremental(pool).await?;

    Ok(())
}

/// Incremental migrations for adding columns to existing tables.
async fn run_incremental(pool: &SqlitePool) -> anyhow::Result<()> {
    migrate_add_watermark_to_summaries(pool).await?;
    migrate_add_sequence_to_events(pool).await?;
    migrate_add_session_sequence_index(pool).await?;
    migrate_add_compaction_state(pool).await?;
    migrate_add_sync_sequence_to_wiki_pages(pool).await?;
    Ok(())
}

/// Add `sync_sequence` column to `wiki_pages` if it doesn't exist.
async fn migrate_add_sync_sequence_to_wiki_pages(pool: &SqlitePool) -> anyhow::Result<()> {
    if table_exists(pool, "wiki_pages").await
        && !column_exists(pool, "wiki_pages", "sync_sequence").await
    {
        sqlx::query("ALTER TABLE wiki_pages ADD COLUMN sync_sequence INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
    }
    Ok(())
}

// ─── Migration helpers ───────────────────────────────────────────────────────

async fn table_exists(pool: &SqlitePool, table: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = ?1",
    )
    .bind(table)
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT COUNT(*) > 0 FROM pragma_table_info(?1) WHERE name = ?2")
        .bind(table)
        .bind(column)
        .fetch_one(pool)
        .await
        .unwrap_or(false)
}

// ─── Incremental migrations ────────────────────────────────────────────────────

/// Add `covered_upto_sequence` column to `session_summaries` if it doesn't exist.
async fn migrate_add_watermark_to_summaries(pool: &SqlitePool) -> anyhow::Result<()> {
    if !column_exists(pool, "session_summaries", "covered_upto_sequence").await {
        sqlx::query(
            "ALTER TABLE session_summaries ADD COLUMN covered_upto_sequence INTEGER NOT NULL DEFAULT 0",
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Add `sequence` column to `session_events` if it doesn't exist.
async fn migrate_add_sequence_to_events(pool: &SqlitePool) -> anyhow::Result<()> {
    if !column_exists(pool, "session_events", "sequence").await {
        sqlx::query("ALTER TABLE session_events ADD COLUMN sequence INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Add the composite sequence index on `session_events` for watermark-based queries.
async fn migrate_add_session_sequence_index(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_events_session_sequence ON session_events(session_key, sequence)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Add compaction-state columns to `session_summaries` if they don't exist.
async fn migrate_add_compaction_state(pool: &SqlitePool) -> anyhow::Result<()> {
    if !column_exists(pool, "session_summaries", "compaction_in_progress").await {
        sqlx::query(
            "ALTER TABLE session_summaries ADD COLUMN compaction_in_progress INTEGER NOT NULL DEFAULT 0",
        )
        .execute(pool)
        .await?;
    }
    if !column_exists(pool, "session_summaries", "compaction_started_at").await {
        sqlx::query("ALTER TABLE session_summaries ADD COLUMN compaction_started_at TEXT")
            .execute(pool)
            .await?;
    }
    Ok(())
}
