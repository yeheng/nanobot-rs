//! Background indexing service — consumes `Topic::WikiChanged` events and
//! asynchronously updates the Tantivy search index.
//!
//! This decouples Tantivy I/O from the hot write path, ensuring that
//! `PageStore::write` returns in <10 ms even when the index is large.

use std::sync::Arc;

use gasket_broker::{BrokerError, MemoryBroker, Topic};
use tracing::{debug, error, info, warn};

use super::{PageIndex, PageStore};

/// Background actor that subscribes to `Topic::WikiChanged` and syncs
/// the Tantivy index.
pub struct WikiIndexingService {
    page_store: Arc<PageStore>,
    page_index: Arc<PageIndex>,
}

impl WikiIndexingService {
    pub fn new(page_store: Arc<PageStore>, page_index: Arc<PageIndex>) -> Self {
        Self {
            page_store,
            page_index,
        }
    }

    /// Spawn the service as a background Tokio task.
    ///
    /// Returns a `JoinHandle` that can be awaited during graceful shutdown.
    pub fn spawn(self, broker: Arc<MemoryBroker>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut sub = match broker.subscribe(&Topic::WikiChanged).await {
                Ok(s) => s,
                Err(e) => {
                    error!("WikiIndexingService: failed to subscribe: {}", e);
                    return;
                }
            };

            info!("WikiIndexingService started");

            loop {
                match sub.recv().await {
                    Ok(envelope) => {
                        self.handle_envelope(&envelope.payload).await;
                    }
                    Err(BrokerError::ChannelClosed) => {
                        info!("WikiIndexingService: channel closed, shutting down");
                        break;
                    }
                    Err(BrokerError::Lagged(n)) => {
                        warn!("WikiIndexingService: lagged {} messages, running repair", n);
                        if let Err(e) = self.page_store.repair_index(&self.page_index).await {
                            warn!("WikiIndexingService: repair failed: {}", e);
                        }
                    }
                    Err(e) => {
                        warn!("WikiIndexingService: recv error: {}", e);
                    }
                }
            }
        })
    }

    async fn handle_envelope(&self, payload: &gasket_broker::BrokerPayload) {
        let (path, sync_sequence) = match payload {
            gasket_broker::BrokerPayload::WikiChanged {
                path,
                sync_sequence,
            } => (path, *sync_sequence),
            _ => return,
        };

        // sync_sequence == 0 signals a deletion.
        if sync_sequence == 0 {
            if let Err(e) = self.page_index.delete(path).await {
                warn!("WikiIndexingService: failed to delete {}: {}", path, e);
            } else {
                debug!("WikiIndexingService: deleted {}", path);
            }
            return;
        }

        match self.page_store.read(path).await {
            Ok(page) => {
                if let Err(e) = self.page_index.upsert(&page).await {
                    warn!("WikiIndexingService: failed to upsert {}: {}", path, e);
                } else {
                    debug!("WikiIndexingService: upserted {}", path);
                }
                if let Err(e) = self.page_store.update_indexed_sequence(sync_sequence).await {
                    warn!("WikiIndexingService: failed to update watermark: {}", e);
                }
            }
            Err(e) => {
                warn!("WikiIndexingService: failed to read page {}: {}", path, e);
            }
        }
    }
}
