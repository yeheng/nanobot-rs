//! Structural lint checks — fast, no LLM required.
//!
//! Checks that operate purely on page metadata and content:
//! - Orphan pages (no incoming relations, not referenced anywhere)
//! - Stubs / weak pages (content too short)
//! - Missing referenced pages (paths mentioned in content that don't exist)
//! - Naming convention violations

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::wiki::page::{PageFilter, PageType, WikiPage};
use crate::wiki::store::PageStore;

/// A single structural lint issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralIssue {
    /// Issue type.
    pub issue_type: StructuralIssueType,
    /// Page path with the issue.
    pub path: String,
    /// Human-readable description.
    pub description: String,
    /// Severity: low, medium, high.
    pub severity: Severity,
}

/// Type of structural issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuralIssueType {
    /// Page has no incoming relations or references.
    OrphanPage,
    /// Page content is too short (< 50 chars).
    Stub,
    /// Path referenced in content doesn't exist.
    MissingReference,
    /// Path doesn't follow naming conventions.
    NamingViolation,
}

/// Issue severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
}

/// Configuration for structural lint.
#[derive(Debug, Clone)]
pub struct StructuralLintConfig {
    /// Minimum content length to not be a stub. Default: 50 chars.
    pub min_content_length: usize,
    /// Whether to check naming conventions. Default: true.
    pub check_naming: bool,
    /// Whether to detect missing references. Default: true.
    pub check_missing_refs: bool,
}

impl Default for StructuralLintConfig {
    fn default() -> Self {
        Self {
            min_content_length: 50,
            check_naming: true,
            check_missing_refs: true,
        }
    }
}

/// Run all structural lint checks.
pub async fn run_structural_lint(
    store: &PageStore,
    config: &StructuralLintConfig,
) -> anyhow::Result<Vec<StructuralIssue>> {
    let mut issues = Vec::new();

    let summaries = store.list(PageFilter::default()).await?;
    let existing_paths: HashSet<String> = summaries.iter().map(|s| s.path.clone()).collect();

    // Load full pages for content-based checks
    let mut pages = Vec::new();
    for summary in &summaries {
        if let Ok(page) = store.read(&summary.path).await {
            pages.push(page);
        }
    }

    // Build reference graph from page content
    let referenced = build_reference_set(&pages);

    for page in &pages {
        // Check 1: Orphan pages (not referenced by any other page)
        if !referenced.contains(&page.path) && page.page_type != PageType::Source {
            debug!("Orphan page: '{}'", page.path);
            issues.push(StructuralIssue {
                issue_type: StructuralIssueType::OrphanPage,
                path: page.path.clone(),
                description: format!("Page '{}' is not referenced by any other page", page.title),
                severity: Severity::Low,
            });
        }

        // Check 2: Stub detection (content too short)
        if page.content.len() < config.min_content_length {
            debug!("Stub page: '{}' ({} chars)", page.path, page.content.len());
            issues.push(StructuralIssue {
                issue_type: StructuralIssueType::Stub,
                path: page.path.clone(),
                description: format!(
                    "Page '{}' has only {} chars of content (minimum: {})",
                    page.title,
                    page.content.len(),
                    config.min_content_length
                ),
                severity: Severity::Medium,
            });
        }

        // Check 3: Missing references (paths mentioned in content that don't exist)
        if config.check_missing_refs {
            let refs = extract_page_references(&page.content);
            for referent in &refs {
                if !existing_paths.contains(referent) && !referent.is_empty() {
                    debug!("Missing reference: '{}' -> '{}'", page.path, referent);
                    issues.push(StructuralIssue {
                        issue_type: StructuralIssueType::MissingReference,
                        path: page.path.clone(),
                        description: format!("Page references '{}' which does not exist", referent),
                        severity: Severity::Medium,
                    });
                }
            }
        }

        // Check 4: Naming conventions
        if config.check_naming {
            if let Some(issue) = check_naming(page) {
                issues.push(issue);
            }
        }
    }

    // Sort by severity (high first)
    issues.sort_by(|a, b| {
        let sa = match a.severity {
            Severity::High => 0,
            Severity::Medium => 1,
            Severity::Low => 2,
        };
        let sb = match b.severity {
            Severity::High => 0,
            Severity::Medium => 1,
            Severity::Low => 2,
        };
        sa.cmp(&sb)
    });

    Ok(issues)
}

/// Build the set of all paths referenced across all pages.
fn build_reference_set(pages: &[WikiPage]) -> HashSet<String> {
    let mut referenced = HashSet::new();
    for page in pages {
        for referent in extract_page_references(&page.content) {
            referenced.insert(referent);
        }
    }
    referenced
}

/// Extract wiki page references from content.
/// Looks for patterns like `[[entities/projects/gasket]]` or `[[topics/rust]]`.
fn extract_page_references(content: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut in_bracket = false;
    let mut bracket_start = 0;

    let chars: Vec<char> = content.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '[' && chars[i + 1] == '[' {
            in_bracket = true;
            bracket_start = i + 2;
            i += 2;
            continue;
        }
        if in_bracket && i + 1 < chars.len() && chars[i] == ']' && chars[i + 1] == ']' {
            let path: String = chars[bracket_start..i].iter().collect();
            let path = path.trim().to_string();
            if !path.is_empty() {
                refs.push(path);
            }
            in_bracket = false;
            i += 2;
            continue;
        }
        i += 1;
    }
    refs
}

/// Check naming conventions for a page.
fn check_naming(page: &WikiPage) -> Option<StructuralIssue> {
    let expected_prefix = match page.page_type {
        PageType::Entity => "entities/",
        PageType::Topic => "topics/",
        PageType::Source => "sources/",
        PageType::Sop => "sops/",
    };

    if !page.path.starts_with(expected_prefix) {
        return Some(StructuralIssue {
            issue_type: StructuralIssueType::NamingViolation,
            path: page.path.clone(),
            description: format!(
                "Page type '{}' expects path prefix '{}' but path is '{}'",
                page.page_type.as_str(),
                expected_prefix,
                page.path
            ),
            severity: Severity::Low,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_page_references() {
        let content = "See [[topics/rust]] and [[entities/projects/gasket]] for details.";
        let refs = extract_page_references(content);
        assert_eq!(refs, vec!["topics/rust", "entities/projects/gasket"]);
    }

    #[test]
    fn test_extract_page_references_empty() {
        let content = "No references here.";
        let refs = extract_page_references(content);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_extract_page_references_single() {
        let content = "Link: [[topics/async-await]].";
        let refs = extract_page_references(content);
        assert_eq!(refs, vec!["topics/async-await"]);
    }

    #[test]
    fn test_check_naming_valid() {
        let page = WikiPage::new(
            "topics/rust".to_string(),
            "Rust".to_string(),
            PageType::Topic,
            "About Rust.".to_string(),
        );
        assert!(check_naming(&page).is_none());
    }

    #[test]
    fn test_check_naming_invalid() {
        let page = WikiPage::new(
            "wrong/rust".to_string(),
            "Rust".to_string(),
            PageType::Topic,
            "About Rust.".to_string(),
        );
        let issue = check_naming(&page);
        assert!(issue.is_some());
        assert_eq!(
            issue.unwrap().issue_type,
            StructuralIssueType::NamingViolation
        );
    }
}
