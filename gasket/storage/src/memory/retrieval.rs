//! Retrieval engine combining tag and embedding search with normalized scoring.

use super::types::*;
use super::embedding_store::EmbeddingStore;
use super::index::FileIndexManager;
use anyhow::Result;

const TAG_WEIGHT: f32 = 0.4;
const EMBEDDING_WEIGHT: f32 = 0.6;

/// A merged search result with combined scoring.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub memory_path: String,
    pub scenario: Scenario,
    pub title: String,
    pub tags: Vec<String>,
    pub frequency: Frequency,
    pub score: f32,
    pub tag_score: f32,
    pub embedding_score: f32,
    pub tokens: u32,
    pub id: String,
}

/// Retrieval engine combining tag and embedding search.
pub struct RetrievalEngine {
    index_manager: FileIndexManager,
    embedding_store: EmbeddingStore,
}

impl RetrievalEngine {
    pub fn new(index_manager: FileIndexManager, embedding_store: EmbeddingStore) -> Self {
        Self {
            index_manager,
            embedding_store,
        }
    }

    /// Tag-only search: scan _INDEX.md entries for tag matches.
    pub async fn search_by_tags(
        &self,
        query_tags: &[String],
        scenario: Option<Scenario>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let scenarios_to_search = match scenario {
            Some(s) => vec![s],
            None => Scenario::all().to_vec(),
        };

        let mut results = Vec::new();

        for scen in scenarios_to_search {
            // Skip active and profile for tag search (they're always loaded)
            if matches!(scen, Scenario::Active | Scenario::Profile) {
                continue;
            }

            if let Ok(index) = self.index_manager.read_index(scen).await {
                for entry in &index.entries {
                    if matches!(entry.frequency, Frequency::Archived) {
                        continue;
                    }

                    let matching = entry
                        .tags
                        .iter()
                        .filter(|t| query_tags.iter().any(|qt| qt.eq_ignore_ascii_case(t)))
                        .count();

                    if matching > 0 {
                        let tag_score = matching as f32 / query_tags.len().max(1) as f32;
                        results.push(SearchResult {
                            memory_path: entry.filename.clone(),
                            scenario: scen,
                            title: entry.title.clone(),
                            tags: entry.tags.clone(),
                            frequency: entry.frequency,
                            score: tag_score * TAG_WEIGHT,
                            tag_score,
                            embedding_score: 0.0,
                            tokens: entry.tokens,
                            id: entry.id.clone(),
                        });
                    }
                }
            }
        }

        // Sort by score desc, then frequency rank asc
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.frequency.rank().cmp(&b.frequency.rank()))
        });
        results.truncate(limit);
        Ok(results)
    }

    /// Embedding-only search: cosine similarity.
    pub async fn search_by_embedding(
        &self,
        _query: &str,
        query_embedding: &[f32],
        scenario: Option<Scenario>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let hits = self
            .embedding_store
            .search_by_similarity(query_embedding, scenario, limit)
            .await?;

        let mut results = Vec::new();
        for hit in hits {
            let emb_score = (hit.similarity + 1.0) / 2.0; // normalize [-1,1] → [0,1]
            let scenario = Scenario::from_dir_name(&hit.scenario).unwrap_or(Scenario::Knowledge);
            let frequency = Frequency::from_str_lossy(&hit.frequency);

            results.push(SearchResult {
                memory_path: hit.memory_path,
                scenario,
                title: String::new(), // not stored in embedding table
                tags: hit.tags,
                frequency,
                score: emb_score * EMBEDDING_WEIGHT,
                tag_score: 0.0,
                embedding_score: emb_score,
                tokens: hit.token_count,
                id: String::new(), // not stored in embedding table
            });
        }
        Ok(results)
    }

    /// Combined search: merge tag and embedding results with normalized scoring.
    pub async fn search(&self, query: &MemoryQuery) -> Result<Vec<SearchResult>> {
        let limit = 20; // fetch more than needed, truncate later

        // Run both searches
        let tag_results = if !query.tags.is_empty() {
            self.search_by_tags(&query.tags, query.scenario, limit)
                .await?
        } else {
            Vec::new()
        };

        let emb_results = if let Some(text) = &query.text {
            // For now, use a dummy embedding if no embedder is configured
            // Real implementation would call TextEmbedder here
            let dummy_emb = vec![0.0f32; 384]; // TODO: use actual embedder
            self.search_by_embedding(text, &dummy_emb, query.scenario, limit)
                .await?
        } else {
            Vec::new()
        };

        // Merge with normalized scoring
        let mut merged: std::collections::HashMap<String, SearchResult> =
            std::collections::HashMap::new();

        for r in tag_results {
            // Use memory_path as key, empty path uses title as fallback
            let key = if r.memory_path.is_empty() {
                format!("{}:{}", r.scenario.dir_name(), r.title)
            } else {
                format!("{}:{}", r.scenario.dir_name(), r.memory_path)
            };
            let entry = merged.entry(key.clone()).or_insert_with(|| r.clone());
            entry.tag_score = r.tag_score;
            entry.score = entry.tag_score * TAG_WEIGHT + entry.embedding_score * EMBEDDING_WEIGHT;
        }

        for r in emb_results {
            let key = if r.memory_path.is_empty() {
                format!("{}:{}", r.scenario.dir_name(), r.title)
            } else {
                format!("{}:{}", r.scenario.dir_name(), r.memory_path)
            };
            merged
                .entry(key)
                .and_modify(|existing| {
                    existing.embedding_score = r.embedding_score;
                    existing.score =
                        existing.tag_score * TAG_WEIGHT + existing.embedding_score * EMBEDDING_WEIGHT;
                    // Copy title/path from embedding result if we had it
                    if existing.title.is_empty() && !r.title.is_empty() {
                        existing.title = r.title.clone();
                    }
                    if existing.memory_path.is_empty() && !r.memory_path.is_empty() {
                        existing.memory_path = r.memory_path.clone();
                    }
                })
                .or_insert(r);
        }

        let mut results: Vec<SearchResult> = merged.into_values().collect();
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteStore;
    use tempfile::TempDir;

    async fn setup_engine() -> (RetrievalEngine, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().to_path_buf();

        // Create SQLite store for embeddings
        let db_path = base_dir.join("test.db");
        let pool = SqliteStore::with_path(db_path)
            .await
            .unwrap()
            .pool()
            .clone();

        // Create index manager
        let index_manager = FileIndexManager::new(base_dir.join("memory"));

        // Create embedding store
        let embedding_store = EmbeddingStore::new(pool);

        let engine = RetrievalEngine::new(index_manager, embedding_store);

        // Create scenario directories
        for scen in Scenario::all() {
            let dir = base_dir.join("memory").join(scen.dir_name());
            tokio::fs::create_dir_all(dir).await.unwrap();
        }

        (engine, temp_dir)
    }

    #[tokio::test]
    async fn test_tag_search_matches_correctly() {
        let (engine, temp_dir) = setup_engine().await;
        let base_dir = temp_dir.path();

        // Create memory files with different tags
        let knowledge_dir = base_dir.join("memory/knowledge");

        let memory1 = r#"---
id: mem_001
title: Rust Programming
type: note
scenario: knowledge
tags:
  - rust
  - programming
frequency: warm
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 100
---
"#;
        tokio::fs::write(knowledge_dir.join("rust.md"), memory1)
            .await
            .unwrap();

        let memory2 = r#"---
id: mem_002
title: Python Programming
type: note
scenario: knowledge
tags:
  - python
  - programming
frequency: warm
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 100
---
"#;
        tokio::fs::write(knowledge_dir.join("python.md"), memory2)
            .await
            .unwrap();

        // Regenerate index
        let index_manager = FileIndexManager::new(base_dir.join("memory"));
        index_manager
            .regenerate(Scenario::Knowledge)
            .await
            .unwrap();

        // Search by "rust" tag
        let results = engine
            .search_by_tags(&["rust".to_string()], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();

        assert_eq!(1, results.len());
        assert_eq!(results[0].title, "Rust Programming");
        assert!(results[0].tags.contains(&"rust".to_string()));
        assert_eq!(results[0].tag_score, 1.0); // 1/1 tags matched
    }

    #[tokio::test]
    async fn test_merged_scoring_normalizes_correctly() {
        let (engine, temp_dir) = setup_engine().await;
        let base_dir = temp_dir.path();

        // Create memory with embedding
        let knowledge_dir = base_dir.join("memory/knowledge");

        let memory1 = r#"---
id: mem_001
title: Test Memory
type: note
scenario: knowledge
tags:
  - test
  - important
frequency: warm
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 50
---
"#;
        tokio::fs::write(knowledge_dir.join("test.md"), memory1)
            .await
            .unwrap();

        // Regenerate index
        let index_manager = FileIndexManager::new(base_dir.join("memory"));
        index_manager
            .regenerate(Scenario::Knowledge)
            .await
            .unwrap();

        // Add embedding
        let pool = SqliteStore::with_path(base_dir.join("test.db"))
            .await
            .unwrap()
            .pool()
            .clone();
        let emb_store = EmbeddingStore::new(pool);
        emb_store
            .upsert(
                "test.md",
                "knowledge",
                &["test".to_string(), "important".to_string()],
                Frequency::Warm,
                &[1.0, 0.0, 0.0],
                50,
            )
            .await
            .unwrap();

        // Test tag-only search
        let tag_results = engine
            .search_by_tags(&["test".to_string()], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();
        assert_eq!(tag_results[0].tag_score, 1.0);
        assert_eq!(tag_results[0].embedding_score, 0.0);
        assert!((tag_results[0].score - 0.4).abs() < 0.001); // 1.0 * 0.4

        // Test embedding-only search
        let emb_results = engine
            .search_by_embedding("test", &[1.0, 0.0, 0.0], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();
        assert_eq!(emb_results[0].tag_score, 0.0);
        assert!((emb_results[0].embedding_score - 1.0).abs() < 0.001); // cosine = 1.0, normalized
        assert!((emb_results[0].score - 0.6).abs() < 0.001); // 1.0 * 0.6
    }

    #[tokio::test]
    async fn test_search_excludes_archived() {
        let (engine, temp_dir) = setup_engine().await;
        let base_dir = temp_dir.path();

        // Create memories with different frequencies
        let knowledge_dir = base_dir.join("memory/knowledge");

        for (freq, title) in &[
            (Frequency::Warm, "Warm Memory"),
            (Frequency::Archived, "Archived Memory"),
        ] {
            let memory = format!(
                r#"---
id: mem_{}
title: {}
type: note
scenario: knowledge
tags:
  - test
frequency: {}
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 50
---
"#,
                title, title, freq
            );
            let filename = format!("{}.md", title.to_lowercase().replace(' ', "_"));
            tokio::fs::write(knowledge_dir.join(&filename), memory)
                .await
                .unwrap();
        }

        // Regenerate index
        let index_manager = FileIndexManager::new(base_dir.join("memory"));
        index_manager
            .regenerate(Scenario::Knowledge)
            .await
            .unwrap();

        // Search by tag - should only return warm, not archived
        let results = engine
            .search_by_tags(&["test".to_string()], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();

        assert_eq!(1, results.len());
        assert_eq!(results[0].title, "Warm Memory");
        assert_eq!(results[0].frequency, Frequency::Warm);
    }

    #[tokio::test]
    async fn test_search_deduplicates_results() {
        let (engine, temp_dir) = setup_engine().await;
        let base_dir = temp_dir.path();

        // Create memory
        let knowledge_dir = base_dir.join("memory/knowledge");

        let memory1 = r#"---
id: mem_001
title: Test Memory
type: note
scenario: knowledge
tags:
  - test
frequency: warm
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 50
---
"#;
        tokio::fs::write(knowledge_dir.join("test.md"), memory1)
            .await
            .unwrap();

        // Regenerate index
        let index_manager = FileIndexManager::new(base_dir.join("memory"));
        index_manager
            .regenerate(Scenario::Knowledge)
            .await
            .unwrap();

        // Add embedding for the same memory
        let pool = SqliteStore::with_path(base_dir.join("test.db"))
            .await
            .unwrap()
            .pool()
            .clone();
        let emb_store = EmbeddingStore::new(pool);
        emb_store
            .upsert(
                "test.md",
                "knowledge",
                &["test".to_string()],
                Frequency::Warm,
                &[1.0, 0.0],
                50,
            )
            .await
            .unwrap();

        // Combined search should deduplicate
        let query = MemoryQuery::new()
            .with_tag("test")
            .with_text("test query");
        let results = engine.search(&query).await.unwrap();

        // Should have exactly 1 result (deduplicated)
        assert_eq!(1, results.len());
        assert_eq!(results[0].memory_path, "test.md");
        // Both scores should be present
        assert!(results[0].tag_score > 0.0);
        assert!(results[0].embedding_score > 0.0);
        // Combined score should use both weights
        let expected = results[0].tag_score * TAG_WEIGHT
            + results[0].embedding_score * EMBEDDING_WEIGHT;
        assert!((results[0].score - expected).abs() < 0.001);
    }
}
