//! Cron service for scheduled tasks
//!
//! Jobs are persisted in SQLite for reliability — **Single Source of Truth**.
//! No memory cache, no dual-state synchronization issues.
//!
//! Legacy JSON files are automatically migrated on startup.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use crate::memory::{CronJobRow, SqliteStore};

/// A scheduled job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Unique job ID
    pub id: String,

    /// Job name
    pub name: String,

    /// Cron expression
    pub cron: String,

    /// Message to send
    pub message: String,

    /// Target channel
    #[serde(default)]
    pub channel: Option<String>,

    /// Target chat ID
    #[serde(default)]
    pub chat_id: Option<String>,

    /// Last run time
    #[serde(default)]
    pub last_run: Option<DateTime<Utc>>,

    /// Next run time
    #[serde(default)]
    pub next_run: Option<DateTime<Utc>>,

    /// Enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl CronJob {
    /// Create a new cron job
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        cron: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let cron_str = cron.into();
        let next_run = Self::calculate_next_run(&cron_str);

        Self {
            id: id.into(),
            name: name.into(),
            cron: cron_str,
            message: message.into(),
            channel: None,
            chat_id: None,
            last_run: None,
            next_run,
            enabled: true,
        }
    }

    /// Calculate next run time from cron expression
    fn calculate_next_run(cron_expr: &str) -> Option<DateTime<Utc>> {
        let schedule: Schedule = cron_expr.parse().ok()?;
        let now = chrono::Utc::now();
        schedule.after(&now).next()
    }

    /// Update next run time
    pub fn update_next_run(&mut self) {
        self.last_run = Some(Utc::now());
        self.next_run = Self::calculate_next_run(&self.cron);
    }
}

impl From<CronJobRow> for CronJob {
    fn from(row: CronJobRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            cron: row.cron,
            message: row.message,
            channel: row.channel,
            chat_id: row.chat_id,
            last_run: row.last_run,
            next_run: row.next_run,
            enabled: row.enabled,
        }
    }
}

/// Cron service for scheduled tasks.
///
/// **Single Source of Truth**: All job data lives in SQLite.
/// No memory cache, no synchronization issues.
pub struct CronService {
    store: SqliteStore,
}

impl CronService {
    /// Create a new cron service with SQLite persistence.
    ///
    /// Uses the default SqliteStore path (~/.nanobot/nanobot.db).
    /// Automatically migrates legacy JSON files if they exist.
    pub async fn new(workspace: PathBuf) -> Self {
        let store = SqliteStore::new()
            .await
            .expect("Failed to create SqliteStore for cron service");
        Self::with_store(store, workspace).await
    }

    /// Create a new cron service with a provided SqliteStore.
    ///
    /// Automatically migrates legacy JSON files if they exist.
    pub async fn with_store(store: SqliteStore, workspace: PathBuf) -> Self {
        // Try to migrate from legacy JSON if no jobs exist in SQLite
        let existing = store.load_cron_jobs().await.unwrap_or_default();
        if existing.is_empty() {
            let json_path = workspace.join("cron").join("jobs.json");
            if json_path.exists() {
                match Self::migrate_from_json(&store, &json_path).await {
                    Ok(count) => {
                        if count > 0 {
                            info!("Migrated {} cron jobs from JSON", count);
                            // Rename old file to prevent re-migration
                            let backup_path = json_path.with_extension("json.migrated");
                            if let Err(e) = tokio::fs::rename(&json_path, &backup_path).await {
                                warn!("Failed to rename migrated JSON file: {}", e);
                            }
                        }
                    }
                    Err(e) => warn!("Failed to migrate cron jobs from JSON: {}", e),
                }
            }
        } else {
            info!("Loaded {} cron jobs from SQLite", existing.len());
        }

        Self { store }
    }

    /// Migrate jobs from legacy JSON file to SQLite.
    async fn migrate_from_json(
        store: &SqliteStore,
        json_path: &std::path::Path,
    ) -> anyhow::Result<usize> {
        let content = tokio::fs::read_to_string(json_path).await?;
        let legacy_jobs: std::collections::HashMap<String, CronJob> =
            serde_json::from_str(&content)?;

        let mut count = 0;
        for (_id, mut job) in legacy_jobs {
            // Recalculate next_run in case it's stale
            job.next_run = CronJob::calculate_next_run(&job.cron);

            store
                .save_cron_job(&CronJobRow {
                    id: job.id.clone(),
                    name: job.name.clone(),
                    cron: job.cron.clone(),
                    message: job.message.clone(),
                    channel: job.channel.clone(),
                    chat_id: job.chat_id.clone(),
                    last_run: job.last_run,
                    next_run: job.next_run,
                    enabled: job.enabled,
                })
                .await?;
            count += 1;
        }

        Ok(count)
    }

    /// Add a job (immediately persisted to SQLite)
    #[instrument(name = "cron.add_job", skip_all, fields(job_id = %job.id))]
    pub async fn add_job(&self, job: CronJob) -> anyhow::Result<()> {
        self.store
            .save_cron_job(&CronJobRow {
                id: job.id.clone(),
                name: job.name.clone(),
                cron: job.cron.clone(),
                message: job.message.clone(),
                channel: job.channel.clone(),
                chat_id: job.chat_id.clone(),
                last_run: job.last_run,
                next_run: job.next_run,
                enabled: job.enabled,
            })
            .await?;
        info!("Added cron job: {} ({})", job.name, job.id);
        Ok(())
    }

    /// Remove a job (immediately persisted to SQLite)
    #[instrument(name = "cron.remove_job", skip(self), fields(job_id = %id))]
    pub async fn remove_job(&self, id: &str) -> anyhow::Result<bool> {
        let removed = self.store.delete_cron_job(id).await?;
        if removed {
            info!("Removed cron job: {}", id);
        }
        Ok(removed)
    }

    /// Get a job by ID (reads directly from SQLite)
    pub async fn get_job(&self, id: &str) -> anyhow::Result<Option<CronJob>> {
        let jobs = self.store.load_cron_jobs().await?;
        Ok(jobs.into_iter().find(|j| j.id == id).map(CronJob::from))
    }

    /// List all jobs (reads directly from SQLite)
    pub async fn list_jobs(&self) -> anyhow::Result<Vec<CronJob>> {
        let jobs = self.store.load_cron_jobs().await?;
        Ok(jobs.into_iter().map(CronJob::from).collect())
    }

    /// Get jobs that are due to run (query directly from SQLite)
    #[instrument(name = "cron.get_due_jobs", skip_all)]
    pub async fn get_due_jobs(&self) -> anyhow::Result<Vec<CronJob>> {
        let now = Utc::now();
        let jobs = self.store.load_due_cron_jobs(now).await?;
        Ok(jobs.into_iter().map(CronJob::from).collect())
    }

    /// Mark a job as run (immediately persisted to SQLite)
    #[instrument(name = "cron.mark_job_run", skip(self), fields(job_id = %id))]
    pub async fn mark_job_run(&self, id: &str) {
        let now = Utc::now();

        // Load the job to calculate next_run
        match self.get_job(id).await {
            Ok(Some(mut job)) => {
                job.update_next_run();

                if let Err(e) = self
                    .store
                    .update_cron_job_run_times(id, now, job.next_run)
                    .await
                {
                    warn!("Failed to persist cron job {}: {}", id, e);
                }

                debug!("Marked job {} as run", id);
            }
            Ok(None) => {
                warn!("Job {} not found when marking as run", id);
            }
            Err(e) => {
                warn!("Failed to load job {} for marking as run: {}", id, e);
            }
        }
    }
}
