//! Stall detection service for the pipeline.
//!
//! Periodically scans for tasks whose heartbeat has gone silent longer
//! than the configured timeout and emits `PipelineEvent::StallDetected`
//! so the orchestrator can recover.

use std::collections::HashSet;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, error, info};

use super::orchestrator::PipelineEvent;
use super::store::PipelineStore;

/// Interval between stall-detection scans.
const SCAN_INTERVAL_SECS: u64 = 30;

/// Background service that watches for stalled pipeline tasks.
pub struct StallDetector {
    store: PipelineStore,
    event_tx: mpsc::Sender<PipelineEvent>,
    timeout_secs: u64,
    /// The set of states considered "active" for stall detection purposes.
    active_states: HashSet<String>,
}

impl StallDetector {
    pub fn new(
        store: PipelineStore,
        event_tx: mpsc::Sender<PipelineEvent>,
        timeout_secs: u64,
        active_states: HashSet<String>,
    ) -> Self {
        Self {
            store,
            event_tx,
            timeout_secs,
            active_states,
        }
    }

    /// Run the stall-detection loop. Spawn this on a dedicated tokio task.
    pub async fn run(self) {
        info!(
            "Pipeline stall detector started (timeout={}s, interval={}s)",
            self.timeout_secs, SCAN_INTERVAL_SECS
        );

        let mut interval = tokio::time::interval(Duration::from_secs(SCAN_INTERVAL_SECS));

        loop {
            interval.tick().await;
            if let Err(e) = self.scan().await {
                error!("Stall detector scan error: {}", e);
            }
        }
    }

    async fn scan(&self) -> anyhow::Result<()> {
        let stalled = self
            .store
            .find_stalled_tasks(self.timeout_secs, &self.active_states)
            .await?;

        if !stalled.is_empty() {
            debug!("Stall detector found {} stalled task(s)", stalled.len());
        }

        for task in stalled {
            info!("Stall detected for task {} (state={})", task.id, task.state);
            let _ = self
                .event_tx
                .send(PipelineEvent::StallDetected {
                    task_id: task.id.clone(),
                })
                .await;
        }

        Ok(())
    }
}
