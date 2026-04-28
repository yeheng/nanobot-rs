//! Semantic search over stored embeddings.

use std::sync::Arc;

use anyhow::Result;
use tracing::info;

use crate::index::MemoryIndex;
use crate::provider::EmbeddingProvider;
use crate::vector_store::VectorStore;

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
///
/// Two-tier architecture:
/// - **Hot index** (memory): recent embeddings for fast queries.
/// - **Cold store** (LanceDB / SQLite): full historical embeddings with ANN search.
pub struct RecallSearcher {
    provider: Arc<dyn EmbeddingProvider>,
    index: Arc<MemoryIndex>,
    store: Arc<dyn VectorStore>,
}

impl RecallSearcher {
    pub fn new(
        provider: Arc<dyn EmbeddingProvider>,
        index: Arc<MemoryIndex>,
        store: Arc<dyn VectorStore>,
    ) -> Self {
        Self {
            provider,
            index,
            store,
        }
    }

    /// Search for similar events. Returns (event_id, score) pairs sorted by descending score.
    ///
    /// Query path:
    /// 1. Search the in-memory hot index (fast).
    /// 2. If the hot index has fewer than `top_k` live entries, search the
    ///    cold store (LanceDB ANN or SQLite brute-force) to fill the gap.
    pub async fn recall(&self, query: &str, config: &RecallConfig) -> Result<Vec<(String, f32)>> {
        info!("Recalling with query: {:?}, config: {:?}", query, config);
        let query_vec = self.provider.embed(query).await?;

        // ── Tier 1: hot index (memory) ──────────────────────────────
        let overfetch = config.top_k * 2;
        let hot_raw = self.index.search(&query_vec, overfetch);
        let mut hot_results: Vec<(String, f32)> = hot_raw
            .into_iter()
            .filter(|(_, score)| *score >= config.min_score)
            .collect();

        // If hot index already has enough results, return early.
        if hot_results.len() >= config.top_k {
            hot_results.truncate(config.top_k);
            return Ok(hot_results);
        }

        // ── Tier 2: cold store (LanceDB ANN or SQLite scan) ────────
        // Build exclusion set so we don't double-count hot results.
        let hot_ids: std::collections::HashSet<String> =
            hot_results.iter().map(|(id, _)| id.clone()).collect();

        let needed = config.top_k - hot_results.len();
        let cold = self
            .store
            .search(&query_vec, needed, config.min_score, &hot_ids)
            .await?;

        if !cold.is_empty() {
            info!(
                "[RecallSearcher] LanceDB cold store returned {} results for query",
                cold.len()
            );
        }

        let cold_tuples: Vec<(String, f32)> = cold.into_iter().map(|r| (r.id, r.score)).collect();

        // Merge hot + cold and re-sort.
        let mut merged = hot_results;
        merged.extend(cold_tuples);
        merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        merged.truncate(config.top_k);

        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::MockProvider;
    use crate::store::EmbeddingStore;
    use crate::vector_store::VectorStore;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_store() -> Arc<dyn VectorStore> {
        let pool = SqlitePoolOptions::new()
            .connect(":memory:")
            .await
            .expect("in-memory pool");
        let store = EmbeddingStore::with_dim(pool, 3);
        store.run_migration().await.expect("migration");
        Arc::new(store)
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
        let index = Arc::new(MemoryIndex::new(3));
        let store = test_store().await;

        // Pre-populate index with some entries.
        index.insert("evt-1".into(), vec![1.0, 0.0, 0.0]);
        index.insert("evt-2".into(), vec![0.0, 1.0, 0.0]);
        index.insert("evt-3".into(), vec![0.9, 0.1, 0.0]);

        // Save to store so it's consistent.
        use crate::vector_store::VectorRecord;
        store
            .upsert(vec![
                VectorRecord {
                    id: "evt-1".to_string(),
                    vector: vec![1.0, 0.0, 0.0],
                    session_key: "sess-a".to_string(),
                    event_type: "user_message".to_string(),
                    content_hash: "h1".to_string(),
                },
                VectorRecord {
                    id: "evt-2".to_string(),
                    vector: vec![0.0, 1.0, 0.0],
                    session_key: "sess-a".to_string(),
                    event_type: "assistant_message".to_string(),
                    content_hash: "h2".to_string(),
                },
                VectorRecord {
                    id: "evt-3".to_string(),
                    vector: vec![0.9, 0.1, 0.0],
                    session_key: "sess-a".to_string(),
                    event_type: "user_message".to_string(),
                    content_hash: "h3".to_string(),
                },
            ])
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
        let index = Arc::new(MemoryIndex::new(3));
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
        let index = Arc::new(MemoryIndex::new(3));
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
