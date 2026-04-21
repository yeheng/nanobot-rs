//! Memory embedding and metadata tables.

use sqlx::SqlitePool;

/// Run memory schema migrations (tables + indexes).
pub async fn run_schema(pool: &SqlitePool) -> anyhow::Result<()> {
    create_memory_embeddings_table(pool).await?;
    create_memory_metadata_table(pool).await?;
    create_memory_indexes(pool).await?;
    Ok(())
}

async fn create_memory_embeddings_table(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS memory_embeddings (
            memory_path   TEXT PRIMARY KEY,
            scenario      TEXT NOT NULL,
            tags          TEXT,
            frequency     TEXT NOT NULL DEFAULT 'warm',
            embedding     BLOB NOT NULL,
            token_count   INTEGER NOT NULL,
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_memory_metadata_table(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS memory_metadata (
            id          TEXT NOT NULL,
            path        TEXT NOT NULL,
            scenario    TEXT NOT NULL,
            title       TEXT NOT NULL DEFAULT '',
            memory_type TEXT NOT NULL DEFAULT 'note',
            frequency   TEXT NOT NULL DEFAULT 'warm',
            tags        TEXT NOT NULL DEFAULT '[]',
            tokens      INTEGER NOT NULL DEFAULT 0,
            updated     TEXT NOT NULL DEFAULT '',
            last_accessed TEXT NOT NULL DEFAULT '',
            access_count BIGINT NOT NULL DEFAULT 0,
            file_mtime  BIGINT NOT NULL DEFAULT 0,
            file_size   BIGINT NOT NULL DEFAULT 0,
            needs_embedding INTEGER NOT NULL DEFAULT 1,
            PRIMARY KEY (scenario, path)
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_memory_indexes(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_mem_emb_scenario ON memory_embeddings(scenario)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_mem_emb_frequency ON memory_embeddings(frequency)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_meta_scenario_freq ON memory_metadata(scenario, frequency)",
    )
    .execute(pool)
    .await?;
    Ok(())
}
