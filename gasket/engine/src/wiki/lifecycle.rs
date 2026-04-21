//! Frequency lifecycle management for wiki pages.
//!
//! Ported from the old memory system. Markdown files are never touched —
//! only SQLite runtime state (frequency, access_count, last_accessed) is updated.

use chrono::{DateTime, Utc};
use gasket_storage::wiki::{Frequency, WikiPageStore};

/// Report from a decay batch run.
#[derive(Debug, Default)]
pub struct DecayReport {
    pub total_scanned: usize,
    pub decayed: usize,
    pub errors: usize,
}

/// Frequency decay and promotion logic (wiki port).
pub struct FrequencyManager;

impl FrequencyManager {
    /// Check if a wiki path is exempt from decay.
    ///
    /// Exempt paths:
    /// - `profile/*` and `entities/people/*` (user profile)
    /// - `sops/*` (AI operation manuals)
    /// - `sources/*` (reference material)
    /// - Any path containing `/decisions/` (ADR records)
    pub fn is_exempt_from_decay(path: &str) -> bool {
        path.starts_with("profile/")
            || path.starts_with("entities/people/")
            || path.starts_with("sops/")
            || path.starts_with("sources/")
            || path.contains("/decisions/")
            || path.starts_with("decisions/")
    }

    /// Recalculate frequency based on decay rules.
    ///
    /// # Decay Rules
    /// - hot → warm: 7 days without access
    /// - warm → cold: 30 days without access
    /// - cold → archived: 90 days without access
    /// - Exempt paths are forced to Hot.
    pub fn recalculate(current: Frequency, last_accessed: &str, path: &str) -> Frequency {
        if Self::is_exempt_from_decay(path) {
            return Frequency::Hot;
        }

        let last_accessed_dt = match DateTime::parse_from_rfc3339(last_accessed) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => return current,
        };

        let now = Utc::now();
        let days_since_access = (now - last_accessed_dt).num_days();

        match current {
            Frequency::Hot => {
                if days_since_access > 7 {
                    Frequency::Warm
                } else {
                    Frequency::Hot
                }
            }
            Frequency::Warm => {
                if days_since_access > 30 {
                    Frequency::Cold
                } else {
                    Frequency::Warm
                }
            }
            Frequency::Cold => {
                if days_since_access > 90 {
                    Frequency::Archived
                } else {
                    Frequency::Cold
                }
            }
            Frequency::Archived => Frequency::Archived,
        }
    }

    /// Calculate promotion based on recent access count.
    ///
    /// # Promotion Rules
    /// - cold → warm: on any access
    /// - warm → hot: 3+ accesses in 7 days
    pub fn calculate_promotion(current: Frequency, recent_access_count: u32) -> Frequency {
        match current {
            Frequency::Cold => Frequency::Warm,
            Frequency::Warm => {
                if recent_access_count >= 3 {
                    Frequency::Hot
                } else {
                    Frequency::Warm
                }
            }
            Frequency::Hot | Frequency::Archived => current,
        }
    }

    /// Run batch decay on stale wiki pages, driven by SQLite queries.
    ///
    /// 1. Query `wiki_pages` for candidates whose `last_accessed` is older than 7 days.
    /// 2. For each candidate, recalculate frequency.
    /// 3. If frequency changed, update SQLite only (no file I/O).
    pub async fn run_decay_batch(store: &WikiPageStore) -> anyhow::Result<DecayReport> {
        let mut report = DecayReport::default();

        // Query SQLite for candidates — O(1) disk I/O (index scan)
        let candidates = store.get_decay_candidates(7).await?;
        report.total_scanned = candidates.len();

        for candidate in candidates {
            let current_freq = candidate.frequency;
            let new_freq =
                Self::recalculate(current_freq, &candidate.last_accessed, &candidate.path);

            if new_freq != current_freq {
                match store.update_frequency(&candidate.path, new_freq).await {
                    Ok(true) => {
                        report.decayed += 1;
                        tracing::debug!(
                            "Decayed {}: {:?} -> {:?}",
                            candidate.path,
                            current_freq,
                            new_freq
                        );
                    }
                    Ok(false) => {
                        tracing::warn!(
                            "Decay candidate {} not found in SQLite",
                            candidate.path
                        );
                        report.errors += 1;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to update frequency for {}: {}",
                            candidate.path,
                            e
                        );
                        report.errors += 1;
                    }
                }
            }
        }

        Ok(report)
    }
}
