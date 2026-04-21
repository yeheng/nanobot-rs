//! Wiki Lint Pipeline — health checking for wiki pages.
//!
//! Two tiers:
//! - **Structural lint** (fast, no LLM): orphan pages, stubs, missing refs, naming
//! - **Semantic lint** (slow, LLM): contradictions, stale claims
//!
//! The linter produces a `LintReport` and can auto-fix simple issues.

pub mod structural;

pub use structural::{Severity, StructuralIssue, StructuralIssueType, StructuralLintConfig};

use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::wiki::store::PageStore;

/// Complete lint report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintReport {
    /// Structural issues (fast checks).
    pub structural: Vec<StructuralIssue>,
    /// Total pages checked.
    pub pages_checked: usize,
}

impl LintReport {
    /// Total number of issues found.
    pub fn total_issues(&self) -> usize {
        self.structural.len()
    }

    /// Whether any high-severity issues exist.
    pub fn has_high_severity(&self) -> bool {
        self.structural.iter().any(|i| i.severity == Severity::High)
    }

    /// Summarize as a human-readable string.
    pub fn summary(&self) -> String {
        let mut out = format!(
            "Lint report: {} pages checked, {} issues found\n",
            self.pages_checked,
            self.total_issues()
        );

        if !self.structural.is_empty() {
            out.push_str(&format!(
                "\nStructural ({} issues):\n",
                self.structural.len()
            ));
            for issue in &self.structural {
                let sev = match issue.severity {
                    Severity::High => "HIGH",
                    Severity::Medium => "MED",
                    Severity::Low => "LOW",
                };
                out.push_str(&format!(
                    "  [{}] {} — {}\n",
                    sev, issue.path, issue.description
                ));
            }
        }

        out
    }
}

/// What was auto-fixed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixReport {
    /// Number of stub pages expanded.
    pub stubs_expanded: usize,
    /// Number of missing reference stubs created.
    pub missing_refs_created: usize,
    /// Number of naming fixes applied.
    pub naming_fixes: usize,
}

impl FixReport {
    pub fn total_fixes(&self) -> usize {
        self.stubs_expanded + self.missing_refs_created + self.naming_fixes
    }
}

/// Wiki linter — runs structural checks only.
pub struct WikiLinter {
    store: Arc<PageStore>,
    structural_config: StructuralLintConfig,
}

impl WikiLinter {
    /// Create a new linter with structural checks only.
    pub fn new(store: Arc<PageStore>) -> Self {
        Self {
            store,
            structural_config: StructuralLintConfig::default(),
        }
    }

    /// Run lint checks and produce a report.
    pub async fn lint(&self) -> Result<LintReport> {
        info!("Running structural lint...");
        let structural =
            structural::run_structural_lint(&self.store, &self.structural_config).await?;

        let summaries = self.store.list(Default::default()).await?;
        let pages_checked = summaries.len();

        let report = LintReport {
            structural,
            pages_checked,
        };

        info!(
            "Lint complete: {} pages, {} issues",
            pages_checked,
            report.total_issues()
        );

        Ok(report)
    }

    /// Auto-fix simple issues.
    pub async fn auto_fix(&self, report: &LintReport) -> Result<FixReport> {
        let mut fix_report = FixReport {
            stubs_expanded: 0,
            missing_refs_created: 0,
            naming_fixes: 0,
        };

        // Auto-fix: create stubs for missing references
        for issue in &report.structural {
            if issue.issue_type == StructuralIssueType::MissingReference {
                // Extract the missing path from the description
                if let Some(missing_path) = extract_missing_path(&issue.description) {
                    // Create a placeholder page
                    let page = crate::wiki::page::WikiPage::new(
                        missing_path.clone(),
                        missing_path.clone(),
                        crate::wiki::page::PageType::Topic,
                        "This page was auto-created as a placeholder. Please add content."
                            .to_string(),
                    );
                    match self.store.write(&page).await {
                        Ok(_) => {
                            fix_report.missing_refs_created += 1;
                            tracing::info!(
                                "Auto-created stub for missing page: '{}'",
                                missing_path
                            );
                        }
                        Err(e) => {
                            tracing::warn!("Failed to create stub '{}': {}", missing_path, e);
                        }
                    }
                }
            }
        }

        Ok(fix_report)
    }
}

/// Extract a missing page path from a lint issue description.
fn extract_missing_path(description: &str) -> Option<String> {
    // Pattern: "Page references 'X' which does not exist"
    if let Some(start) = description.find("'") {
        if let Some(end) = description[start + 1..].find("'") {
            return Some(description[start + 1..start + 1 + end].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lint_report_summary() {
        let report = LintReport {
            structural: vec![StructuralIssue {
                issue_type: StructuralIssueType::OrphanPage,
                path: "topics/rust".to_string(),
                description: "Not referenced".to_string(),
                severity: Severity::Low,
            }],
            pages_checked: 10,
        };
        let summary = report.summary();
        assert!(summary.contains("10 pages checked"));
        assert!(summary.contains("1 issues found"));
        assert!(summary.contains("Not referenced"));
    }

    #[test]
    fn test_lint_report_total() {
        let report = LintReport {
            structural: vec![],
            pages_checked: 5,
        };
        assert_eq!(report.total_issues(), 0);
        assert!(!report.has_high_severity());
    }

    #[test]
    fn test_extract_missing_path() {
        let desc = "Page references 'topics/missing' which does not exist";
        let path = extract_missing_path(desc);
        assert_eq!(path, Some("topics/missing".to_string()));
    }

    #[test]
    fn test_extract_missing_path_none() {
        let desc = "Some other description";
        let path = extract_missing_path(desc);
        assert!(path.is_none());
    }

    #[test]
    fn test_fix_report_total() {
        let report = FixReport {
            stubs_expanded: 1,
            missing_refs_created: 2,
            naming_fixes: 0,
        };
        assert_eq!(report.total_fixes(), 3);
    }
}
