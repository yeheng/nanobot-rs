//! Semantic deduplication for wiki ingest.
//!
//! Compares new knowledge against existing wiki pages using text similarity
//! (Phase 2) and embedding-based similarity (Phase 3, requires local-embedding).

use tracing::debug;

use super::extractor::ExtractedItem;
use crate::wiki::page::PageSummary;

/// Result of a deduplication check.
#[derive(Debug, Clone)]
pub struct DedupResult {
    /// Whether the item is a duplicate.
    pub is_duplicate: bool,
    /// Path of the existing page that matches (if any).
    pub existing_path: Option<String>,
    /// Similarity score (0.0-1.0).
    pub similarity: f64,
}

/// Semantic deduplicator for wiki ingest.
///
/// Compares extracted knowledge items against existing wiki pages
/// to avoid creating duplicate pages.
pub struct SemanticDeduplicator {
    /// Similarity threshold above which items are considered duplicates.
    /// Default: 0.85
    threshold: f64,
}

impl SemanticDeduplicator {
    /// Create a new deduplicator with the given threshold.
    pub fn new(threshold: f64) -> Self {
        Self { threshold }
    }

    /// Check if an extracted item duplicates an existing page.
    ///
    /// Uses text-based similarity (title + content overlap) for Phase 2.
    /// Phase 3 will add embedding-based comparison.
    pub fn check(&self, item: &ExtractedItem, existing: &[PageSummary]) -> DedupResult {
        // Check path match first (exact dedup)
        if let Some(ref suggested_path) = item.suggested_path {
            for page in existing {
                if page.path == *suggested_path {
                    debug!("Dedup: exact path match '{}'", suggested_path);
                    return DedupResult {
                        is_duplicate: true,
                        existing_path: Some(page.path.clone()),
                        similarity: 1.0,
                    };
                }
            }
        }

        // Title-based similarity
        let item_title_lower = item.title.to_lowercase();
        for page in existing {
            let page_title_lower = page.title.to_lowercase();

            // Exact title match
            if item_title_lower == page_title_lower {
                debug!(
                    "Dedup: exact title match '{}' == '{}'",
                    item.title, page.title
                );
                return DedupResult {
                    is_duplicate: true,
                    existing_path: Some(page.path.clone()),
                    similarity: 0.95,
                };
            }

            // Title containment check
            if item_title_lower.contains(&page_title_lower)
                || page_title_lower.contains(&item_title_lower)
            {
                let shorter = item_title_lower.len().min(page_title_lower.len());
                let longer = item_title_lower.len().max(page_title_lower.len());
                let score = shorter as f64 / longer as f64;
                if score >= self.threshold {
                    debug!(
                        "Dedup: title containment '{}' ~= '{}' (score {:.2})",
                        item.title, page.title, score
                    );
                    return DedupResult {
                        is_duplicate: true,
                        existing_path: Some(page.path.clone()),
                        similarity: score,
                    };
                }
            }

            // Tag overlap check
            let tag_overlap = self.tag_similarity(&item.tags, &page.tags);
            if tag_overlap > 0.8 && item_title_lower.len() > 3 && page_title_lower.len() > 3 {
                // High tag overlap + similar-ish titles
                let title_sim = self.string_similarity(&item_title_lower, &page_title_lower);
                let combined = tag_overlap * 0.6 + title_sim * 0.4;
                if combined >= self.threshold {
                    debug!(
                        "Dedup: tag+title match '{}' ~= '{}' (combined {:.2})",
                        item.title, page.title, combined
                    );
                    return DedupResult {
                        is_duplicate: true,
                        existing_path: Some(page.path.clone()),
                        similarity: combined,
                    };
                }
            }
        }

        DedupResult {
            is_duplicate: false,
            existing_path: None,
            similarity: 0.0,
        }
    }

