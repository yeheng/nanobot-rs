//! Semantic lint checks — LLM-driven, slower.
//!
//! Checks that require LLM analysis:
//! - Contradiction detection (pages making conflicting claims)
//! - Stale claim detection (time-sensitive information that may be outdated)
//!
//! Only runs when `semantic_checks: true` in config and a provider is available.

use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};

use crate::wiki::page::{PageFilter, WikiPage};
use crate::wiki::store::PageStore;

/// A semantic lint issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticIssue {
    /// Issue type.
    pub issue_type: SemanticIssueType,
    /// Pages involved.
    pub pages: Vec<String>,
    /// Human-readable description.
    pub description: String,
    /// Suggested fix (if any).
    pub suggestion: Option<String>,
}

/// Type of semantic issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticIssueType {
    /// Two pages make conflicting claims.
    Contradiction,
    /// Information may be outdated.
    StaleClaim,
}

/// Run semantic lint checks using LLM.
pub async fn run_semantic_lint(
    store: &PageStore,
    provider: &Arc<dyn LlmProvider>,
    model: &str,
) -> Result<Vec<SemanticIssue>> {
    let mut issues = Vec::new();

    let summaries = store.list(PageFilter::default()).await?;
    if summaries.len() < 2 {
        debug!("Semantic lint: fewer than 2 pages, skipping");
        return Ok(issues);
    }

    // Load pages with content
    let mut pages = Vec::new();
    for summary in &summaries {
        if let Ok(page) = store.read(&summary.path).await {
            pages.push(page);
        }
    }

    // Check for contradictions: compare pairs of pages sharing tags
    let contradictions = detect_contradictions(&pages, provider, model).await?;
    issues.extend(contradictions);

    // Check for stale claims
    let stale = detect_stale_claims(&pages, provider, model).await?;
    issues.extend(stale);

    Ok(issues)
}

/// Detect contradictions between pages that share tags or entities.
async fn detect_contradictions(
    pages: &[WikiPage],
    provider: &Arc<dyn LlmProvider>,
    model: &str,
) -> Result<Vec<SemanticIssue>> {
    // Find page pairs that share tags (likely to have overlapping claims)
    let mut pairs_to_check = Vec::new();

    for i in 0..pages.len() {
        for j in (i + 1)..pages.len() {
            let shared_tags: Vec<_> = pages[i]
                .tags
                .iter()
                .filter(|t| pages[j].tags.contains(t))
                .collect();
            if !shared_tags.is_empty() {
                pairs_to_check.push((i, j));
            }
        }
    }

    // Limit to top 10 pairs (LLM calls are expensive)
    pairs_to_check.truncate(10);

    let mut issues = Vec::new();
    for (i, j) in pairs_to_check {
        let page_a = &pages[i];
        let page_b = &pages[j];

        // Skip if content is too long for a single LLM call
        if page_a.content.len() + page_b.content.len() > 8000 {
            continue;
        }

        match check_pair_contradiction(page_a, page_b, provider, model).await {
            Ok(Some(description)) => {
                issues.push(SemanticIssue {
                    issue_type: SemanticIssueType::Contradiction,
                    pages: vec![page_a.path.clone(), page_b.path.clone()],
                    description,
                    suggestion: Some("Review both pages and resolve the conflicting claims.".to_string()),
                });
            }
            Ok(None) => {} // No contradiction found
            Err(e) => {
                warn!("Contradiction check failed for '{}', '{}': {}", page_a.path, page_b.path, e);
            }
        }
    }

    Ok(issues)
}

/// Check a pair of pages for contradictions using LLM.
async fn check_pair_contradiction(
    page_a: &WikiPage,
    page_b: &WikiPage,
    provider: &Arc<dyn LlmProvider>,
    model: &str,
) -> Result<Option<String>> {
    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::system(CONTRADICTION_SYSTEM_PROMPT),
            ChatMessage::user(format!(
                "Page 1 ({}):\n{}\n\nPage 2 ({}):\n{}\n\n\
                 Do these pages contain any contradictory claims? \
                 If yes, describe the contradiction. If no, respond with 'NONE'.",
                page_a.title,
                &page_a.content[..page_a.content.len().min(3000)],
                page_b.title,
                &page_b.content[..page_b.content.len().min(3000)],
            )),
        ],
        tools: None,
        temperature: Some(0.1),
        max_tokens: Some(512),
        thinking: None,
    };

    let response = provider.chat(request).await?;
    let content = response.content.unwrap_or_default();

    if content.trim().eq_ignore_ascii_case("NONE") || content.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(content.trim().to_string()))
    }
}

