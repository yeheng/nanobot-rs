//! Cron job persistence API for SqliteStore.

use chrono::{DateTime, Utc};
use sqlx::Row;
use tracing::debug;

use super::SqliteStore;

/// Cron job row for database storage.
#[derive(Debug, Clone)]
pub struct CronJobRow {
    pub id: String,
    pub name: String,
    pub cron: String,
    pub message: String,
    pub channel: Option<String>,
    pub chat_id: Option<String>,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
    pub enabled: bool,
}

impl SqliteStore {
    /// Save or update a cron job (O(1) operation).
    pub async fn save_cron_job(&self, job: &CronJobRow) -> anyhow::Result<()> {
        let last_run = job.last_run.map(|dt| dt.to_rfc3339());
        let next_run = job.next_run.map(|dt| dt.to_rfc3339());
        let enabled_int: i64 = if job.enabled { 1 } else { 0 };

        sqlx::query(
            "INSERT OR REPLACE INTO cron_jobs (id, name, cron, message, channel, chat_id, last_run, next_run, enabled)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&job.id)
        .bind(&job.name)
        .bind(&job.cron)
        .bind(&job.message)
        .bind(&job.channel)
        .bind(&job.chat_id)
        .bind(&last_run)
        .bind(&next_run)
        .bind(enabled_int)
        .execute(&self.pool)
        .await?;
        debug!("Saved cron job: {}", job.id);
        Ok(())
    }

    /// Load all cron jobs.
    pub async fn load_cron_jobs(&self) -> anyhow::Result<Vec<CronJobRow>> {
        let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
            "SELECT id, name, cron, message, channel, chat_id, last_run, next_run, enabled FROM cron_jobs",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut jobs = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: String = row.get("id");
            let name: String = row.get("name");
            let cron: String = row.get("cron");
            let message: String = row.get("message");
            let channel: Option<String> = row.get("channel");
            let chat_id: Option<String> = row.get("chat_id");
            let last_run_str: Option<String> = row.get("last_run");
            let next_run_str: Option<String> = row.get("next_run");
            let enabled_int: i64 = row.get("enabled");

            let last_run = last_run_str
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            let next_run = next_run_str
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            jobs.push(CronJobRow {
                id,
                name,
                cron,
                message,
                channel,
                chat_id,
                last_run,
                next_run,
                enabled: enabled_int != 0,
            });
        }
        Ok(jobs)
    }

    /// Delete a cron job by ID. Returns true if the job existed.
    pub async fn delete_cron_job(&self, id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM cron_jobs WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Load cron jobs that are due to run (enabled and next_run <= now).
    /// Single Source of Truth: query directly from SQLite, no memory cache.
    pub async fn load_due_cron_jobs(&self, now: DateTime<Utc>) -> anyhow::Result<Vec<CronJobRow>> {
        let now_str = now.to_rfc3339();
        let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
            "SELECT id, name, cron, message, channel, chat_id, last_run, next_run, enabled
             FROM cron_jobs
             WHERE enabled = 1 AND next_run <= $1",
        )
        .bind(&now_str)
        .fetch_all(&self.pool)
        .await?;

        let mut jobs = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: String = row.get("id");
            let name: String = row.get("name");
            let cron: String = row.get("cron");
            let message: String = row.get("message");
            let channel: Option<String> = row.get("channel");
            let chat_id: Option<String> = row.get("chat_id");
            let last_run_str: Option<String> = row.get("last_run");
            let next_run_str: Option<String> = row.get("next_run");

            let last_run = last_run_str
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            let next_run = next_run_str
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            jobs.push(CronJobRow {
                id,
                name,
                cron,
                message,
                channel,
                chat_id,
                last_run,
                next_run,
                enabled: true,
            });
        }
        Ok(jobs)
    }

    /// Update a cron job's last_run and next_run timestamps.
    /// Called after a job executes to schedule its next run.
    pub async fn update_cron_job_run_times(
        &self,
        id: &str,
        last_run: DateTime<Utc>,
        next_run: Option<DateTime<Utc>>,
    ) -> anyhow::Result<()> {
        let last_run_str = last_run.to_rfc3339();
        let next_run_str = next_run.map(|dt| dt.to_rfc3339());

        sqlx::query("UPDATE cron_jobs SET last_run = $1, next_run = $2 WHERE id = $3")
            .bind(&last_run_str)
            .bind(&next_run_str)
            .bind(id)
            .execute(&self.pool)
            .await?;
        debug!("Updated cron job run times: {}", id);
        Ok(())
    }
}
