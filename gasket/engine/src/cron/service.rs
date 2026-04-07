//! Cron service for scheduled tasks
//!
//! **File-Driven Architecture**: Jobs are defined in `~/.gasket/cron/*.md` files.
//! No SQLite persistence — runtime state is in-memory only.
//! Supports hot reload via file system watching.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

use chrono::{DateTime, Utc};
use cron::Schedule;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use tracing::{debug, info, instrument, warn};

/// A scheduled job (in-memory only)
#[derive(Debug, Clone)]
pub struct CronJob {
    /// Unique job ID (filename without .md)
    pub id: String,
    /// Job name
    pub name: String,
    /// Cron expression
    pub cron: String,
    /// Message to send
    pub message: String,
    /// Target channel
    pub channel: Option<String>,
    /// Target chat ID
    pub chat_id: Option<String>,
    /// Next run time (in-memory only)
    pub next_run: Option<DateTime<Utc>>,
    /// Enabled
    pub enabled: bool,
    /// File path for hot reload
    pub file_path: PathBuf,
}

/// Frontmatter structure for markdown job files
#[derive(Debug, Deserialize)]
struct CronJobFrontmatter {
    name: Option<String>,
    cron: String,
    channel: Option<String>,
    to: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Cron service for scheduled tasks.
///
/// **File-Driven**: All job data lives in `~/.gasket/cron/*.md` files.
/// No memory cache synchronization issues — files are Single Source of Truth.
pub struct CronService {
    /// In-memory job storage
    jobs: RwLock<HashMap<String, CronJob>>,
    /// Workspace path
    workspace: PathBuf,
    /// File watcher
    watcher: RwLock<Option<RecommendedWatcher>>,
    /// Watcher event receiver (wrapped in Mutex for thread safety)
    rx: Mutex<Receiver<Result<Event, notify::Error>>>,
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
            next_run,
            enabled: true,
            file_path: PathBuf::new(),
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
        self.next_run = Self::calculate_next_run(&self.cron);
    }
}

/// Parse markdown file with frontmatter
fn parse_markdown(content: &str, file_path: &Path) -> anyhow::Result<CronJob> {
    // Split frontmatter and body
    // Format: ---\n<frontmatter>\n---\n<body>
    let parts: Vec<&str> = content.splitn(3, "---\n").collect();
    if parts.len() < 3 {
        anyhow::bail!("Invalid markdown format: missing frontmatter delimiters");
    }

    // Parse frontmatter (parts[1])
    let fm: CronJobFrontmatter = serde_yaml::from_str(parts[1])?;

    // Body is parts[2]
    let message = parts.get(2).unwrap_or(&"").trim().to_string();

    // Calculate next_run
    let next_run = CronJob::calculate_next_run(&fm.cron);

    // Use filename as ID
    let id = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    Ok(CronJob {
        id: id.clone(),
        name: fm.name.unwrap_or(id),
        cron: fm.cron,
        message,
        channel: fm.channel,
        chat_id: fm.to,
        next_run,
        enabled: fm.enabled,
        file_path: file_path.to_path_buf(),
    })
}

impl CronService {
    /// Create a new cron service with file-driven architecture.
    ///
    /// Jobs are loaded from `~/.gasket/cron/*.md` files.
    /// File watcher is started for hot reload support.
    pub async fn new(workspace: PathBuf) -> Self {
        let (tx, rx) = channel();

        let service = Self {
            jobs: RwLock::new(HashMap::new()),
            workspace: workspace.clone(),
            watcher: RwLock::new(None),
            rx: Mutex::new(rx),
        };

        // Load existing jobs
        service.load_all_jobs(&workspace);

        // Start file watcher
        service.start_watcher(tx);

        service
    }

    /// Load all cron jobs from markdown files
    fn load_all_jobs(&self, workspace: &Path) {
        let cron_dir = workspace.join("cron");
        if !cron_dir.exists() {
            let _ = std::fs::create_dir_all(&cron_dir);
            return;
        }

        let mut count = 0;
        for entry in std::fs::read_dir(&cron_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                match Self::parse_markdown_file(&path) {
                    Ok(job) => {
                        debug!("Loaded cron job from markdown: {}", job.id);
                        self.jobs.write().insert(job.id.clone(), job);
                        count += 1;
                    }
                    Err(e) => {
                        warn!("Failed to load cron job from {:?}: {}", path, e);
                    }
                }
            }
        }