/// Detect stale claims across pages.
async fn detect_stale_claims(
    pages: &[WikiPage],
    provider: &Arc<dyn LlmProvider>,
    model: &str,
) -> Result<Vec<SemanticIssue>> {
    // Batch check: send multiple pages at once
    let page_descriptions: Vec<String> = pages
        .iter()
        .take(20) // Limit to 20 pages per batch
        .map(|p| {
            format!(
                "- {} ({}): {}",
                p.title,
                p.path,
                &p.content[..p.content.len().min(200)]
            )
        })
        .collect();

    if page_descriptions.is_empty() {
        return Ok(vec![]);
    }

    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::system(STALE_CLAIM_SYSTEM_PROMPT),
            ChatMessage::user(format!(
                "Wiki pages:\n{}\n\n\
                 Which pages contain time-sensitive claims that may be outdated? \
                 Return JSON array: [{{\"path\": \"...\", \"claim\": \"...\"}}] \
                 or [] if none.",
                page_descriptions.join("\n")
            )),
        ],
        tools: None,
        temperature: Some(0.1),
        max_tokens: Some(1024),
        thinking: None,
    };

    let response = provider.chat(request).await?;
    let content = response.content.unwrap_or_default();

    // Parse response
    let issues = parse_stale_response(&content);
    Ok(issues)
}

/// Parse stale claim response from LLM.
fn parse_stale_response(text: &str) -> Vec<SemanticIssue> {
    let trimmed = text.trim();
    let clean = trimmed
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let json_str = if let Some(start) = clean.find('[') {
        if let Some(end) = clean.rfind(']') {
            &clean[start..=end]
        } else {
            return vec![];
        }
    } else {
        return vec![];
    };

    #[derive(serde::Deserialize)]
    struct StaleEntry {
        path: String,
        claim: String,
    }

    match serde_json::from_str::<Vec<StaleEntry>>(json_str) {
        Ok(entries) => entries
            .into_iter()
            .map(|e| SemanticIssue {
                issue_type: SemanticIssueType::StaleClaim,
                pages: vec![e.path],
                description: format!("Potentially stale claim: {}", e.claim),
                suggestion: Some("Review and update if outdated.".to_string()),
            })
            .collect(),
        Err(e) => {
            debug!("Failed to parse stale claim response: {}", e);
            vec![]
        }
    }
}

// ── Prompts ───────────────────────────────────────────────────────

const CONTRADICTION_SYSTEM_PROMPT: &str = r#"You are a wiki consistency checker. Your job is to detect factual contradictions between two wiki pages.

Rules:
1. Only flag genuine factual contradictions (not complementary information)
2. Be specific about what contradicts
3. If no contradiction exists, respond with exactly "NONE"
4. Keep your response concise (1-2 sentences)"#;

const STALE_CLAIM_SYSTEM_PROMPT: &str = r#"You are a wiki freshness checker. Your job is to identify time-sensitive claims that may be outdated.

Look for:
- Version-specific information (e.g., "Rust 1.65 introduces...")
- Temporal claims (e.g., "Currently the latest...", "As of 2024...")
- API or tool references that may have changed

Return a JSON array of objects with "path" and "claim" fields. Return [] if no stale claims found."#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stale_response_empty() {
        let issues = parse_stale_response("[]");
        assert!(issues.is_empty());
    }

    #[test]
    fn test_parse_stale_response_with_entries() {
        let input = r#"[{"path": "topics/rust", "claim": "Rust 1.75 is the latest version"}]"#;
        let issues = parse_stale_response(input);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].pages[0], "topics/rust");
        assert_eq!(issues[0].issue_type, SemanticIssueType::StaleClaim);
    }

    #[test]
    fn test_parse_stale_response_with_fences() {
        let input = "```json\n[{\"path\": \"topics/go\", \"claim\": \"Go 1.21 is current\"}]\n```";
        let issues = parse_stale_response(input);
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn test_parse_stale_response_invalid() {
        let issues = parse_stale_response("not json at all");
        assert!(issues.is_empty());
    }
}
