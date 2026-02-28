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

    /// Read heartbeat tasks
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
        content
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.starts_with("- [ ]") {
                    Some(line.trim_start_matches("- [ ]").trim().to_string())
                } else {
                    None
                }
            })
            .collect()
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
