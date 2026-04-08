//! Cron service for scheduled tasks
//!
//! **File-Driven Architecture**: Jobs are defined in `~/.gasket/cron/*.md` files.
//! No SQLite persistence — runtime state is in-memory only.
//! Supports hot reload via file system watching with debouncing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use cron::Schedule;
use gasket_storage::fs::atomic_write;
use gasket_storage::memory::extract_frontmatter_raw;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use serde::Deserialize;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tracing::{debug, info, instrument, warn};

/// Debounce duration for file watcher events (milliseconds).
const DEBOUNCE_MS: u64 = 500;

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
    /// Parsed cron schedule (cached to avoid parsing on every check)
    schedule: Option<Schedule>,
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
    /// In-memory job storage (Arc for sharing with background task)
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    /// Workspace path
    workspace: PathBuf,
    /// _watcher needs to be kept alive to continue watching
    _watcher: RwLock<Option<RecommendedWatcher>>,
    /// Sender for watcher events (kept to prevent channel closing)
    _tx: UnboundedSender<Result<Event, notify::Error>>,
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
        let (schedule, next_run) = Self::parse_schedule(&cron_str);

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
            schedule,
        }
    }

    /// Parse cron expression and calculate next run time.
    /// Returns (parsed_schedule, next_run_time).
    fn parse_schedule(cron_expr: &str) -> (Option<Schedule>, Option<DateTime<Utc>>) {
        let schedule: Schedule = match cron_expr.parse() {
            Ok(s) => s,
            Err(_) => return (None, None),
        };
        let now = chrono::Utc::now();
        let next_run = schedule.after(&now).next();
        (Some(schedule), next_run)
    }

    /// Calculate next run time using the cached schedule.
    /// This avoids re-parsing the cron expression on every check.
    fn calculate_next_run(&self) -> Option<DateTime<Utc>> {
        let schedule = self.schedule.as_ref()?;
        let now = chrono::Utc::now();
        schedule.after(&now).next()
    }

    /// Update next run time using the cached schedule
    pub fn update_next_run(&mut self) {
        self.next_run = self.calculate_next_run();
    }
}

/// Parse markdown file with frontmatter
fn parse_markdown(content: &str, file_path: &Path) -> anyhow::Result<CronJob> {
    let (yaml_str, body) = extract_frontmatter_raw(content)?;

    // Parse frontmatter
    let fm: CronJobFrontmatter = serde_yaml::from_str(&yaml_str)
        .map_err(|e| anyhow::anyhow!("Failed to parse YAML frontmatter: {}", e))?;

    // Parse schedule and calculate next_run
    let (schedule, next_run) = CronJob::parse_schedule(&fm.cron);

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
        message: body,
        channel: fm.channel,
        chat_id: fm.to,
        next_run,
        enabled: fm.enabled,
        file_path: file_path.to_path_buf(),
        schedule,
    })
}

impl CronService {
    /// Create a new cron service with file-driven architecture.
    ///
    /// Jobs are loaded from `~/.gasket/cron/*.md` files.
    /// File watcher is started for hot reload support.
    pub async fn new(workspace: PathBuf) -> Self {
        let (tx, rx) = unbounded_channel::<Result<Event, notify::Error>>();

        let jobs = Arc::new(RwLock::new(HashMap::new()));

        let service = Self {
            jobs: jobs.clone(),
            workspace: workspace.clone(),
            _watcher: RwLock::new(None),
            _tx: tx.clone(),
        };

        // Load existing jobs
        service.load_all_jobs(&workspace);

        // Start file watcher - convert std::sync::mpsc to tokio::sync::mpsc
        let watcher_tx = tx.clone();
        let (std_tx, std_rx) = std::sync::mpsc::channel::<Result<Event, notify::Error>>();
        service.start_watcher(std_tx);

        // Spawn bridge task: std::sync::mpsc -> tokio::sync::mpsc
        tokio::spawn(async move {
            loop {
                while let Ok(event) = std_rx.recv() {
                    if watcher_tx.send(event).is_err() {
                        break;
                    }
                }
            }
        });

        // Spawn background task to process watcher events with debouncing
        tokio::spawn(Self::debounce_loop(rx, jobs));

        service
    }

