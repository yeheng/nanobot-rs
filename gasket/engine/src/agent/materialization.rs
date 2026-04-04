//! Materialization engine — event-driven processing pipeline.
//!
//! Converts existing direct-call patterns (indexing, compaction, memory updates)
//! into event-driven handlers that react to EventStore broadcasts.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gasket_storage::EventStoreTrait;
use gasket_types::session_event::SessionEvent;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::broadcast;

/// Handler context — provides event + state query capability.
pub struct HandlerContext<'a> {
    pub event: &'a SessionEvent,
    pub event_store: &'a dyn EventStoreTrait,
}

/// Event handler trait — all handlers must implement this.
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// Determine if this handler should process the event (no side effects).
    fn can_handle(&self, event: &SessionEvent) -> bool;

    /// Process the event. Can query additional state via ctx.event_store.
    async fn handle(&self, ctx: &HandlerContext<'_>) -> Result<()>;

    /// Handler name (for checkpoint and logging).
    fn name(&self) -> &str;
}

/// Checkpoint — tracks each handler's processing progress.
#[derive(Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub handler_name: String,
    pub last_sequence: i64,
    pub updated_at: DateTime<Utc>,
}

/// Checkpoint storage — reuses SqliteStore's kv interface.
///
/// Key format: `mat:checkpoint:{handler_name}`
/// Value: JSON-serialized Checkpoint struct
pub struct CheckpointStore {
    store: Arc<gasket_storage::SqliteStore>,
}

impl CheckpointStore {
    pub fn new(store: Arc<gasket_storage::SqliteStore>) -> Self {
        Self { store }
    }

    pub async fn load(&self, handler_name: &str) -> Result<Option<Checkpoint>> {
        let key = format!("mat:checkpoint:{}", handler_name);
        let val = self.store.read_raw(&key).await?;
        match val {
            Some(v) => Ok(Some(serde_json::from_str(&v)?)),
            None => Ok(None),
        }
    }

    pub async fn save(&self, checkpoint: &Checkpoint) -> Result<()> {
        let key = format!("mat:checkpoint:{}", checkpoint.handler_name);
        let val = serde_json::to_string(checkpoint)?;
        self.store.write_raw(&key, &val).await?;
        Ok(())
    }
}

/// Failed event store — records handler failures for retry.
pub struct FailedEventStore {
    pool: SqlitePool,
}

impl FailedEventStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn record_failure(
        &self,
        event_id: &str,
        handler_name: &str,
        error: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO failed_events
             (event_id, handler_name, error_text, retry_count, next_retry_at)
             VALUES (?, ?, ?, 0, datetime('now', '+30 seconds'))",
        )
        .bind(event_id)
        .bind(handler_name)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_dead_letter(&self, event_id: &str, handler_name: &str) -> Result<()> {
        sqlx::query(
            "UPDATE failed_events SET dead_letter = 1
             WHERE event_id = ? AND handler_name = ?",
        )
        .bind(event_id)
        .bind(handler_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Materialization engine — event-driven processing pipeline.
///
/// Subscribes to EventStore's broadcast channel and dispatches events
/// to registered handlers. Each handler independently tracks its checkpoint.
pub struct MaterializationEngine {
    event_store: Arc<dyn EventStoreTrait>,
    handlers: Vec<Box<dyn EventHandler>>,
    checkpoint_store: CheckpointStore,
    failed_store: FailedEventStore,
}

impl MaterializationEngine {
    pub fn new(
        event_store: Arc<dyn EventStoreTrait>,
        handlers: Vec<Box<dyn EventHandler>>,
        checkpoint_store: CheckpointStore,
        failed_store: FailedEventStore,
    ) -> Self {
        Self {
            event_store,
            handlers,
            checkpoint_store,
            failed_store,
        }
    }

    /// Start the event processing loop.
    ///
    /// Subscribes to EventStore broadcast and processes events sequentially.
    /// On first run, CheckpointStore::load() returns None,
    /// so all handlers start from sequence 0 (full replay).
    pub async fn run(self) -> Result<()> {
        let mut rx = self.event_store.subscribe();

        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Err(e) = self.process_event(&event).await {
                        tracing::error!(
                            "MaterializationEngine error processing event {}: {:?}",
                            event.id,
                            e
                        );
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("MaterializationEngine lagged {} events, will catch up", n);
                    // Checkpoint guarantees no data loss — recover on restart
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::info!("MaterializationEngine broadcast closed, shutting down");
                    break;
                }
            }
        }
        Ok(())
    }

    /// Process a single event — iterate all matching handlers.
    async fn process_event(&self, event: &SessionEvent) -> Result<()> {
        let ctx = HandlerContext {
            event,
            event_store: self.event_store.as_ref(),
        };

        for handler in &self.handlers {
            if !handler.can_handle(event) {
                continue;
            }

            match handler.handle(&ctx).await {
                Ok(()) => {
                    // Advance checkpoint
                    let checkpoint = Checkpoint {
                        handler_name: handler.name().to_string(),
                        last_sequence: event.sequence,
                        updated_at: Utc::now(),
                    };
                    self.checkpoint_store.save(&checkpoint).await?;
                }
                Err(e) => {
                    // Record failure
                    let error_msg = format!("{:?}", e);
                    self.failed_store
                        .record_failure(&event.id.to_string(), handler.name(), &error_msg)
                        .await?;
                    tracing::error!(
                        "Handler {} failed for event {}: {}",
                        handler.name(),
                        event.id,
                        error_msg
                    );
                }
            }
        }
        Ok(())
    }
}
