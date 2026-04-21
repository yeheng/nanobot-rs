//! Key-value store tables.

use sqlx::SqlitePool;

/// Run KV schema migrations (tables).
pub async fn run_schema(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS kv_store (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    )
    .execute(pool)
    .await?;
    Ok(())
}
