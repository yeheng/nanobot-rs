use anyhow::Result;

use super::page::PageSummary;

/// PageIndex: search over wiki pages.
/// Phase 1: stub. Phase 3: Tantivy + embedding.
pub struct PageIndex;

impl PageIndex {
    pub fn new() -> Self {
        Self
    }

    /// Phase 1: placeholder. Returns empty results.
    pub async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<PageSummary>> {
        Ok(vec![])
    }
}

impl Default for PageIndex {
    fn default() -> Self {
        Self::new()
    }
}
