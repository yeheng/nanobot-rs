//! Semantic deduplication for wiki ingest.
//!
//! Compares new knowledge against existing wiki pages using text similarity
//! (Phase 2) and embedding-based similarity (Phase 3, requires local-embedding).

use tracing::debug;

use super::extractor::ExtractedItem;
use crate::page::PageSummary;

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
pub struct SemanticDeduplicator;

impl SemanticDeduplicator {
    /// Create a new deduplicator.
    pub fn new() -> Self {
        Self
    }

    /// Check if an extracted item duplicates an existing page.
    ///
    /// Phase 1: exact path match.
    /// Phase 2: exact title match (case-insensitive).
    /// Phase 3 (future): embedding-based semantic similarity.
    pub fn check(&self, item: &ExtractedItem, existing: &[PageSummary]) -> DedupResult {
        // Phase 1: exact path match
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

        // Phase 2: exact title match (case-insensitive)
        let item_title_lower = item.title.to_lowercase();
        for page in existing {
            if page.title.to_lowercase() == item_title_lower {
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
}

impl Default for SemanticDeduplicator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::PageType;
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
            frequency: gasket_storage::wiki::Frequency::Warm,
            access_count: 0,
            last_accessed: None,
            content_length: 0,
            file_mtime: 0,
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
    fn test_empty_existing() {
        let dedup = SemanticDeduplicator::default();
        let item = make_item("Anything", "topics/anything", vec![]);
        let result = dedup.check(&item, &[]);
        assert!(!result.is_duplicate);
    }
}
