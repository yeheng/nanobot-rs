//! Hybrid reranking for wiki query results.
//!
//! Task 15: BM25 score-based reranker. Embedding reranker will be added
//! behind `local-embedding` feature flag in a future iteration.
//!
//! Reranking strategy:
//! 1. BM25 score from Tantivy (text relevance)
//! 2. Page confidence (metadata quality signal)
//! 3. Recency boost (fresher pages ranked higher)
//!
//! Future: `local-embedding` adds semantic similarity as an additional signal.

use super::super::page::PageSummary;
use super::tantivy_adapter::SearchHit;

/// A scored candidate for reranking.
#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    /// The search hit from Tantivy.
    pub hit: SearchHit,
    /// Page summary metadata (if available).
    pub summary: Option<PageSummary>,
    /// Combined score after reranking.
    pub score: f64,
}

/// Reranker: combines multiple signals to produce a final ranking.
pub struct Reranker {
    /// Weight for BM25 text relevance score. Default: 0.6
    pub text_weight: f64,
    /// Weight for page confidence. Default: 0.2
    pub confidence_weight: f64,
    /// Weight for recency. Default: 0.2
    pub recency_weight: f64,
}

impl Default for Reranker {
    fn default() -> Self {
        Self {
            text_weight: 0.6,
            confidence_weight: 0.2,
            recency_weight: 0.2,
        }
    }
}

impl Reranker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rerank search hits using BM25 score + confidence + recency.
    /// Takes raw Tantivy hits and optional page summaries for metadata.
    pub fn rerank(&self, hits: Vec<SearchHit>, summaries: &[PageSummary]) -> Vec<ScoredCandidate> {
        let mut candidates: Vec<ScoredCandidate> = hits
            .into_iter()
            .map(|hit| {
                let summary = summaries.iter().find(|s| s.path == hit.path).cloned();
                let score = self.compute_score(&hit, summary.as_ref());
                ScoredCandidate {
                    hit,
                    summary,
                    score,
                }
            })
            .collect();

        // Sort by combined score descending
        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates
    }

    /// Compute combined score for a single candidate.
    fn compute_score(&self, hit: &SearchHit, summary: Option<&PageSummary>) -> f64 {
        // Normalize BM25 score to [0, 1] range
        // Tantivy scores are unbounded, so we use a sigmoid-like normalization
        let bm25_normalized = (hit.score as f64 / (1.0 + hit.score as f64)).min(1.0);

        let confidence = summary.map(|s| s.confidence).unwrap_or(0.5);

        // Recency: more recent pages get higher score
        // Use a simple decay based on age in days
        let recency = summary
            .map(|s| {
                let age = chrono::Utc::now()
                    .signed_duration_since(s.updated)
                    .num_days()
                    .max(0) as f64;
                // Half-life of 30 days: score drops 50% every 30 days
                (-age / 30.0).exp()
            })
            .unwrap_or(0.5);

        self.text_weight * bm25_normalized
            + self.confidence_weight * confidence
            + self.recency_weight * recency
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wiki::page::PageType;
    use chrono::Utc;

    fn make_hit(path: &str, title: &str, score: f32) -> SearchHit {
        SearchHit {
            path: path.to_string(),
            score,
            title: title.to_string(),
        }
    }

    fn make_summary(path: &str, title: &str, confidence: f64) -> PageSummary {
        PageSummary {
            path: path.to_string(),
            title: title.to_string(),
            page_type: PageType::Topic,
            category: None,
            tags: vec![],
            updated: Utc::now(),
            confidence,
            frequency: gasket_storage::wiki::Frequency::Warm,
            access_count: 0,
            last_accessed: None,
        }
    }

    #[test]
    fn test_rerank_sorts_by_combined_score() {
        let reranker = Reranker::default();

        let hits = vec![
            make_hit("topics/a", "Low BM25", 0.5),
            make_hit("topics/b", "High BM25", 5.0),
            make_hit("topics/c", "Medium BM25", 2.0),
        ];

        let summaries = vec![
            make_summary("topics/a", "Low BM25", 0.9),
            make_summary("topics/b", "High BM25", 0.5),
            make_summary("topics/c", "Medium BM25", 0.8),
        ];

        let ranked = reranker.rerank(hits, &summaries);
        assert_eq!(ranked.len(), 3);
        // High BM25 should still rank highest despite lower confidence
        assert_eq!(ranked[0].hit.path, "topics/b");
    }

    #[test]
    fn test_rerank_empty_hits() {
        let reranker = Reranker::default();
        let ranked = reranker.rerank(vec![], &[]);
        assert!(ranked.is_empty());
    }

    #[test]
    fn test_rerank_no_summaries() {
        let reranker = Reranker::default();
        let hits = vec![make_hit("topics/a", "Test", 3.0)];
        let ranked = reranker.rerank(hits, &[]);
        assert_eq!(ranked.len(), 1);
        // Without summaries, confidence=0.5, recency=0.5
        assert!(ranked[0].score > 0.0);
    }

    #[test]
    fn test_rerank_equal_bm25_confidence_wins() {
        let reranker = Reranker::default();

        let hits = vec![
            make_hit("topics/a", "A", 2.0),
            make_hit("topics/b", "B", 2.0),
        ];

        let summaries = vec![
            make_summary("topics/a", "A", 0.3),
            make_summary("topics/b", "B", 0.9),
        ];

        let ranked = reranker.rerank(hits, &summaries);
        // Same BM25 score, B has higher confidence
        assert_eq!(ranked[0].hit.path, "topics/b");
    }
}