    /// Debounce loop that receives raw events and emits settled events.
    ///
    /// Events are collected and only processed after DEBOUNCE_MS has passed
    /// since the last event for a given file. This avoids processing
    /// intermediate states during multi-step writes.
    async fn debounce_loop(
        mut rx: tokio::sync::mpsc::UnboundedReceiver<Result<Event, notify::Error>>,
        jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    ) {
        use std::collections::HashMap;

        let mut pending: HashMap<PathBuf, (notify::EventKind, Instant)> = HashMap::new();
        let debounce = Duration::from_millis(DEBOUNCE_MS);
        let check_interval = Duration::from_millis(100);

        loop {
            // Process incoming events
            while let Ok(event_result) = rx.try_recv() {
                match event_result {
                    Ok(event) => {
                        for path in &event.paths {
                            if path.extension().is_some_and(|ext| ext == "md") {
                                pending.insert(path.clone(), (event.kind, Instant::now()));
                            }
                        }
                    }
                    Err(e) => {
                        warn!("File watcher error: {}", e);
                    }
                }
            }

            // Process settled events
            let now = Instant::now();
            let settled: Vec<(PathBuf, notify::EventKind)> = pending
                .iter()
                .filter(|(_, (_, timestamp))| now.saturating_duration_since(*timestamp) >= debounce)
                .map(|(path, (kind, _))| (path.clone(), *kind))
                .collect();

            for (path, kind) in settled {
                pending.remove(&path);
                Self::process_watcher_event(&jobs, &path, kind).await;
            }

            tokio::time::sleep(check_interval).await;
        }
    }

