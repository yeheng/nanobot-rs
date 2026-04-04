//! Compaction handler -- triggers context compression when event threshold is reached.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use gasket_storage::EventFilter;
#[allow(unused_imports)] // Needed for dyn EventStoreTrait method calls
use gasket_storage::EventStoreTrait;
use gasket_types::session_event::{EventType, SessionEvent};

use crate::agent::compactor::ContextCompactor;
use crate::agent::materialization::{EventHandler, HandlerContext};

const COMPACTION_EVENT_THRESHOLD: usize = 50;

/// Compaction handler -- wraps existing ContextCompactor.
///
/// Checks session event count after each AssistantMessage.
/// Triggers compression when threshold is exceeded.
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
        // Query current session event count
        let filter = EventFilter {
            session_key: Some(ctx.event.session_key.clone()),
            time_range: None,
            event_types: None,
            event_ids: None,
            limit: None,
            branch: None,
            sequence_after: None,
        };
        let events = ctx.event_store.query_events(&filter).await?;

        if events.len() >= self.threshold && events.len() > 10 {
            let evicted: Vec<_> = events[..events.len() - 10].to_vec();
            let _ = self
                .compactor
                .compact(&ctx.event.session_key, &evicted, &[])
                .await;
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "compaction"
    }
}
