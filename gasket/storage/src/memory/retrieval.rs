//! Retrieval engine combining tag and embedding search with frequency-boosted scoring.
//!
//! Scoring strategy (no magic numbers):
//! 1. **Tags as hard filter**: If the query has tags, results must match at least one.
//! 2. **Embedding similarity as primary score**: Raw cosine similarity (0.0–1.0).
//! 3. **Frequency as bonus multiplier**: Hot = 1.2x, Warm = 1.1x, Cold = 1.0x.
//!
//! Final score = embedding_similarity × frequency_bonus

use super::embedding_store::EmbeddingStore;
use super::index::FileIndexManager;
use super::types::*;
use anyhow::Result;

/// Frequency bonus multipliers for scoring.
///
/// Hot items get a 20% boost, Warm get 10%, Cold items get no bonus.
/// Archived items are excluded from search entirely.
impl Frequency {
    fn bonus(self) -> f32 {
        match self {
            Frequency::Hot => 1.2,
            Frequency::Warm => 1.1,
            Frequency::Cold => 1.0,
            Frequency::Archived => 0.0, // excluded
        }
    }
}

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
    ///
    /// Tags act as a hard filter (must match at least one). Score is based on
    /// tag match ratio × frequency bonus.
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
                    // Hard filter: exclude archived
                    if matches!(entry.frequency, Frequency::Archived) {
                        continue;
                    }

                    // Hard filter: must match at least one tag
                    let matching = entry
                        .tags
                        .iter()
                        .filter(|t| query_tags.iter().any(|qt| qt.eq_ignore_ascii_case(t)))
                        .count();

                    if matching == 0 {
                        continue;
                    }

                    let tag_score = matching as f32 / query_tags.len().max(1) as f32;
                    let final_score = tag_score * entry.frequency.bonus();

                    results.push(SearchResult {
                        memory_path: entry.filename.clone(),
                        scenario: scen,
                        title: entry.title.clone(),
                        tags: entry.tags.clone(),
                        frequency: entry.frequency,
                        score: final_score,
                        tag_score,
                        embedding_score: 0.0,
                        tokens: entry.tokens,
                        id: entry.id.clone(),
                    });
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

    /// Embedding-only search: cosine similarity with frequency bonus.
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

            let final_score = emb_score * frequency.bonus();

            results.push(SearchResult {
                memory_path: hit.memory_path,
                scenario,
                title: String::new(), // not stored in embedding table
                tags: hit.tags,
                frequency,
                score: final_score,
                tag_score: 0.0,
                embedding_score: emb_score,
                tokens: hit.token_count,
                id: String::new(), // not stored in embedding table
            });
        }
        Ok(results)
    }

    /// Combined search: tag hard-filter + embedding primary score + frequency bonus.
    ///
    /// Strategy:
    /// 1. If query has tags, run tag search as a hard filter to get candidate set
    /// 2. Run embedding search for semantic similarity
    /// 3. Merge: embedding score is primary, frequency provides bonus multiplier
    /// 4. If tags were specified, only include results that matched at least one tag
    pub async fn search(&self, query: &MemoryQuery) -> Result<Vec<SearchResult>> {
        let limit = 20;

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

        // Build tag-matched set for hard filtering
        let tag_matched_keys: std::collections::HashSet<String> = tag_results
            .iter()
            .map(|r| {
                if r.memory_path.is_empty() {
                    format!("{}:{}", r.scenario.dir_name(), r.title)
                } else {
                    format!("{}:{}", r.scenario.dir_name(), r.memory_path)
                }
            })
            .collect();

        // Merge results
        let mut merged: std::collections::HashMap<String, SearchResult> =
            std::collections::HashMap::new();

        // Insert tag results
        for r in tag_results {
            let key = if r.memory_path.is_empty() {
                format!("{}:{}", r.scenario.dir_name(), r.title)
            } else {
                format!("{}:{}", r.scenario.dir_name(), r.memory_path)
            };
            merged.entry(key).or_insert(r);
        }

        // Merge embedding results (embedding score is primary)
        for r in emb_results {
            let key = if r.memory_path.is_empty() {
                format!("{}:{}", r.scenario.dir_name(), r.title)
            } else {
                format!("{}:{}", r.scenario.dir_name(), r.memory_path)
            };

            // If tags were specified and this result doesn't match any tag, skip it
            if !tag_matched_keys.is_empty() && !tag_matched_keys.contains(&key) {
                continue;
            }

            merged
                .entry(key)
                .and_modify(|existing| {
                    existing.embedding_score = r.embedding_score;
                    // Recalculate: embedding primary × frequency bonus
                    existing.score = existing.embedding_score * existing.frequency.bonus();
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
        index_manager.regenerate(Scenario::Knowledge).await.unwrap();

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
        index_manager.regenerate(Scenario::Knowledge).await.unwrap();

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
        assert!((tag_results[0].score - 1.1).abs() < 0.001); // 1.0 * 1.1 (Warm bonus)

        // Test embedding-only search
        let emb_results = engine
            .search_by_embedding("test", &[1.0, 0.0, 0.0], Some(Scenario::Knowledge), 10)
            .await
            .unwrap();
        assert_eq!(emb_results[0].tag_score, 0.0);
        assert!((emb_results[0].embedding_score - 1.0).abs() < 0.001); // cosine = 1.0, normalized
        assert!((emb_results[0].score - 1.1).abs() < 0.001); // 1.0 * 1.1 (Warm bonus)
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
        index_manager.regenerate(Scenario::Knowledge).await.unwrap();

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
        index_manager.regenerate(Scenario::Knowledge).await.unwrap();

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
        let query = MemoryQuery::new().with_tag("test").with_text("test query");
        let results = engine.search(&query).await.unwrap();

        // Should have exactly 1 result (deduplicated)
        assert_eq!(1, results.len());
        assert_eq!(results[0].memory_path, "test.md");
        // Both scores should be present
        assert!(results[0].tag_score > 0.0);
        assert!(results[0].embedding_score > 0.0);
        // Combined score: embedding primary × frequency bonus
        let expected = results[0].embedding_score * results[0].frequency.bonus();
        assert!((results[0].score - expected).abs() < 0.001);
    }
}
