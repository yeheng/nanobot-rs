//! Automatic maintenance scheduler.
//!
//! Uses `Arc<IndexManager>` directly without outer RwLock.
//! The IndexManager handles its own internal locking via DashMap.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::index::IndexManager;
use crate::Result;

/// Maintenance scheduler for automatic index maintenance.
pub struct MaintenanceScheduler {
    manager: Arc<IndexManager>,
    config: MaintenanceConfig,
    status: Arc<RwLock<MaintenanceStatus>>,
}

/// Maintenance configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceConfig {
    /// Enable automatic compaction.
    pub auto_compact: bool,
    /// Deleted ratio threshold for auto-compaction.
    pub deleted_ratio_threshold: f32,
    /// Maximum segments before auto-compaction.
    pub max_segments: usize,
    /// Enable automatic expiration.
    pub auto_expire: bool,
    /// Expiration check interval in seconds.
    pub expire_interval_secs: u64,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            auto_compact: true,
            deleted_ratio_threshold: 0.2,
            max_segments: 10,
            auto_expire: true,
            expire_interval_secs: 3600,
        }
    }
}

/// Maintenance status.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MaintenanceStatus {
    /// Last compaction time per index.
    pub last_compaction: HashMap<String, DateTime<Utc>>,
    /// Last expiration time per index.
    pub last_expiration: HashMap<String, DateTime<Utc>>,
    /// Pending maintenance tasks.
    pub pending_tasks: Vec<MaintenanceTask>,
}

/// A pending maintenance task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceTask {
    /// Index name.
    pub index_name: String,
    /// Task type.
    pub task_type: MaintenanceTaskType,
    /// When the task was created.
    pub created_at: DateTime<Utc>,
}

/// Type of maintenance task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceTaskType {
    Compaction,
    Expiration,
}

impl MaintenanceScheduler {
    /// Create a new maintenance scheduler.
    pub fn new(manager: Arc<IndexManager>, config: MaintenanceConfig) -> Self {
        Self {
            manager,
            config,
            status: Arc::new(RwLock::new(MaintenanceStatus::default())),
        }
    }

    /// Start the maintenance scheduler.
    /// Returns the JoinHandle and a CancellationToken to signal shutdown.
    pub fn start(&self) -> (tokio::task::JoinHandle<()>, CancellationToken) {
        let manager = self.manager.clone();
        let config = self.config.clone();
        let status = self.status.clone();
        let cancel_token = CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();

        let handle = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(config.expire_interval_secs));

            loop {
                tokio::select! {
                    _ = cancel_token_clone.cancelled() => {
                        info!("Maintenance scheduler received shutdown signal");
                        break;
                    }
                    _ = interval.tick() => {
                        if config.auto_expire || config.auto_compact {
                            if let Err(e) = run_maintenance(&manager, &config, &status) {
                                error!("Maintenance error: {}", e);
                            }
                        }
                    }
                }
            }

            info!("Maintenance scheduler stopped");
        });

        (handle, cancel_token)
    }

    /// Get current maintenance status.
    pub async fn get_status(&self) -> MaintenanceStatus {
        self.status.read().clone()
    }
}

/// Run maintenance tasks.
///
/// Directly calls IndexManager methods without any outer lock.
/// Each operation manages its own internal locking.
fn run_maintenance(
    manager: &Arc<IndexManager>,
    config: &MaintenanceConfig,
    status: &Arc<RwLock<MaintenanceStatus>>,
) -> Result<()> {
    // Get list of indexes (no lock needed for this read operation)
    let indexes = manager.list_indexes();

    for index_name in indexes {
        // Check if compaction is needed
        if config.auto_compact {
            let needs_compaction = if let Ok(stats) = manager.get_stats(&index_name) {
                let deleted_ratio = stats.deleted_count as f32 / (stats.doc_count as f32 + 1.0);
                deleted_ratio > config.deleted_ratio_threshold
                    || stats.segment_count > config.max_segments
            } else {
                false
            };

            if needs_compaction {
                info!("Auto-compacting index: {}", index_name);
                // compact manages its own internal locking
                if let Err(e) = manager.compact(&index_name) {
                    error!("Compaction failed for {}: {}", index_name, e);
                } else {
                    let mut status = status.write();
                    status
                        .last_compaction
                        .insert(index_name.clone(), Utc::now());
                }
            }
        }

        // Run expiration
        if config.auto_expire {
            info!("Running expiration for index: {}", index_name);
            // TODO: implement expiration
            let mut status = status.write();
            status.last_expiration.insert(index_name, Utc::now());
        }
    }

    Ok(())
}
