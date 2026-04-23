//! Maintenance watermark storage repository.

/// Repository for maintenance task watermarks.
///
/// Tracks per-task, per-target processing progress (e.g. evolution scans).
#[derive(Clone)]
pub struct MaintenanceStore {
    pool: sqlx::SqlitePool,
}

impl MaintenanceStore {
    /// Create from an existing pool.
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    /// Read the last watermark for a given maintenance task and target.
    pub async fn read_watermark(&self, task_name: &str, target_id: &str) -> anyhow::Result<i64> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT last_watermark FROM maintenance_state WHERE task_name = ?1 AND target_id = ?2",
        )
        .bind(task_name)
        .bind(target_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(v,)| v).unwrap_or(0))
    }

    /// Write (or overwrite) the watermark for a maintenance task and target.
    pub async fn write_watermark(
        &self,
        task_name: &str,
        target_id: &str,
        watermark: i64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO maintenance_state (task_name, target_id, last_watermark, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))",
        )
        .bind(task_name)
        .bind(target_id)
        .bind(watermark)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
