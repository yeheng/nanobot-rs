//! Three-phase memory loading with relevance scoring.
//!
//! `MemoryLoader` owns the read path — budget-aware context loading,
//! semantic search, and access tracking. It composes a `RetrievalEngine`
//! for tag/embedding queries and an `AccessTracker` for write-behind
//! frequency updates.

use anyhow::Result;
use gasket_storage::memory::*;
use std::collections::HashSet;
use tracing::warn;

use super::access::AccessTracker;
use super::types::{MemoryContext, PhaseBreakdown};

/// Read-side memory engine with three-phase context loading.
///
/// Phases:
/// 1. **Bootstrap** — Profile + Active hot/warm (always loaded)
/// 2. **Scenario** — current scenario hot + tag-matched warm
/// 3. **On-demand** — semantic/tag search fill
///
/// Total never exceeds `budget.total_cap`.
pub(crate) struct MemoryLoader {
    store: FileMemoryStore,
    metadata_store: MetadataStore,
    embedding_store: EmbeddingStore,
    retrieval: RetrievalEngine,
    budget: TokenBudget,
    access: AccessTracker,
}

impl MemoryLoader {
    /// Assemble from pre-built components.
    pub fn new(
        store: FileMemoryStore,
        metadata_store: MetadataStore,
        embedding_store: EmbeddingStore,
        retrieval: RetrievalEngine,
        budget: TokenBudget,
        access: AccessTracker,
    ) -> Self {
        Self {
            store,
            metadata_store,
            embedding_store,
            retrieval,
            budget,
            access,
        }
    }

