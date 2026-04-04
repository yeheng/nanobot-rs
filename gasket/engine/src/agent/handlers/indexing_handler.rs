//! Indexing handler -- generates embeddings for events with content.

use async_trait::async_trait;
use gasket_types::session_event::SessionEvent;

use crate::agent::indexing::IndexingService;
use crate::agent::materialization::{EventHandler, HandlerContext};

/// Indexing handler -- wraps existing IndexingService.
///
/// Generates embeddings for all events with non-empty content.
/// Delegates to `IndexingService::index_events` which handles
/// embedding generation and persistence internally.
pub struct IndexingHandler {
    indexing_service: IndexingService,
}

impl IndexingHandler {
    pub fn new(indexing_service: IndexingService) -> Self {
        Self { indexing_service }
    }
}

#[async_trait]
impl EventHandler for IndexingHandler {
    fn can_handle(&self, event: &SessionEvent) -> bool {
        !event.content.is_empty()
    }

    async fn handle(&self, ctx: &HandlerContext<'_>) -> anyhow::Result<()> {
        self.indexing_service
            .index_events(
                &ctx.event.session_key,
                std::slice::from_ref(ctx.event),
            )
            .await;
        Ok(())
    }

    fn name(&self) -> &str {
        "indexing"
    }
}
