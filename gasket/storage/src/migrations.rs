//! Database schema migrations.
//!
//! Each `run_*` function is a migration step. They are idempotent: checking
//! for column existence before ALTERing, using CREATE IF NOT EXISTS for tables
//! and indexes.

use sqlx::SqlitePool;

/// Run all migrations on an existing pool.
pub async fn run_all(pool: &SqlitePool) -> anyhow::Result<()> {
    migrate_add_watermark_to_summaries(pool).await?;
    migrate_add_sequence_to_events(pool).await?;
    migrate_add_needs_embedding_to_metadata(pool).await?;
    migrate_add_access_count_to_metadata(pool).await?;
    migrate_add_session_sequence_index(pool).await?;
    Ok(())
}

// ─── Migration helpers ───────────────────────────────────────────────────────

async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT COUNT(*) > 0 FROM pragma_table_info(?1) WHERE name = ?2",
    )
    .bind(table)
    .bind(column)
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

// ─── Individual migrations ────────────────────────────────────────────────────

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
        sqlx::query(
            "ALTER TABLE session_events ADD COLUMN sequence INTEGER NOT NULL DEFAULT 0",
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Add `needs_embedding` column to `memory_metadata` if it doesn't exist.
async fn migrate_add_needs_embedding_to_metadata(pool: &SqlitePool) -> anyhow::Result<()> {
    if !column_exists(pool, "memory_metadata", "needs_embedding").await {
        sqlx::query(
            "ALTER TABLE memory_metadata ADD COLUMN needs_embedding INTEGER NOT NULL DEFAULT 1",
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Add `access_count` column to `memory_metadata` if it doesn't exist.
async fn migrate_add_access_count_to_metadata(pool: &SqlitePool) -> anyhow::Result<()> {
    if !column_exists(pool, "memory_metadata", "access_count").await {
        sqlx::query(
            "ALTER TABLE memory_metadata ADD COLUMN access_count BIGINT NOT NULL DEFAULT 0",
        )
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
