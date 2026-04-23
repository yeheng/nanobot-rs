//! Cron state persistence repository.

use tracing::debug;

/// Repository for cron job execution state.
///
/// Persists `last_run_at` / `next_run_at` per job to survive restarts.
#[derive(Clone)]
pub struct CronStore {
    pool: sqlx::SqlitePool,
}

impl CronStore {
    /// Create from an existing pool.
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    /// Get cron state for a job.
    ///
    /// Returns `(last_run_at, next_run_at)` if state exists, or `None` if not found.
    pub async fn get_state(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<(Option<String>, Option<String>)>> {
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT last_run_at, next_run_at FROM cron_state WHERE job_id = $1")
                .bind(job_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    /// Upsert cron state for a job.
    pub async fn upsert_state(
        &self,
        job_id: &str,
        last_run: Option<&str>,
        next_run: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO cron_state (job_id, last_run_at, next_run_at) VALUES ($1, $2, $3)",
        )
        .bind(job_id)
        .bind(last_run)
        .bind(next_run)
        .execute(&self.pool)
        .await?;
        debug!(
            "Updated cron state for job {}: last_run={:?}, next_run={:?}",
            job_id, last_run, next_run
        );
        Ok(())
    }

    /// Delete cron state for a job.
    pub async fn delete_state(&self, job_id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM cron_state WHERE job_id = $1")
            .bind(job_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() > 0 {
            debug!("Deleted cron state for job {}", job_id);
        }
        Ok(result.rows_affected() > 0)
    }
}
