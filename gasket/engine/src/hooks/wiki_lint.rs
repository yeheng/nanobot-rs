//! Wiki Lint Hook — periodic wiki health checking.
//!
//! Runs on a configurable interval (default 24h). Calls WikiLinter,
//! auto-fixes simple issues, and logs the report.

use std::sync::Arc;

use chrono::Utc;
use tracing::{info, warn};

use crate::wiki::lint::{LintReport, WikiLinter};

/// Wiki lint hook — runs periodic lint checks.
pub struct WikiLintHook {
    linter: Arc<WikiLinter>,
    auto_fix: bool,
    interval_hours: u64,
    last_run: Arc<parking_lot::Mutex<Option<chrono::DateTime<Utc>>>>,
}

impl WikiLintHook {
    /// Create a new lint hook.
    pub fn new(linter: Arc<WikiLinter>, auto_fix: bool, interval_hours: u64) -> Self {
        Self {
            linter,
            auto_fix,
            interval_hours,
            last_run: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    /// Check if the lint should run now based on the interval.
    pub fn should_run(&self) -> bool {
        let last = self.last_run.lock();
        match *last {
            None => true,
            Some(last_time) => {
                let elapsed = Utc::now().signed_duration_since(last_time).num_hours();
                elapsed as u64 >= self.interval_hours
            }
        }
    }

    /// Run the lint check. Returns the report.
    pub async fn run(&self) -> anyhow::Result<LintReport> {
        info!("WikiLintHook: running periodic lint check");

        let report = self.linter.lint().await?;

        // Auto-fix if enabled
        if self.auto_fix && report.total_issues() > 0 {
            match self.linter.auto_fix(&report).await {
                Ok(fix_report) => {
                    if fix_report.total_fixes() > 0 {
                        info!(
                            "WikiLintHook: auto-fixed {} issues",
                            fix_report.total_fixes()
                        );
                    }
                }
                Err(e) => {
                    warn!("WikiLintHook: auto-fix failed: {}", e);
                }
            }
        }

        // Update last run time
        *self.last_run.lock() = Some(Utc::now());

        Ok(report)
    }

    /// Force run regardless of interval.
    pub async fn force_run(&self) -> anyhow::Result<LintReport> {
        let report = self.linter.lint().await?;

        if self.auto_fix && report.total_issues() > 0 {
            let _ = self.linter.auto_fix(&report).await;
        }

        *self.last_run.lock() = Some(Utc::now());
        Ok(report)
    }

    /// Get the interval in hours.
    pub fn interval_hours(&self) -> u64 {
        self.interval_hours
    }
}

/// Parse an interval string like "24h", "12h", "1h" into hours.
pub fn parse_interval(interval: &str) -> Option<u64> {
    let interval = interval.trim();
    if interval.ends_with('h') {
        interval[..interval.len() - 1].parse().ok()
    } else if interval.ends_with('d') {
        interval[..interval.len() - 1]
            .parse::<u64>()
            .ok()
            .map(|d| d * 24)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_interval_hours() {
        assert_eq!(parse_interval("24h"), Some(24));
        assert_eq!(parse_interval("12h"), Some(12));
        assert_eq!(parse_interval("1h"), Some(1));
    }

    #[test]
    fn test_parse_interval_days() {
        assert_eq!(parse_interval("1d"), Some(24));
        assert_eq!(parse_interval("7d"), Some(168));
    }

    #[test]
    fn test_parse_interval_invalid() {
        assert_eq!(parse_interval("abc"), None);
        assert_eq!(parse_interval(""), None);
    }
}