    /// Replace the token budget (builder-style).
    pub fn with_budget(mut self, budget: TokenBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Shut down the background access tracker.
    pub async fn shutdown(&self) -> Result<()> {
        self.access.shutdown().await
    }

    // ── Public read API ─────────────────────────────────────────────────

    /// Three-phase memory loading for context injection.
    pub async fn load_for_context(&self, query: &MemoryQuery) -> Result<MemoryContext> {
        let scenario = query.scenario.unwrap_or(Scenario::Knowledge);
        let mut seen = HashSet::new();
        let mut memories = Vec::new();
        let mut phase = PhaseBreakdown::default();

        // Phase 1: Bootstrap
        let bootstrap_candidates = self.collect_bootstrap_candidates().await?;
        let bootstrap_cap = self.budget.bootstrap.min(self.budget.total_cap);
        phase.bootstrap_tokens = self
            .load_results(
                &bootstrap_candidates,
                bootstrap_cap,
                &mut seen,
                &mut memories,
            )
            .await;

        // Phase 2: Scenario-specific
        let scenario_candidates = self
            .collect_scenario_candidates(scenario, &query.tags)
            .await?;
        let scenario_cap = self
            .budget
            .scenario
            .min(self.budget.total_cap.saturating_sub(phase.bootstrap_tokens));
        phase.scenario_tokens = self
            .load_results(&scenario_candidates, scenario_cap, &mut seen, &mut memories)
            .await;

        // Phase 3: On-demand semantic search
        let on_demand_cap = self.budget.on_demand.min(
            self.budget
                .total_cap
                .saturating_sub(phase.bootstrap_tokens + phase.scenario_tokens),
        );
        phase.on_demand_tokens = self
            .load_on_demand(query, on_demand_cap, &mut seen, &mut memories)
            .await;

        let tokens_used = phase.bootstrap_tokens + phase.scenario_tokens + phase.on_demand_tokens;

        Ok(MemoryContext {
            memories,
            tokens_used,
            phase_breakdown: phase,
        })
    }

    /// Semantic search across memories with real relevance scores.
    ///
    /// - Non-empty query → RetrievalEngine returns results with real scores
    /// - Empty query → enumerate all metadata entries (stats mode)
    pub async fn search(&self, query: &str, top_k: usize) -> Result<Vec<MemoryHit>> {
        if query.is_empty() {
            return self.enumerate_all(top_k).await;
        }

        let memory_query = MemoryQuery::new()
            .with_text(query)
            .with_max_tokens(top_k * 200);

        let results = match self.retrieval.search(&memory_query).await {
            Ok(r) => r,
            Err(_) => return Ok(vec![]),
        };

        Ok(results
            .into_iter()
            .take(top_k)
            .map(|r| MemoryHit {
                path: format!("{}/{}", r.scenario.dir_name(), r.memory_path),
                scenario: r.scenario,
                title: r.title,
                tags: r.tags,
                frequency: r.frequency,
                score: r.score,
                tokens: r.tokens as usize,
            })
            .collect())
    }

    // ── Phase collectors ────────────────────────────────────────────────

    /// Phase 1: all Profile + Active hot/warm.
    async fn collect_bootstrap_candidates(&self) -> Result<Vec<SearchResult>> {
        let mut candidates = Vec::new();

        let profile_entries = self.metadata_store.query_entries(Scenario::Profile).await?;
        candidates.extend(profile_entries.into_iter().map(SearchResult::from));

        let active_entries = self.metadata_store.query_entries(Scenario::Active).await?;
        candidates.extend(
            active_entries
                .into_iter()
                .filter(|e| matches!(e.frequency, Frequency::Hot | Frequency::Warm))
                .map(SearchResult::from),
        );

        candidates.sort();
        Ok(candidates)
    }

    /// Phase 2: hot always + tag-matched warm for the given scenario.
    async fn collect_scenario_candidates(
        &self,
        scenario: Scenario,
        tags: &[String],
    ) -> Result<Vec<SearchResult>> {
        let entries = self.metadata_store.query_entries(scenario).await?;

        let mut candidates: Vec<SearchResult> = entries
            .into_iter()
            .filter(|e| match e.frequency {
                Frequency::Hot => true,
                Frequency::Warm if tags.is_empty() => true,
                Frequency::Warm => e
                    .tags
                    .iter()
                    .any(|t| tags.iter().any(|qt| qt.eq_ignore_ascii_case(t))),
                _ => false,
            })
            .map(SearchResult::from)
            .collect();

        candidates.sort();
        Ok(candidates)
    }

    /// Phase 3: on-demand semantic/tag search fill.
    async fn load_on_demand(
        &self,
        query: &MemoryQuery,
        budget: usize,
        seen: &mut HashSet<String>,
        memories: &mut Vec<MemoryFile>,
    ) -> usize {
        if budget == 0 || (query.text.is_none() && query.tags.is_empty()) {
            return 0;
        }

        let Ok(results) = self.retrieval.search(query).await else {
            return 0;
        };

        self.load_results(&results, budget, seen, memories).await
    }

    // ── Shared loading core ─────────────────────────────────────────────

    /// Load results within budget, deduplicating via seen set.
    ///
    /// Stale entries (file deleted but SQLite row remains) are cleaned up
    /// inline — no separate method needed.
    async fn load_results(
        &self,
        results: &[SearchResult],
        budget: usize,
        seen: &mut HashSet<String>,
        memories: &mut Vec<MemoryFile>,
    ) -> usize {
        let mut tokens_used = 0usize;
        for r in results {
            if tokens_used + r.tokens as usize > budget {
                break;
            }
            let key = format!("{}/{}", r.scenario.dir_name(), r.memory_path);
            if !seen.insert(key) {
                continue;
            }
            match self.store.read(r.scenario, &r.memory_path).await {
                Ok(mem) => {
                    tokens_used += mem.metadata.tokens;
                    self.access.record(r.scenario, &r.memory_path);
                    memories.push(mem);
                }
                Err(e) => {
                    // Inline stale cleanup: if file is gone from disk, nuke the
                    // SQLite row + embedding so we don't keep retrying it.
                    if Self::is_not_found(&e) {
                        warn!(
                            "File gone from disk, cleaning stale entry: {}/{}",
                            r.scenario.dir_name(),
                            r.memory_path
                        );
                        let _ = self
                            .metadata_store
                            .delete_by_scenario_and_path(r.scenario, &r.memory_path)
                            .await;
                        let _ = self
                            .embedding_store
                            .delete(&format!("{}/{}", r.scenario.dir_name(), r.memory_path))
                            .await;
                    } else {
                        warn!(
                            "Failed to load {}/{}: {}",
                            r.scenario.dir_name(),
                            r.memory_path,
                            e
                        );
                    }
                }
            }
        }
        tokens_used
    }

    // ── Private helpers ─────────────────────────────────────────────────

    /// Enumerate all metadata entries for stats (empty-query fallback).
    async fn enumerate_all(&self, top_k: usize) -> Result<Vec<MemoryHit>> {
        let mut hits = Vec::new();
        for scenario in Scenario::all() {
            let entries = self.metadata_store.query_entries(*scenario).await?;
            for e in entries {
                hits.push(MemoryHit {
                    path: format!("{}/{}", e.scenario.dir_name(), e.filename),
                    scenario: e.scenario,
                    title: e.title,
                    tags: e.tags,
                    frequency: e.frequency,
                    score: 0.0, // No semantic score for metadata-only enumeration
                    tokens: e.tokens as usize,
                });
            }
        }
        hits.truncate(top_k);
        Ok(hits)
    }

    /// Check if an error's root cause is `std::io::ErrorKind::NotFound`.
    fn is_not_found(error: &anyhow::Error) -> bool {
        error
            .root_cause()
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    }
}
