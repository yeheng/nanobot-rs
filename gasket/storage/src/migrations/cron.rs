//! Cron scheduling state tables.

use sqlx::SqlitePool;

/// Run cron schema migrations (tables + indexes).
pub async fn run_schema(pool: &SqlitePool) -> anyhow::Result<()> {
    create_cron_state_table(pool).await?;
    create_cron_indexes(pool).await?;
    Ok(())
}

async fn create_cron_state_table(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cron_state (
            job_id      TEXT PRIMARY KEY,
            last_run_at TEXT,
            next_run_at TEXT
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_cron_indexes(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_cron_state_next_run ON cron_state(next_run_at)")
        .execute(pool)
        .await?;
    Ok(())
}
