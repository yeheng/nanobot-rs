//! Cron service for scheduled tasks
//!
//! Jobs are persisted in SQLite for reliability and O(1) operations.
//! Legacy JSON files are automatically migrated on startup.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
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
/// Jobs are persisted in SQLite for reliability. Legacy JSON files
/// are automatically migrated on startup.
pub struct CronService {
    jobs: RwLock<Vec<CronJob>>,
    store: SqliteStore,
}

impl CronService {
    /// Create a new cron service with SQLite persistence.
    ///
    /// Uses the default SqliteStore path (~/.nanobot/memory.db).
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
        // Try to load from SQLite first
        let jobs = async {
            // Load from SQLite
            match store.load_cron_jobs().await {
                Ok(jobs) if !jobs.is_empty() => {
                    let jobs: Vec<CronJob> = jobs.into_iter().map(CronJob::from).collect();
                    info!("Loaded {} cron jobs from SQLite", jobs.len());
                    return jobs;
                }
                Ok(_) => {}
                Err(e) => warn!("Failed to load cron jobs from SQLite: {}", e),
            }

            // Try to migrate from legacy JSON
            let json_path = workspace.join("cron").join("jobs.json");
            if json_path.exists() {
                match Self::migrate_from_json(&store, &json_path).await {
                    Ok(jobs) => {
                        info!("Migrated {} cron jobs from JSON", jobs.len());
                        // Rename old file to prevent re-migration
                        let backup_path = json_path.with_extension("json.migrated");
                        if let Err(e) = tokio::fs::rename(&json_path, &backup_path).await {
                            warn!("Failed to rename migrated JSON file: {}", e);
                        }
                        return jobs;
                    }
                    Err(e) => warn!("Failed to migrate cron jobs from JSON: {}", e),
                }
            }

            Vec::new()
        }
        .await;

        Self {
            jobs: RwLock::new(jobs),
            store,
        }
    }

    /// Migrate jobs from legacy JSON file to SQLite.
    async fn migrate_from_json(
        store: &SqliteStore,
        json_path: &std::path::Path,
    ) -> anyhow::Result<Vec<CronJob>> {
        let content = tokio::fs::read_to_string(json_path).await?;
        let legacy_jobs: std::collections::HashMap<String, CronJob> =
            serde_json::from_str(&content)?;

        let mut jobs = Vec::new();
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

            jobs.push(job);
        }

        Ok(jobs)
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

        let mut jobs = self.jobs.write().await;
        jobs.push(job.clone());
        info!("Added cron job: {} ({})", job.name, job.id);
        Ok(())
    }

    /// Remove a job (immediately persisted to SQLite)
    #[instrument(name = "cron.remove_job", skip(self), fields(job_id = %id))]
    pub async fn remove_job(&self, id: &str) -> anyhow::Result<bool> {
        let removed = self.store.delete_cron_job(id).await?;

        if removed {
            let mut jobs = self.jobs.write().await;
            jobs.retain(|j| j.id != id);
            info!("Removed cron job: {}", id);
        }

        Ok(removed)
    }

    /// Get a job
    pub async fn get_job(&self, id: &str) -> Option<CronJob> {
        let jobs = self.jobs.read().await;
        jobs.iter().find(|j| j.id == id).cloned()
    }

    /// List all jobs
    pub async fn list_jobs(&self) -> Vec<CronJob> {
        let jobs = self.jobs.read().await;
        jobs.clone()
    }

    /// Get jobs that are due to run
    #[instrument(name = "cron.get_due_jobs", skip_all)]
    pub async fn get_due_jobs(&self) -> Vec<CronJob> {
        let jobs = self.jobs.read().await;
        let now = Utc::now();

        jobs.iter()
            .filter(|job| job.enabled && job.next_run.is_some_and(|next| next <= now))
            .cloned()
            .collect()
    }

    /// Mark a job as run (immediately persisted to SQLite)
    #[instrument(name = "cron.mark_job_run", skip(self), fields(job_id = %id))]
    pub async fn mark_job_run(&self, id: &str) {
        let mut jobs = self.jobs.write().await;

        if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
            job.update_next_run();

            if let Err(e) = self
                .store
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
                .await
            {
                warn!("Failed to persist cron job {}: {}", id, e);
            }

            debug!("Marked job {} as run", id);
        }
    }
}
