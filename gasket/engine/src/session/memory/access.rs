//! Background access log tracker with lock-free MPSC channel.
//!
//! Decoupled from MemoryManager so the write-behind flush lifecycle
//! doesn't pollute the main facade.

use anyhow::Result;
use chrono::Utc;
use gasket_storage::memory::*;
use std::sync::Mutex;
use tracing::{debug, info, warn};

/// Lock-free access tracker with background write-behind flush.
///
/// Records memory accesses via an unbounded MPSC channel. A background
/// worker batches entries and flushes to SQLite when the threshold is reached.
pub(crate) struct AccessTracker {
    access_tx: tokio::sync::mpsc::UnboundedSender<AccessEntry>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    access_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl AccessTracker {
    /// Spawn the background access log worker.
    pub fn new(metadata_store: MetadataStore) -> Self {
        let (access_tx, access_rx) = tokio::sync::mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let handle = tokio::spawn(access_log_worker(access_rx, shutdown_rx, metadata_store));

        Self {
            access_tx,
            shutdown_tx,
            access_task: Mutex::new(Some(handle)),
        }
    }

    /// Record a memory access — lock-free, non-blocking.
    ///
    /// Sends the access entry to the background worker via MPSC channel.
    /// Safe on the hot LLM response path (never blocks or awaits).
    pub fn record(&self, scenario: Scenario, filename: &str) {
        let entry = AccessEntry {
            scenario,
            filename: filename.to_string(),
            timestamp: Utc::now(),
        };
        let _ = self.access_tx.send(entry);
    }

    /// Flush remaining entries on graceful shutdown.
    ///
    /// Sends a shutdown signal, then awaits the worker task to ensure
    /// all data is persisted before returning.
    pub async fn shutdown(&self) -> Result<()> {
        let _ = self.shutdown_tx.send(true);
        let handle = { self.access_task.lock().unwrap().take() };
        if let Some(handle) = handle {
            let _ = handle.await;
        }
        Ok(())
    }
}

// ── Background access log worker ──────────────────────────────────────────

/// Background worker that receives access entries from the MPSC channel,
/// batches them in memory, and flushes to disk when the threshold is reached.
async fn access_log_worker(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<AccessEntry>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    metadata_store: MetadataStore,
) {
    let mut log = AccessLog::default_threshold();

    loop {
        tokio::select! {
            entry = rx.recv() => {
                match entry {
                    Some(entry) => {
                        log.record(entry.scenario, &entry.filename);
                        if log.should_flush() {
                            match FrequencyManager::flush_access_log(
                                &mut log, &metadata_store,
                            )
                            .await
                            {
                                Ok(report) if report.total_flushed > 0 => {
                                    debug!(
                                        "Access log flushed: {} files updated, {} promoted",
                                        report.total_flushed, report.promoted
                                    );
                                }
                                Err(e) => warn!("Access log flush failed: {}", e),
                                _ => {}
                            }
                        }
                    }
                    None => break,
                }
            }
            _ = shutdown_rx.changed() => break,
        }
    }

    // Final flush on shutdown
    if !log.is_empty() {
        info!(
            "Flushing {} remaining access log entries on shutdown",
            log.len()
        );
        if let Err(e) = FrequencyManager::flush_access_log(&mut log, &metadata_store).await {
            warn!("Shutdown flush failed: {}", e);
        }
    }
}
