//! Cron service for scheduled tasks
//!
//! **Hybrid Architecture**:
//! - Config lives in `~/.gasket/cron/*.md` files (SSOT, read-only)
//! - Execution state (last_run/next_run) lives in SQLite `cron_state` table
//!
//! This separation ensures:
//! - Config files remain clean (no runtime mutations)
//! - State survives restarts (missed ticks detection)
//! - High-frequency writes don't wear out SSD with file rewrites

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use cron::Schedule;
use gasket_storage::fs::atomic_write;
use gasket_storage::memory::extract_frontmatter_raw;
use gasket_storage::SqliteStore;
use parking_lot::RwLock;
use serde::Deserialize;
use tracing::{debug, info, instrument, warn};

/// A scheduled job (config from file, state from database)
#[derive(Debug, Clone)]
pub struct CronJob {
    /// Unique job ID (filename without .md)
    pub id: String,
    /// Job name
    pub name: String,
    /// Cron expression
    pub cron: String,
    /// Message to send (for LLM-based jobs)
    pub message: String,
    /// Target channel
    pub channel: Option<String>,
    /// Target chat ID
    pub chat_id: Option<String>,
    /// Tool name to execute directly (bypasses LLM)
    pub tool: Option<String>,
    /// Tool arguments (JSON value)
    pub tool_args: Option<serde_json::Value>,
    /// Last run time (restored from database)
    pub last_run: Option<DateTime<Utc>>,
    /// Next run time (restored from database)
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
    /// Tool name to execute directly (bypasses LLM)
    tool: Option<String>,
    /// Tool arguments as JSON value
    tool_args: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

/// Cached file metadata for change detection
#[derive(Debug, Clone)]
struct FileMetadata {
    mtime: u64,
    size: u64,
}

/// Report from refresh_all_jobs operation
#[derive(Debug, Clone)]
pub struct RefreshReport {
    pub loaded: usize,
    pub updated: usize,
    pub removed: usize,
    pub errors: usize,
}

/// Cron service for scheduled tasks.
///
/// **Hybrid Architecture**:
/// - Config (cron expression, message, tool) lives in `~/.gasket/cron/*.md` files
/// - Execution state (last_run_at, next_run_at) lives in SQLite `cron_state` table
///
/// This separation ensures state survives restarts and enables missed-tick detection.
pub struct CronService {
    /// In-memory job storage (Arc for sharing)
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    /// Workspace path
    workspace: PathBuf,
    /// Cached file metadata for change detection
    file_metadata: Arc<RwLock<HashMap<String, FileMetadata>>>,
    /// SQLite store for execution state persistence
    db: Arc<SqliteStore>,
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
            tool: None,
            tool_args: None,
            last_run: None,
            next_run,
            enabled: true,
            file_path: PathBuf::new(),
            schedule,
        }
    }

    /// Parse cron expression and calculate next run time.
    /// Returns (parsed_schedule, next_run_time).
    fn parse_schedule(cron_expr: &str) -> (Option<Schedule>, Option<DateTime<Utc>>) {
        // Normalize: the `cron` crate requires 7 fields (sec min hour dom month dow year),
        // but users typically provide 5-field standard cron.
        let normalized = {
            let parts: Vec<&str> = cron_expr.split_whitespace().collect();
            match parts.len() {
                5 => format!("0 {} *", cron_expr),
                6 => format!("0 {}", cron_expr),
                _ => cron_expr.to_string(),
            }
        };
        let schedule: Schedule = match normalized.parse() {
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
        tool: fm.tool,
        tool_args: fm.tool_args,
        last_run: None,
        next_run,
        enabled: fm.enabled,
        file_path: file_path.to_path_buf(),
        schedule,
    })
}

