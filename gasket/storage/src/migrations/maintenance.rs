//! Maintenance state tables for background task watermarks.

use sqlx::SqlitePool;

/// Run maintenance schema migrations.
pub async fn run_schema(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS maintenance_state (
            task_name       TEXT NOT NULL,
            target_id       TEXT NOT NULL,
            last_watermark  INTEGER NOT NULL DEFAULT 0,
            updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (task_name, target_id)
        )
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}
