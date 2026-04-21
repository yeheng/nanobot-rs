//! Tool: search_sops — find relevant SOPs by query string.
//!
//! Queries the wiki index and filters for SOP pages only.
//! Available to the LLM during any turn for self-discovered knowledge retrieval.

use crate::wiki::{PageIndex, PageType};

/// Search the wiki for SOP pages relevant to the given query.
///
/// Returns up to `limit` hits, filtered to `PageType::Sop` only.
pub async fn search_sops(
    page_index: &PageIndex,
    query: &str,
    limit: usize,
) -> anyhow::Result<Vec<crate::wiki::PageSummary>> {
    // Over-fetch to account for filtering, then truncate
    let fetch_limit = limit * 3;
    let results = page_index.search(query, fetch_limit).await?;

    let sops: Vec<_> = results
        .into_iter()
        .filter(|p| p.page_type == PageType::Sop)
        .take(limit)
        .collect();

    Ok(sops)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_search_sops_module_compiles() {
        // search_sops is a thin async wrapper — integration tests cover
        // actual search via wiki_integration tests.
        assert!(true);
    }
}