impl CronService {
    /// Create a new cron service with hybrid file+database architecture.
    ///
    /// Jobs are loaded from `~/.gasket/cron/*.md` files.
    /// Execution state is restored from SQLite `cron_state` table.
    pub async fn new(workspace: PathBuf, db: Arc<SqliteStore>) -> Self {
        let jobs = Arc::new(RwLock::new(HashMap::new()));
        let file_metadata = Arc::new(RwLock::new(HashMap::new()));

        let service = Self {
            jobs: jobs.clone(),
            workspace: workspace.clone(),
            file_metadata: file_metadata.clone(),
            db,
        };

        // Load existing jobs from cron directory
        service.load_all_jobs(&workspace).await;

        service
    }

    /// Load all cron jobs from markdown files, restoring state from database.
    async fn load_all_jobs(&self, workspace: &Path) {
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
            let ext = path.extension().and_then(|s| s.to_str());

            if ext == Some("md") {
                match self.parse_markdown_file_with_state(&path).await {
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
            info!("Loaded {} cron jobs from files", count);
        }
    }

    /// Parse a markdown file and restore execution state from database.
    async fn parse_markdown_file_with_state(&self, path: &Path) -> anyhow::Result<CronJob> {
        let mut job = Self::parse_markdown_file(path)?;

        // Restore state from database
        match self.db.get_cron_state(&job.id).await {
            Ok(Some((last_run_at, next_run_at))) => {
                // Use database state if available
                if let Some(last_run_str) = last_run_at {
                    if let Ok(last_run) = DateTime::parse_from_rfc3339(&last_run_str) {
                        job.last_run = Some(last_run.with_timezone(&Utc));
                    }
                }
                if let Some(next_run_str) = next_run_at {
                    if let Ok(next_run) = DateTime::parse_from_rfc3339(&next_run_str) {
                        job.next_run = Some(next_run.with_timezone(&Utc));
                    }
                }
                debug!("Restored cron state for {} from database", job.id);
            }
            Ok(None) => {
                // No state in database - initialize with next_run based on now
                if job.next_run.is_none() {
                    // Invalid cron or couldn't parse - job will be disabled
                    warn!("Cron job {} has invalid schedule, disabling", job.id);
                } else {
                    // Persist initial state to database
                    if let Some(next_run) = job.next_run {
                        let _ = self
                            .db
                            .upsert_cron_state(&job.id, None, Some(&next_run.to_rfc3339()))
                            .await;
                    }
                }
            }
            Err(e) => {
                warn!("Failed to load cron state for {}: {}", job.id, e);
            }
        }

        Ok(job)
    }

    /// Refresh all cron jobs from disk, comparing mtime and size to detect changes.
    ///
    /// This is the manual replacement for the file watcher - call this method
    /// when you suspect external file changes may have occurred.
    /// Also performs garbage collection of orphaned database state.
    pub async fn refresh_all_jobs(&self) -> anyhow::Result<RefreshReport> {
        let cron_dir = self.workspace.join("cron");
        if !cron_dir.exists() {
            return Ok(RefreshReport {
                loaded: 0,
                updated: 0,
                removed: 0,
                errors: 0,
            });
        }

        let mut report = RefreshReport {
            loaded: 0,
            updated: 0,
            removed: 0,
            errors: 0,
        };

        // Collect current file IDs from disk
        let mut current_ids = std::collections::HashSet::new();

        for entry in std::fs::read_dir(&cron_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            let ext = path.extension().and_then(|s| s.to_str());

            if ext != Some("md") {
                continue;
            }

            // Read file metadata
            let Ok(metadata) = std::fs::metadata(&path) else {
                report.errors += 1;
                continue;
            };

            let disk_mtime = metadata
                .modified()
                .ok()
                .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let disk_size = metadata.len();

            // Get cached metadata
            let job_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            current_ids.insert(job_id.clone());

            let cached = self.file_metadata.read().get(&job_id).cloned();

            // Skip if mtime and size match (no changes)
            if let Some(cached_meta) = cached {
                if cached_meta.mtime == disk_mtime && cached_meta.size == disk_size {
                    debug!("File unchanged: {}", job_id);
                    continue;
                }
            }

            // Parse and update job (with state restoration from DB)
            match self.parse_markdown_file_with_state(&path).await {
                Ok(job) => {
                    if self.jobs.read().contains_key(&job_id) {
                        report.updated += 1;
                        debug!("Updated cron job: {}", job_id);
                    } else {
                        report.loaded += 1;
                        debug!("Loaded cron job: {}", job_id);
                    }
                    self.jobs.write().insert(job_id.clone(), job);

                    // Cache file metadata
                    self.file_metadata.write().insert(
                        job_id,
                        FileMetadata {
                            mtime: disk_mtime,
                            size: disk_size,
                        },
                    );
                }
                Err(e) => {
                    report.errors += 1;
                    warn!("Failed to parse cron job file {:?}: {}", path, e);
                }
            }
        }

        // Remove jobs for files that no longer exist + clean up database state
        let existing_ids: Vec<String> = self.jobs.read().keys().cloned().collect();
        for id in existing_ids {
            if !current_ids.contains(&id) {
                self.jobs.write().remove(&id);
                self.file_metadata.write().remove(&id);
                // Clean up database state for removed job
                if let Err(e) = self.db.delete_cron_state(&id).await {
                    warn!("Failed to delete cron state for {}: {}", id, e);
                }
                report.removed += 1;
                debug!("Removed stale cron job and state: {}", id);
            }
        }

        Ok(report)
    }

    /// Parse a single markdown cron job file
    fn parse_markdown_file(path: &Path) -> anyhow::Result<CronJob> {
        let content = std::fs::read_to_string(path)?;
        parse_markdown(&content, path)
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

        // Build frontmatter dynamically
        let mut frontmatter_lines = vec![
            format!("name: {}", job.name),
            format!("cron: \"{}\"", job.cron),
            format!("channel: {}", job.channel.as_deref().unwrap_or("")),
            format!("to: {}", job.chat_id.as_deref().unwrap_or("")),
            format!("enabled: {}", job.enabled),
        ];

        // Add tool and tool_args if present
        if let Some(ref tool) = job.tool {
            frontmatter_lines.push(format!("tool: {}", tool));
        }
        if let Some(ref args) = job.tool_args {
            let args_yaml = serde_yaml::to_string(args)?;
            frontmatter_lines.push(format!("tool_args: {}", args_yaml.trim()));
        }

        let content = format!(
            "---\n{}\n---\n\n{}",
            frontmatter_lines.join("\n"),
            job.message
        );

        atomic_write(&file_path, content).await?;

        // IMMEDIATELY update in-memory state for read-after-write consistency
        let job_id = job.id.clone();
        self.jobs.write().insert(job_id.clone(), job);

        info!("Added cron job: {} ({})", job_id, job_id);
        Ok(())
    }

    /// Remove a job (deletes markdown file and database state)
    #[instrument(name = "cron.remove_job", skip(self), fields(job_id = %id))]
    pub async fn remove_job(&self, id: &str) -> anyhow::Result<bool> {
        let cron_dir = self.workspace.join("cron");
        let file_path = cron_dir.join(format!("{}.md", id));

        // Always clean up database state
        if let Err(e) = self.db.delete_cron_state(id).await {
            warn!("Failed to delete cron state for {}: {}", id, e);
        }

        if !file_path.exists() {
            // Also remove from memory if it exists there (stale state)
            let removed_from_memory = self.jobs.write().remove(id).is_some();
            self.file_metadata.write().remove(id);
            return Ok(removed_from_memory);
        }

        // FIRST remove from memory, then delete file
        self.jobs.write().remove(id);
        self.file_metadata.write().remove(id);

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
    #[deprecated(
        since = "2.0.0",
        note = "Use advance_job_tick instead for state persistence"
    )]
    pub async fn update_job_next_run(&self, id: &str, next_run: Option<DateTime<Utc>>) {
        if let Some(job) = self.jobs.write().get_mut(id) {
            job.next_run = next_run;
        }
    }

