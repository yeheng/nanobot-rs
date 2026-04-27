//! Semantic search over stored embeddings.

use std::sync::Arc;

use anyhow::Result;

use crate::index::HnswIndex;
use crate::provider::EmbeddingProvider;
use crate::store::EmbeddingStore;

fn default_top_k() -> usize {
    5
}
fn default_token_budget() -> usize {
    500
}
fn default_min_score() -> f32 {
    0.3
}

/// Configuration for recall search behavior.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecallConfig {
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default = "default_token_budget")]
    pub token_budget: usize,
    #[serde(default = "default_min_score")]
    pub min_score: f32,
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            top_k: default_top_k(),
            token_budget: default_token_budget(),
            min_score: default_min_score(),
        }
    }
}

/// A single recall search result with full content.
pub struct RecallHit {
    pub event_id: String,
    pub session_key: String,
    pub role: String,
    pub content: String,
    pub score: f32,
    pub created_at: String,
}

/// Searches embeddings for semantically similar content.
pub struct RecallSearcher {
    provider: Arc<dyn EmbeddingProvider>,
    index: Arc<HnswIndex>,
    _store: EmbeddingStore,
}

impl RecallSearcher {
    pub fn new(
        provider: Arc<dyn EmbeddingProvider>,
        index: Arc<HnswIndex>,
        store: EmbeddingStore,
    ) -> Self {
        Self {
            provider,
            index,
            _store: store,
        }
    }

    /// Search for similar events. Returns (event_id, score) pairs sorted by descending score.
    pub async fn recall(&self, query: &str, config: &RecallConfig) -> Result<Vec<(String, f32)>> {
        let query_vec = self.provider.embed(query).await?;

        let overfetch = config.top_k * 2;
        let raw = self.index.search(&query_vec, overfetch);

        let results: Vec<(String, f32)> = raw
            .into_iter()
            .filter(|(_, score)| *score >= config.min_score)
            .take(config.top_k)
            .collect();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::MockProvider;
    use crate::store::EmbeddingStore;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_store() -> EmbeddingStore {
        let pool = SqlitePoolOptions::new()
            .connect(":memory:")
            .await
            .expect("in-memory pool");
        let store = EmbeddingStore::new(pool);
        store.run_migration().await.expect("migration");
        store
    }

    #[test]
    fn test_config_defaults() {
        let config = RecallConfig::default();
        assert_eq!(config.top_k, 5);
        assert_eq!(config.token_budget, 500);
        assert!((config.min_score - 0.3f32).abs() < f32::EPSILON);
    }

    #[test]
    fn test_config_deserialize_defaults() {
        let json = "{}";
        let config: RecallConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.top_k, 5);
        assert_eq!(config.token_budget, 500);
        assert!((config.min_score - 0.3f32).abs() < f32::EPSILON);
    }

    #[test]
    fn test_config_deserialize_custom() {
        let json = r#"{"top_k": 10, "token_budget": 1000, "min_score": 0.5}"#;
        let config: RecallConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.top_k, 10);
        assert_eq!(config.token_budget, 1000);
        assert!((config.min_score - 0.5f32).abs() < f32::EPSILON);
    }

    #[test]
    fn test_config_serialize_roundtrip() {
        let config = RecallConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: RecallConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.top_k, config.top_k);
        assert_eq!(parsed.token_budget, config.token_budget);
        assert!((parsed.min_score - config.min_score).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_recall_returns_results() {
        let provider = Arc::new(MockProvider::new(3));
        let index = Arc::new(HnswIndex::new(3));
        let store = test_store().await;

        // Pre-populate index with some entries.
        index.insert("evt-1".into(), vec![1.0, 0.0, 0.0]);
        index.insert("evt-2".into(), vec![0.0, 1.0, 0.0]);
        index.insert("evt-3".into(), vec![0.9, 0.1, 0.0]);

        // Save to store so it's consistent.
        store
            .save(
                "evt-1",
                "sess-a",
                "",
                "",
                &[1.0, 0.0, 0.0],
                "user_message",
                "h1",
            )
            .await
            .unwrap();
        store
            .save(
                "evt-2",
                "sess-a",
                "",
                "",
                &[0.0, 1.0, 0.0],
                "assistant_message",
                "h2",
            )
            .await
            .unwrap();
        store
            .save(
                "evt-3",
                "sess-a",
                "",
                "",
                &[0.9, 0.1, 0.0],
                "user_message",
                "h3",
            )
            .await
            .unwrap();

        let searcher = RecallSearcher::new(provider.clone(), index, store);

        // MockProvider returns zero vectors, so cosine similarity will be 0.0.
        // With min_score=0.0 we should still get results (they match).
        let config = RecallConfig {
            min_score: 0.0,
            ..Default::default()
        };
        let results = searcher.recall("anything", &config).await.unwrap();
        // All entries have zero similarity with the zero query vector,
        // but 0.0 >= 0.0 filter passes them all.
        assert!(
            !results.is_empty(),
            "should find entries with min_score=0.0"
        );
    }

    #[tokio::test]
    async fn test_recall_filters_by_min_score() {
        let provider = Arc::new(MockProvider::new(3));
        let index = Arc::new(HnswIndex::new(3));
        let store = test_store().await;

        index.insert("evt-1".into(), vec![1.0, 0.0, 0.0]);

        let searcher = RecallSearcher::new(provider, index, store);

        // With default min_score=0.3, MockProvider zero vectors produce 0.0 similarity.
        // Nothing should pass the filter.
        let config = RecallConfig {
            min_score: 0.3,
            ..Default::default()
        };
        let results = searcher.recall("anything", &config).await.unwrap();
        assert!(
            results.is_empty(),
            "zero similarity should not pass min_score=0.3"
        );
    }

    #[tokio::test]
    async fn test_recall_respects_top_k() {
        let provider = Arc::new(MockProvider::new(3));
        let index = Arc::new(HnswIndex::new(3));
        let store = test_store().await;

        for i in 0..20 {
            let id = format!("evt-{i}");
            index.insert(id.clone(), vec![1.0, 0.0, 0.0]);
        }

        let searcher = RecallSearcher::new(provider, index, store);
        let config = RecallConfig {
            top_k: 3,
            min_score: 0.0,
            ..Default::default()
        };
        let results = searcher.recall("anything", &config).await.unwrap();
        assert!(results.len() <= 3, "should respect top_k limit");
    }
}