    /// Process a single settled watcher event.
    async fn process_watcher_event(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        path: &Path,
        kind: notify::EventKind,
    ) {
        match kind {
            notify::EventKind::Modify(_) | notify::EventKind::Create(_) => {
                if path.exists() {
                    match Self::parse_markdown_file(path) {
                        Ok(job) => {
                            let job_id = job.id.clone();
                            jobs.write().insert(job_id.clone(), job);
                            debug!("Reloaded cron job: {}", job_id);
                        }
                        Err(e) => {
                            warn!("Failed to parse cron job file {:?}: {}", path, e);
                        }
                    }
                }
            }
            notify::EventKind::Remove(_) => {
                if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                    jobs.write().remove(id);
                    debug!("Removed cron job: {}", id);
                }
            }
            _ => {}
        }
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
                        info!("Loaded cron job from markdown: {}", job.id);
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
                *self._watcher.write() = Some(watcher);
                debug!("Started cron file watcher for {:?}", cron_dir);
            }
            Err(e) => {
                warn!("Failed to create file watcher: {}", e);
            }
        }
    }

    /// Add a job (creates/updates markdown file)
    #[instrument(name = "cron.add_job", skip_all, fields(job_id = %job.id))]
    pub async fn add_job(&self, job: CronJob) -> anyhow::Result<()> {
        // Create cron directory if it doesn't exist
        let cron_dir = self.workspace.join("cron");
        if !cron_dir.exists() {
            tokio::fs::create_dir_all(&cron_dir).await?;
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
            job.channel.as_deref().unwrap_or(""),
            job.chat_id.as_deref().unwrap_or(""),
            job.enabled,
            job.message
        );

        atomic_write(&file_path, content).await?;

        // IMMEDIATELY update in-memory state for read-after-write consistency
        let job_id = job.id.clone();
        self.jobs.write().insert(job_id.clone(), job);

        info!("Added cron job: {} ({})", job_id, job_id);
        Ok(())
    }

    /// Remove a job (deletes markdown file)
    #[instrument(name = "cron.remove_job", skip(self), fields(job_id = %id))]
    pub async fn remove_job(&self, id: &str) -> anyhow::Result<bool> {
        let cron_dir = self.workspace.join("cron");
        let file_path = cron_dir.join(format!("{}.md", id));

        if !file_path.exists() {
            // Also remove from memory if it exists there (stale state)
            let removed_from_memory = self.jobs.write().remove(id).is_some();
            return Ok(removed_from_memory);
        }

        // FIRST remove from memory, then delete file
        self.jobs.write().remove(id);

        tokio::fs::remove_file(&file_path).await?;
        info!("Removed cron job: {}", id);
        Ok(true)
    }

    /// Get a job by ID (reads from memory)
    pub async fn get_job(&self, id: &str) -> anyhow::Result<Option<CronJob>> {
        // No more poll_watcher() - background task handles updates
        Ok(self.jobs.read().get(id).cloned())
    }

    /// List all jobs (reads from memory)
    pub async fn list_jobs(&self) -> anyhow::Result<Vec<CronJob>> {
        // No more poll_watcher() - background task handles updates
        Ok(self.jobs.read().values().cloned().collect())
    }

    /// Get jobs that are due to run (query from memory)
    #[instrument(name = "cron.get_due_jobs", skip_all)]
    pub async fn get_due_jobs(&self) -> anyhow::Result<Vec<CronJob>> {
        // No more poll_watcher() - background task handles updates
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_frontmatter_raw_basic() {
        let content = r#"---
name: Test Job
cron: "0 9 * * *"
---

Hello World"#;

        let (yaml, body) = extract_frontmatter_raw(content).unwrap();
        assert!(yaml.contains("name: Test Job"));
        assert!(yaml.contains("cron:"));
        assert_eq!(body, "Hello World");
    }

    #[test]
    fn test_extract_frontmatter_raw_with_crlf() {
        let content = "---\r\nname: Test\r\ncron: \"0 9 * * *\"\r\n---\r\n\r\nBody content";

        let (yaml, body) = extract_frontmatter_raw(content).unwrap();
        assert!(yaml.contains("name: Test"));
        assert_eq!(body, "Body content");
    }

    #[test]
    fn test_extract_frontmatter_raw_with_code_block() {
        // Body contains --- which should not confuse the parser
        let content = r#"---
name: Code Job
cron: "*/5 * * * *"
---

Some code:
```
---
```

More content"#;

        let (yaml, body) = extract_frontmatter_raw(content).unwrap();
        assert!(yaml.contains("name: Code Job"));
        assert!(body.contains("---")); // Body should contain the code block
    }

    #[test]
    fn test_extract_frontmatter_raw_missing_start() {
        let content = "No frontmatter here";
        assert!(extract_frontmatter_raw(content).is_err());
    }

    #[test]
    fn test_extract_frontmatter_raw_missing_end() {
        let content = "---\nname: Test\nNo end delimiter";
        assert!(extract_frontmatter_raw(content).is_err());
    }

    #[test]
    fn test_parse_markdown_complete() {
        let content = r#"---
name: My Job
cron: "0 9 * * Mon"
channel: telegram
to: "12345"
enabled: true
---

Send daily report"#;

        let path = Path::new("/tmp/test-job.md");
        let job = parse_markdown(content, path).unwrap();

        assert_eq!(job.name, "My Job");
        assert_eq!(job.cron, "0 9 * * Mon");
        assert_eq!(job.channel, Some("telegram".to_string()));
        assert_eq!(job.chat_id, Some("12345".to_string()));
        assert!(job.enabled);
        assert_eq!(job.message, "Send daily report");
    }

    #[test]
    fn test_cron_job_parse_schedule() {
        // Test valid cron expression (every minute)
        let (schedule, next_run) = CronJob::parse_schedule("0 * * * * *");
        // Should have a parsed schedule and next_run calculated
        assert!(
            schedule.is_some(),
            "Valid cron '0 * * * * *' should parse into a Schedule"
        );
        assert!(
            next_run.is_some(),
            "Valid cron '0 * * * * *' should calculate next_run"
        );

        if let Some(next) = next_run {
            let now = Utc::now();
            assert!(
                next > now,
                "Next run {:?} should be after now {:?}",
                next,
                now
            );
        }
    }

    #[test]
    fn test_cron_job_invalid_cron() {
        let job = CronJob::new("test", "Test", "invalid cron", "Message");
        // Should handle invalid cron gracefully
        assert!(job.next_run.is_none());
        assert!(job.schedule.is_none());
    }
}