    /// Advance job execution tick, persist state to database.
    ///
    /// This is the **correct** way to advance a job after execution.
    /// It updates both memory and database state, ensuring:
    /// - last_run_at records when the job actually ran
    /// - next_run_at is calculated from current time (handles missed ticks)
    /// - State survives process restarts
    ///
    /// Returns `(last_run_at, next_run_at)` on success.
    #[instrument(name = "cron.advance_tick", skip_all, fields(job_id = %job_id))]
    pub async fn advance_job_tick(
        &self,
        job_id: &str,
    ) -> anyhow::Result<(DateTime<Utc>, DateTime<Utc>)> {
        let now = Utc::now();

        // Update in-memory state
        let next_run = {
            let mut jobs = self.jobs.write();
            let job = jobs
                .get_mut(job_id)
                .ok_or_else(|| anyhow::anyhow!("Job not found: {}", job_id))?;

            job.last_run = Some(now);

            // Calculate next run based on current time (handles missed ticks)
            let next = if let Some(schedule) = job.schedule.as_ref() {
                schedule.after(&now).next().ok_or_else(|| {
                    anyhow::anyhow!("Failed to calculate next run for job {}", job_id)
                })?
            } else {
                // No valid schedule - job won't run again
                return Err(anyhow::anyhow!("Job {} has no valid schedule", job_id));
            };

            job.next_run = Some(next);
            next
        };

        // Persist to database
        self.db
            .upsert_cron_state(
                job_id,
                Some(&now.to_rfc3339()),
                Some(&next_run.to_rfc3339()),
            )
            .await?;

        debug!(
            "Advanced job {} tick: last_run={}, next_run={}",
            job_id, now, next_run
        );

        Ok((now, next_run))
    }