        if count > 0 {
            info!("Loaded {} cron jobs from markdown files", count);
        }
    }

    /// Parse a single markdown cron job file
    fn parse_markdown_file(path: &Path) -> anyhow::Result<CronJob> {
        let content = std::fs::read_to_string(path)?;
        parse_markdown(&content, path)
    }

    /// Start file watcher for hot reload
    fn start_watcher(&self, tx: std::sync::mpsc::Sender<Result<Event, notify::Error>>) {
        let cron_dir = self.workspace.join("cron");

        match RecommendedWatcher::new(tx, notify::Config::default()) {
            Ok(mut watcher) => {
                if let Err(e) = watcher.watch(&cron_dir, RecursiveMode::NonRecursive) {
                    warn!("Failed to watch cron directory: {}", e);
                }
                *self.watcher.write() = Some(watcher);
                debug!("Started cron file watcher for {:?}", cron_dir);
            }
            Err(e) => {
                warn!("Failed to create file watcher: {}", e);
            }
        }
    }

    /// Poll watcher and update jobs (called periodically)
    fn poll_watcher(&self) {
        let rx = self.rx.lock();
        while let Ok(event_result) = rx.try_recv() {
            if let Ok(event) = event_result {
                for path in &event.paths {
                    if path.extension().is_some_and(|ext| ext == "md") {
                        match event.kind {
                            notify::EventKind::Modify(_) | notify::EventKind::Create(_) => {
                                // Small delay to ensure file is fully written
                                std::thread::sleep(std::time::Duration::from_millis(50));
                                if let Ok(job) = Self::parse_markdown_file(path) {
                                    let job_id = job.id.clone();
                                    self.jobs.write().insert(job_id.clone(), job);
                                    debug!("Reloaded cron job: {}", job_id);
                                }
                            }
                            notify::EventKind::Remove(_) => {
                                if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                                    self.jobs.write().remove(id);
                                    debug!("Removed cron job: {}", id);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    /// Add a job (creates/updates markdown file)
    #[instrument(name = "cron.add_job", skip_all, fields(job_id = %job.id))]
    pub async fn add_job(&self, job: CronJob) -> anyhow::Result<()> {
        // Create cron directory if it doesn't exist
        let cron_dir = self.workspace.join("cron");
        if !cron_dir.exists() {
            std::fs::create_dir_all(&cron_dir)?;
        }

        // Write markdown file
        let file_path = cron_dir.join(format!("{}.md", job.id));
        let content = format!(
            "---
name: {}
cron: \"{}\"
channel: {}
to: {}
enabled: {}
---

{}",
            job.name,
            job.cron,
            job.channel.unwrap_or_default(),
            job.chat_id.unwrap_or_default(),
            job.enabled,
            job.message
        );

        std::fs::write(&file_path, content)?;
        info!("Added cron job: {} ({})", job.name, job.id);
        Ok(())
    }

    /// Remove a job (deletes markdown file)
    #[instrument(name = "cron.remove_job", skip(self), fields(job_id = %id))]
    pub async fn remove_job(&self, id: &str) -> anyhow::Result<bool> {
        let cron_dir = self.workspace.join("cron");
        let file_path = cron_dir.join(format!("{}.md", id));

        if !file_path.exists() {
            return Ok(false);
        }

        std::fs::remove_file(&file_path)?;
        // Remove from memory immediately
        self.jobs.write().remove(id);
        info!("Removed cron job: {}", id);
        Ok(true)
    }

    /// Get a job by ID (reads from memory)
    pub async fn get_job(&self, id: &str) -> anyhow::Result<Option<CronJob>> {
        self.poll_watcher();
        Ok(self.jobs.read().get(id).cloned())
    }

    /// List all jobs (reads from memory)
    pub async fn list_jobs(&self) -> anyhow::Result<Vec<CronJob>> {
        self.poll_watcher();
        Ok(self.jobs.read().values().cloned().collect())
    }

    /// Get jobs that are due to run (query from memory)
    #[instrument(name = "cron.get_due_jobs", skip_all)]
    pub async fn get_due_jobs(&self) -> anyhow::Result<Vec<CronJob>> {
        self.poll_watcher();
        let now = Utc::now();

        Ok(self
            .jobs
            .read()
            .values()
            .filter(|job| job.enabled && job.next_run.is_some_and(|nr| nr <= now))
            .cloned()
            .collect())
    }

    /// Check if any job should execute immediately on startup
    pub fn should_execute_on_startup(&self, job: &CronJob) -> bool {
        job.next_run.is_some_and(|nr| nr <= Utc::now())
    }

    /// Update job's next_run time (in-memory only, no persistence)
    pub async fn update_job_next_run(&self, id: &str, next_run: Option<DateTime<Utc>>) {
        if let Some(job) = self.jobs.write().get_mut(id) {
            job.next_run = next_run;
        }
    }
}
