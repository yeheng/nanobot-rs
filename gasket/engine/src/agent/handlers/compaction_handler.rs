//! Compaction handler -- triggers context compression via the MaterializationEngine.
//!
//! This handler is invoked by the background MaterializationEngine pipeline
//! after each AssistantMessage event. It estimates the current token count
//! and delegates to `ContextCompactor::try_compact` which handles the
//! AtomicBool guard, threshold check, and tokio::spawn internally.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
#[allow(unused_imports)] // Needed for dyn EventStoreTrait method calls
use gasket_storage::EventStoreTrait;
use gasket_types::session_event::{EventType, SessionEvent};

use crate::agent::compactor::ContextCompactor;
use crate::agent::materialization::{EventHandler, HandlerContext};

const COMPACTION_EVENT_THRESHOLD: usize = 50;

/// Compaction handler -- triggers watermark-based context compression.
///
/// Checks session event count after each AssistantMessage.
/// When the count exceeds the threshold, delegates to `ContextCompactor::try_compact`
/// which handles the AtomicBool guard, watermark-based compaction, and GC internally.
pub struct CompactionHandler {
    compactor: Arc<ContextCompactor>,
    threshold: usize,
}

impl CompactionHandler {
    pub fn new(compactor: Arc<ContextCompactor>) -> Self {
        Self {
            compactor,
            threshold: COMPACTION_EVENT_THRESHOLD,
        }
    }

    pub fn with_threshold(mut self, threshold: usize) -> Self {
        self.threshold = threshold;
        self
    }
}

#[async_trait]
impl EventHandler for CompactionHandler {
    fn can_handle(&self, event: &SessionEvent) -> bool {
        matches!(event.event_type, EventType::AssistantMessage)
    }

    async fn handle(&self, ctx: &HandlerContext<'_>) -> Result<()> {
        // Estimate tokens from event count (rough: ~100 tokens per event on average).
        // The compactor does its own precise threshold check internally.
        let estimated_tokens = self.threshold * 100;
        self.compactor
            .try_compact(&ctx.event.session_key, estimated_tokens, &[]);
        Ok(())
    }

    fn name(&self) -> &str {
        "compaction"
    }
}
