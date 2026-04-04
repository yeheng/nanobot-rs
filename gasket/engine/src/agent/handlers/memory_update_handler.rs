//! Memory update handler -- extracts knowledge from user messages.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use gasket_types::session_event::{EventType, SessionEvent};

use crate::agent::materialization::{EventHandler, HandlerContext};
use crate::agent::memory_provider::MemoryProvider;

/// Memory update handler -- wraps MemoryProvider.
///
/// Analyzes UserMessage events and extracts knowledge into memory.
/// Delegates to `MemoryProvider::update_from_event` which has a
/// default no-op implementation for providers that don't support
/// knowledge extraction.
pub struct MemoryUpdateHandler {
    memory: Arc<dyn MemoryProvider>,
}

impl MemoryUpdateHandler {
    pub fn new(memory: Arc<dyn MemoryProvider>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl EventHandler for MemoryUpdateHandler {
    fn can_handle(&self, event: &SessionEvent) -> bool {
        matches!(event.event_type, EventType::UserMessage)
    }

    async fn handle(&self, ctx: &HandlerContext<'_>) -> Result<()> {
        self.memory.update_from_event(ctx.event).await
    }

    fn name(&self) -> &str {
        "memory_update"
    }
}