    /// Check if a job has missed ticks (next_run is in the past).
    ///
    /// Used on startup to detect and compensate for downtime.
    pub async fn has_missed_ticks(&self, job_id: &str) -> anyhow::Result<bool> {
        let jobs = self.jobs.read();
        let job = jobs
            .get(job_id)
            .ok_or_else(|| anyhow::anyhow!("Job not found: {}", job_id))?;

        Ok(job.next_run.is_some_and(|nr| nr <= Utc::now()))
    }

    /// Ensure system-maintained cron jobs exist.
    ///
    /// Creates default system jobs for memory decay, memory refresh, and cron
    /// reload if they are not already present. These jobs bypass LLM and execute
    /// tools directly (zero token cost).
    pub async fn ensure_system_cron_jobs(&self) {
        let system_jobs = [
            (
                "system-memory-decay",
                "Memory Decay",
                "0 0 */6 * * *", // every 6 hours
                Some("memory_decay".to_string()),
                None,
            ),
            (
                "system-memory-refresh",
                "Memory Refresh",
                "0 0 */3 * * *", // every 3 hours
                Some("memory_refresh".to_string()),
                Some(serde_json::json!({"action": "sync"})),
            ),
            (
                "system-cron-refresh",
                "Cron Reload",
                "0 0 * * * *", // every hour
                Some("cron".to_string()),
                Some(serde_json::json!({"action": "refresh"})),
            ),
        ];

        for (id, name, cron_expr, tool, tool_args) in &system_jobs {
            let exists = self.jobs.read().contains_key(*id);
            if exists {
                debug!("System cron job '{}' already exists, skipping", id);
                continue;
            }

            let mut job = CronJob::new(*id, *name, *cron_expr, "system maintenance");
            job.tool = tool.clone();
            job.tool_args = tool_args.clone();

            match self.add_job(job).await {
                Ok(()) => info!("Created system cron job: {} ({})", name, id),
                Err(e) => warn!("Failed to create system cron job '{}': {}", id, e),
            }
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
