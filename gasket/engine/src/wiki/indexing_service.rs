//! Background indexing service — consumes `Topic::WikiChanged` events and
//! asynchronously updates the Tantivy search index.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gasket_broker::{BrokerError, Subscriber};
use gasket_storage::wiki::WikiRelationStore;
use tracing::{debug, info, warn};

use super::{PageIndex, PageStore};

/// Background actor that subscribes to `Topic::WikiChanged` and syncs
/// the Tantivy index and relation store.
pub struct WikiIndexingService {
    page_store: PageStore,
    page_index: Arc<PageIndex>,
    relation_store: WikiRelationStore,
}

impl WikiIndexingService {
    pub fn new(
        page_store: PageStore,
        page_index: Arc<PageIndex>,
        relation_store: WikiRelationStore,
    ) -> Self {
        Self {
            page_store,
            page_index,
            relation_store,
        }
    }

    /// Spawn the service as a background Tokio task.
    /// Consumes a broker `Subscriber` for WikiChanged events — no global singleton.
    pub fn spawn(self, mut sub: Subscriber) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("WikiIndexingService started");

            // Guard to prevent concurrent rebuild tasks from piling up
            // when the consumer falls behind repeatedly.
            let is_rebuilding = Arc::new(AtomicBool::new(false));

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
                            "WikiIndexingService: lagged {} messages, scheduling async rebuild",
                            n
                        );
                        let index = self.page_index.clone();
                        let store = self.page_store.clone();
                        let guard = is_rebuilding.clone();
                        // Only spawn a rebuild if one isn't already running.
                        if !guard.load(Ordering::Relaxed) {
                            tokio::spawn(async move {
                                guard.store(true, Ordering::Relaxed);
                                if let Err(e) = index.rebuild(&store).await {
                                    warn!("WikiIndexingService: async rebuild failed: {}", e);
                                }
                                guard.store(false, Ordering::Relaxed);
                                info!("WikiIndexingService: async rebuild completed");
                            });
                        } else {
                            debug!("WikiIndexingService: rebuild already in progress, skipping");
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
                // Page doesn't exist anymore — delete from index and relations.
                if let Err(e) = self.page_index.delete(path).await {
                    warn!(
                        "WikiIndexingService: failed to delete {} from index: {}",
                        path, e
                    );
                } else {
                    debug!("WikiIndexingService: deleted {} from index", path);
                }
                if let Err(e) = self.relation_store.delete_all_for_page(path).await {
                    warn!(
                        "WikiIndexingService: failed to delete relations for {}: {}",
                        path, e
                    );
                } else {
                    debug!("WikiIndexingService: deleted relations for {}", path);
                }
            }
        }
    }
}
