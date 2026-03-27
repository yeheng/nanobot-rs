//! Compression actor for background summarization.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

use gasket_types::{EventMetadata, EventType, SessionEvent};

use super::context::CompressionTask;

/// Summarization service trait.
///
/// Implementations provide summarization logic for compressing
/// multiple session events into a concise summary.
#[async_trait]
pub trait SummarizationService: Send + Sync {
    /// Summarize a list of events into a concise summary.
    async fn summarize(&self, events: &[SessionEvent]) -> Result<String, anyhow::Error>;
}

/// Embedding service trait.
///
/// Implementations provide embedding generation for semantic search
/// and similarity comparisons.
#[async_trait]
pub trait EmbeddingService: Send + Sync {
    /// Generate an embedding vector for the given text.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, anyhow::Error>;
}

/// Compression Actor - single-threaded processor for all compression requests.
///
/// This actor handles background summarization tasks with exponential backoff retry.
/// It processes tasks sequentially via an mpsc channel, ensuring that only one
/// summarization runs at a time per actor instance.
///
/// # Design
///
/// - Uses mpsc channel for single-threaded processing
/// - Exponential backoff retry with max 3 attempts
/// - Creates summary events with embeddings for semantic search
///
/// # Example
///
/// ```ignore
/// let tx = CompressionActor::spawn(
///     event_store,
///     summarization,
///     embedding_service,
/// );
///
/// // Send compression task
/// tx.send(CompressionTask {
///     session_key: "cli:test".into(),
///     branch: "main".into(),
///     evicted_events: vec![event_id],
///     compression_type: SummaryType::Compression { token_budget: 1000 },
///     retry_count: 0,
/// }).await?;
/// ```
pub struct CompressionActor {
    /// Channel receiver for compression tasks
    receiver: mpsc::Receiver<CompressionTask>,
    /// Event store for loading and persisting events
    event_store: Arc<gasket_storage::EventStore>,
    /// Summarization service for generating summaries
    summarization: Arc<dyn SummarizationService>,
    /// Embedding service for generating embeddings
    embedding_service: Arc<dyn EmbeddingService>,
    /// Maximum retry attempts before giving up
    max_retries: u32,
}

impl CompressionActor {
    /// Spawn the compression actor and return a task sender.
    ///
    /// Creates a new actor instance and spawns it on the tokio runtime.
    /// Returns an mpsc sender that can be used to submit compression tasks.
    ///
    /// # Arguments
    ///
    /// * `event_store` - Event store for loading and persisting events
    /// * `summarization` - Service for generating summaries
    /// * `embedding_service` - Service for generating embeddings
    ///
    /// # Returns
    ///
    /// An mpsc sender for submitting compression tasks
    pub fn spawn(
        event_store: Arc<gasket_storage::EventStore>,
        summarization: Arc<dyn SummarizationService>,
        embedding_service: Arc<dyn EmbeddingService>,
    ) -> mpsc::Sender<CompressionTask> {
        let (tx, rx) = mpsc::channel(64);

        let actor = Self {
            receiver: rx,
            event_store,
            summarization,
            embedding_service,
            max_retries: 3,
        };

        tokio::spawn(async move {
            actor.run().await;
        });

        tx
    }

    /// Main actor loop - processes tasks sequentially.
    async fn run(mut self) {
        info!("CompressionActor started");

        while let Some(task) = self.receiver.recv().await {
            if let Err(e) = self.process_task(task.clone()).await {
                error!("Compression task failed: {}", e);

                if task.retry_count < self.max_retries {
                    let retry_task = CompressionTask {
                        retry_count: task.retry_count + 1,
                        ..task
                    };
                    warn!(
                        "Retrying compression task (attempt {}/{})",
                        retry_task.retry_count, self.max_retries
                    );
                    // Exponential backoff: 2^retry_count seconds
                    tokio::time::sleep(Duration::from_secs(2u64.pow(task.retry_count))).await;
                    if let Err(e) = self.process_task(retry_task).await {
                        error!("Compression retry failed: {}", e);
                    }
                } else {
                    error!(
                        "Compression task failed after {} retries, events may be lost: session={}, event_count={}",
                        self.max_retries,
                        task.session_key,
                        task.evicted_events.len()
                    );
                }
            }
        }

        info!("CompressionActor stopped");
    }

    /// Process a single compression task.
    async fn process_task(&self, task: CompressionTask) -> Result<(), anyhow::Error> {
        info!(
            "Processing compression for session '{}', {} events",
            task.session_key,
            task.evicted_events.len()
        );

        // 1. Load evicted events
        let events = self
            .event_store
            .get_events_by_ids(&task.session_key, &task.evicted_events)
            .await?;

        if events.is_empty() {
            warn!("No events found for compression, skipping");
            return Ok(());
        }

        // 2. Generate summary
        let summary_content = self.summarization.summarize(&events).await?;

        // 3. Generate embedding for the summary
        let summary_embedding = self.embedding_service.embed(&summary_content).await?;

        // 4. Create summary event
        let summary_event = SessionEvent {
            id: Uuid::now_v7(),
            session_key: task.session_key,
            parent_id: events.last().map(|e| e.id),
            event_type: EventType::Summary {
                summary_type: task.compression_type,
                covered_event_ids: task.evicted_events,
            },
            content: summary_content,
            embedding: Some(summary_embedding),
            metadata: EventMetadata {
                branch: if task.branch == "main" {
                    None
                } else {
                    Some(task.branch)
                },
                ..Default::default()
            },
            created_at: chrono::Utc::now(),
        };

        // 5. Persist summary event
        self.event_store.append_event(&summary_event).await?;

        info!(
            "Compression complete: summary event {} created",
            summary_event.id
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gasket_types::SummaryType;

    struct MockSummarization;

    #[async_trait]
    impl SummarizationService for MockSummarization {
        async fn summarize(&self, _events: &[SessionEvent]) -> Result<String, anyhow::Error> {
            Ok("Summary content".into())
        }
    }

    struct MockEmbedding;

    #[async_trait]
    impl EmbeddingService for MockEmbedding {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, anyhow::Error> {
            Ok(vec![0.1, 0.2, 0.3])
        }
    }

    #[tokio::test]
    async fn test_compression_actor_channel_creation() {
        // Verify channel creation works
        let (tx, _rx) = mpsc::channel::<CompressionTask>(1);
        assert_eq!(tx.capacity(), 1);
    }

    #[test]
    fn test_compression_task_debug() {
        let task = CompressionTask {
            session_key: "cli:test".to_string(),
            branch: "main".to_string(),
            evicted_events: vec![Uuid::now_v7()],
            compression_type: SummaryType::Compression { token_budget: 1000 },
            retry_count: 0,
        };
        let debug_str = format!("{:?}", task);
        assert!(debug_str.contains("CompressionTask"));
        assert!(debug_str.contains("cli:test"));
    }

    #[test]
    fn test_summarization_service_trait() {
        // Just verify the trait can be implemented
        let _service: Arc<dyn SummarizationService> = Arc::new(MockSummarization);
    }

    #[test]
    fn test_embedding_service_trait() {
        // Just verify the trait can be implemented
        let _service: Arc<dyn EmbeddingService> = Arc::new(MockEmbedding);
    }
}