    /// Filter a list of extracted items, removing duplicates.
    pub fn filter_duplicates<'a>(
        &self,
        items: &'a [ExtractedItem],
        existing: &[PageSummary],
    ) -> Vec<(&'a ExtractedItem, DedupResult)> {
        items
            .iter()
            .map(|item| {
                let result = self.check(item, existing);
                (item, result)
            })
            .collect()
    }

    /// Return only non-duplicate items.
    pub fn filter_unique<'a>(
        &self,
        items: &'a [ExtractedItem],
        existing: &[PageSummary],
    ) -> Vec<&'a ExtractedItem> {
        items
            .iter()
            .filter(|item| !self.check(item, existing).is_duplicate)
            .collect()
    }

    // ── Helpers ────────────────────────────────────────────────────

    /// Compute tag overlap (Jaccard-like similarity).
    fn tag_similarity(&self, tags_a: &[String], tags_b: &[String]) -> f64 {
        if tags_a.is_empty() || tags_b.is_empty() {
            return 0.0;
        }
        let set_a: std::collections::HashSet<&str> = tags_a.iter().map(|s| s.as_str()).collect();
        let set_b: std::collections::HashSet<&str> = tags_b.iter().map(|s| s.as_str()).collect();
        let intersection = set_a.intersection(&set_b).count();
        let union = set_a.union(&set_b).count();
        if union == 0 {
            return 0.0;
        }
        intersection as f64 / union as f64
    }

    /// Simple trigram-based string similarity.
    fn string_similarity(&self, a: &str, b: &str) -> f64 {
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        let trigrams_a = self.trigrams(a);
        let trigrams_b = self.trigrams(b);
        if trigrams_a.is_empty() || trigrams_b.is_empty() {
            return 0.0;
        }
        let intersection = trigrams_a.intersection(&trigrams_b).count();
        let union = trigrams_a.union(&trigrams_b).count();
        if union == 0 {
            return 0.0;
        }
        intersection as f64 / union as f64
    }

    /// Extract character trigrams from a string.
    fn trigrams(&self, s: &str) -> std::collections::HashSet<String> {
        let chars: Vec<char> = s.chars().collect();
        if chars.len() < 3 {
            return std::collections::HashSet::from([s.to_string()]);
        }
        (0..chars.len() - 2)
            .map(|i| chars[i..i + 3].iter().collect())
            .collect()
    }
}

impl Default for SemanticDeduplicator {
    fn default() -> Self {
        Self::new(0.85)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wiki::page::PageType;
    use chrono::Utc;

    fn make_summary(path: &str, title: &str, tags: Vec<&str>) -> PageSummary {
        PageSummary {
            path: path.to_string(),
            title: title.to_string(),
            page_type: PageType::Topic,
            category: None,
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
            updated: Utc::now(),
            confidence: 1.0,
        }
    }

    fn make_item(title: &str, path: &str, tags: Vec<&str>) -> ExtractedItem {
        ExtractedItem {
            title: title.to_string(),
            item_type: super::super::extractor::ExtractedItemType::Topic,
            content: "test content".to_string(),
            suggested_path: Some(path.to_string()),
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
            confidence: 0.9,
        }
    }

    #[test]
    fn test_exact_path_dedup() {
        let dedup = SemanticDeduplicator::default();
        let existing = vec![make_summary("topics/rust", "Rust", vec![])];
        let item = make_item("Rust Programming", "topics/rust", vec![]);
        let result = dedup.check(&item, &existing);
        assert!(result.is_duplicate);
        assert_eq!(result.existing_path, Some("topics/rust".to_string()));
    }

    #[test]
    fn test_exact_title_dedup() {
        let dedup = SemanticDeduplicator::default();
        let existing = vec![make_summary("topics/rust-lang", "Rust", vec![])];
        let item = make_item("Rust", "topics/rust", vec![]);
        let result = dedup.check(&item, &existing);
        assert!(result.is_duplicate);
    }

    #[test]
    fn test_no_duplicate() {
        let dedup = SemanticDeduplicator::default();
        let existing = vec![make_summary("topics/rust", "Rust", vec![])];
        let item = make_item("Python", "topics/python", vec![]);
        let result = dedup.check(&item, &existing);
        assert!(!result.is_duplicate);
    }

    #[test]
    fn test_filter_unique() {
        let dedup = SemanticDeduplicator::default();
        let existing = vec![make_summary("topics/rust", "Rust", vec![])];
        let items = vec![
            make_item("Rust", "topics/rust", vec![]),     // duplicate
            make_item("Python", "topics/python", vec![]), // unique
            make_item("Go", "topics/go", vec![]),         // unique
        ];
        let unique = dedup.filter_unique(&items, &existing);
        assert_eq!(unique.len(), 2);
        assert_eq!(unique[0].title, "Python");
        assert_eq!(unique[1].title, "Go");
    }

    #[test]
    fn test_tag_similarity() {
        let dedup = SemanticDeduplicator::default();
        let sim = dedup.tag_similarity(
            &["rust".to_string(), "async".to_string()],
            &["rust".to_string(), "systems".to_string()],
        );
        assert!(sim > 0.0 && sim < 1.0); // 1 shared out of 3 unique = 0.33
    }

    #[test]
    fn test_string_similarity_identical() {
        let dedup = SemanticDeduplicator::default();
        let sim = dedup.string_similarity("hello world", "hello world");
        assert_eq!(sim, 1.0);
    }

    #[test]
    fn test_string_similarity_different() {
        let dedup = SemanticDeduplicator::default();
        let sim = dedup.string_similarity("hello world", "foo bar baz");
        assert!(sim < 0.3);
    }

    #[test]
    fn test_empty_existing() {
        let dedup = SemanticDeduplicator::default();
        let item = make_item("Anything", "topics/anything", vec![]);
        let result = dedup.check(&item, &[]);
        assert!(!result.is_duplicate);
    }
}
