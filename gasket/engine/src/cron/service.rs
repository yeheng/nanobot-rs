//! Cron service for scheduled tasks
//!
//! **File-Driven Architecture**: Jobs are defined in `~/.gasket/cron/*.md` files.
//! No SQLite persistence — runtime state is in-memory only.
//! Supports hot reload via file system watching.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use cron::Schedule;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use serde::Deserialize;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
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

/// Parse frontmatter and body from markdown content.
///
/// This is a generic parser that handles:
/// - Leading/trailing whitespace
/// - Windows line endings (\r\n)
/// - Content containing `---` after frontmatter
///
/// Returns (frontmatter_yaml, body) or error if format is invalid.
fn parse_frontmatter_generic(content: &str) -> anyhow::Result<(String, String)> {
    let content = content.trim_start();

    if !content.starts_with("---") {
        anyhow::bail!("Invalid markdown format: missing frontmatter start delimiter '---'");
    }

    // Find the end of frontmatter (\n--- or \r\n---)
    // Skip the first "---"
    let after_start = &content[3..];

    // Find the next "\n---" which closes frontmatter
    // We need to handle both \n and \r\n
    let end_pos = after_start.find("\n---").ok_or_else(|| {
        anyhow::anyhow!("Invalid markdown format: missing frontmatter end delimiter '---'")
    })?;

    // Extract YAML (skip initial ---, take content until closing ---)
    let yaml_str = &after_start[..end_pos];
    // Normalize line endings for YAML parsing
    let yaml_str = yaml_str.replace("\r\n", "\n").replace('\r', "\n");

    // Extract body (skip past the closing ---)
    // Position after "\n---" is end_pos + 4 (for "\n---")
    let body_start = 3 + end_pos + 4;
    let body = if body_start < content.len() {
        content[body_start..].trim().to_string()
    } else {
        String::new()
    };

    Ok((yaml_str, body))
}

/// Parse markdown file with frontmatter
fn parse_markdown(content: &str, file_path: &Path) -> anyhow::Result<CronJob> {
    let (yaml_str, body) = parse_frontmatter_generic(content)?;

    // Parse frontmatter
    let fm: CronJobFrontmatter = serde_yaml::from_str(&yaml_str)
        .map_err(|e| anyhow::anyhow!("Failed to parse YAML frontmatter: {}", e))?;

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
        message: body,
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
        let (tx, mut rx) = unbounded_channel::<Result<Event, notify::Error>>();

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
                match std_rx.recv() {
                    Ok(event) => {
                        if watcher_tx.send(event).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Spawn background task to process watcher events
        let workspace_for_watcher = workspace.clone();
        tokio::spawn(async move {
            while let Some(event_result) = rx.recv().await {
                match event_result {
                    Ok(event) => {
                        Self::handle_watcher_event(&jobs, &workspace_for_watcher, event).await;
                    }
                    Err(e) => {
                        warn!("File watcher error: {}", e);
                    }
                }
            }
        });

        service
    }

    /// Handle a single watcher event
    async fn handle_watcher_event(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        _workspace: &Path,
        event: Event,
    ) {
        for path in &event.paths {
            if path.extension().is_some_and(|ext| ext == "md") {
                match event.kind {
                    notify::EventKind::Modify(_) | notify::EventKind::Create(_) => {
                        // Small delay to ensure file is fully written
                        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
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
                    notify::EventKind::Remove(_) => {
                        if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                            jobs.write().remove(id);
                            debug!("Removed cron job: {}", id);
                        }
                    }
                    _ => {}
                }
            }
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

        tokio::fs::write(&file_path, content).await?;

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
    fn test_parse_frontmatter_generic_basic() {
        let content = r#"---
name: Test Job
cron: "0 9 * * *"
---

Hello World"#;

        let (yaml, body) = parse_frontmatter_generic(content).unwrap();
        assert!(yaml.contains("name: Test Job"));
        assert!(yaml.contains("cron:"));
        assert_eq!(body, "Hello World");
    }

    #[test]
    fn test_parse_frontmatter_generic_with_crlf() {
        let content = "---\r\nname: Test\r\ncron: \"0 9 * * *\"\r\n---\r\n\r\nBody content";

        let (yaml, body) = parse_frontmatter_generic(content).unwrap();
        assert!(yaml.contains("name: Test"));
        assert_eq!(body, "Body content");
    }

    #[test]
    fn test_parse_frontmatter_generic_with_code_block() {
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

        let (yaml, body) = parse_frontmatter_generic(content).unwrap();
        assert!(yaml.contains("name: Code Job"));
        assert!(body.contains("---")); // Body should contain the code block
    }

    #[test]
    fn test_parse_frontmatter_generic_missing_start() {
        let content = "No frontmatter here";
        assert!(parse_frontmatter_generic(content).is_err());
    }

    #[test]
    fn test_parse_frontmatter_generic_missing_end() {
        let content = "---\nname: Test\nNo end delimiter";
        assert!(parse_frontmatter_generic(content).is_err());
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
    fn test_cron_job_calculate_next_run() {
        // Test valid cron expression (every minute)
        let next_run = CronJob::calculate_next_run("0 * * * * *");
        // Should have a next_run calculated
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
    }
}
