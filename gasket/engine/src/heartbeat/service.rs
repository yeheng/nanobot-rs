//! Heartbeat service for proactive wake-up tasks

use std::path::PathBuf;
use std::time::Duration;

use tokio::fs;
use tokio::time::interval;
use tracing::{debug, info};

/// Heartbeat service for periodic task checking
pub struct HeartbeatService {
    workspace: PathBuf,
    interval_secs: u64,
}

impl HeartbeatService {
    /// Create a new heartbeat service with default interval (30 minutes)
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            interval_secs: 1800, // 30 minutes
        }
    }

    /// Create a new heartbeat service with custom interval
    pub fn with_interval(workspace: PathBuf, interval_secs: u64) -> Self {
        Self {
            workspace,
            interval_secs,
        }
    }

    /// Get the workspace path
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Get the heartbeat file path
    fn heartbeat_path(&self) -> PathBuf {
        self.workspace.join("HEARTBEAT.md")
    }

    /// Maximum number of task lines to parse from HEARTBEAT.md.
    const MAX_TASK_LINES: usize = 100;

    /// Read heartbeat tasks.
    ///
    /// Parses pending `- [ ]` lines (up to `MAX_TASK_LINES`), and automatically
    /// compacts the file by removing completed `- [x]` entries when any exist.
    pub async fn read_tasks(&self) -> Vec<String> {
        let path = self.heartbeat_path();
        if !path.exists() {
            return Vec::new();
        }

        let content = match fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to read heartbeat file '{}': {}", path.display(), e);
                return Vec::new();
            }
        };

        let mut pending = Vec::new();
        let mut has_completed = false;
        let mut non_task_lines: Vec<String> = Vec::new(); // header / comment lines

        for line in content.lines().take(Self::MAX_TASK_LINES) {
            let trimmed = line.trim();
            if trimmed.starts_with("- [ ]") {
                if pending.len() < Self::MAX_TASK_LINES {
                    pending.push(trimmed.trim_start_matches("- [ ]").trim().to_string());
                }
            } else if trimmed.starts_with("- [x]") || trimmed.starts_with("- [X]") {
                has_completed = true;
            } else if !trimmed.is_empty() {
                // Preserve non-task lines (headers, comments)
                non_task_lines.push(line.to_string());
            }
        }

        // Auto-compact: rewrite file without completed tasks
        if has_completed {
            self.compact_file(&path, &pending, &non_task_lines).await;
        }

        pending
    }

    /// Rewrite the heartbeat file, keeping only pending tasks and non-task lines.
    async fn compact_file(&self, path: &std::path::Path, pending: &[String], non_task: &[String]) {
        let mut new_content = String::new();
        for line in non_task {
            new_content.push_str(line);
            new_content.push('\n');
        }
        if !new_content.is_empty() && !new_content.ends_with("\n\n") {
            new_content.push('\n');
        }
        for task in pending {
            new_content.push_str("- [ ] ");
            new_content.push_str(task);
            new_content.push('\n');
        }

        match fs::write(path, new_content).await {
            Ok(()) => debug!("Compacted heartbeat file (removed completed tasks)"),
            Err(e) => tracing::warn!("Failed to compact heartbeat file: {}", e),
        }
    }

    /// Run the heartbeat loop
    pub async fn run<F, Fut>(&self, mut callback: F)
    where
        F: FnMut(String) -> Fut + Send,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let mut ticker = interval(Duration::from_secs(self.interval_secs));

        info!(
            "Heartbeat service started (interval: {}s)",
            self.interval_secs
        );

        loop {
            ticker.tick().await;

            let tasks = self.read_tasks().await;
            debug!("Heartbeat check: {} tasks", tasks.len());

            for task in tasks {
                info!("Heartbeat task: {}", task);
                callback(task).await;
            }
        }
    }
}
