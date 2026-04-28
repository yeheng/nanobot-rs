//! Background indexing service — consumes `Topic::WikiChanged` events and
//! asynchronously updates the Tantivy search index.

use std::sync::Arc;

use gasket_broker::{get_broker, BrokerError, Topic};
use tracing::{debug, error, info, warn};

use super::{PageIndex, PageStore};

/// Background actor that subscribes to `Topic::WikiChanged` and syncs
/// the Tantivy index.
pub struct WikiIndexingService {
    page_store: PageStore,
    page_index: Arc<PageIndex>,
}

impl WikiIndexingService {
    pub fn new(page_store: PageStore, page_index: Arc<PageIndex>) -> Self {
        Self {
            page_store,
            page_index,
        }
    }

    /// Spawn the service as a background Tokio task.
    /// Uses the global broker singleton to subscribe to WikiChanged events.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let broker = get_broker();
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
                        warn!(
                            "WikiIndexingService: lagged {} messages, doing full rebuild",
                            n
                        );
                        if let Err(e) = self.page_index.rebuild(&self.page_store).await {
                            warn!("WikiIndexingService: rebuild failed: {}", e);
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
        let path = match payload {
            gasket_broker::BrokerPayload::WikiChanged { path } => path,
            _ => return,
        };

        // Check if page still exists — if not, it was a deletion.
        match self.page_store.read(path).await {
            Ok(page) => {
                if let Err(e) = self.page_index.upsert(&page).await {
                    warn!("WikiIndexingService: failed to upsert {}: {}", path, e);
                } else {
                    debug!("WikiIndexingService: upserted {}", path);
                }
            }
            Err(_) => {
                // Page doesn't exist anymore — delete from index.
                if let Err(e) = self.page_index.delete(path).await {
                    warn!("WikiIndexingService: failed to delete {}: {}", path, e);
                } else {
                    debug!("WikiIndexingService: deleted {}", path);
                }
            }
        }
    }
}
