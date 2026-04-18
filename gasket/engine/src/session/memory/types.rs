//! Shared types for the memory subsystem.
//!
//! Public types re-exported from the facade (`mod.rs`).

use gasket_storage::memory::MemoryFile;

/// Result of a full reindex operation.
#[derive(Debug)]
pub struct ReindexReport {
    pub total_files: usize,
    pub total_errors: usize,
}

/// Per-phase token breakdown for three-phase memory loading.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PhaseBreakdown {
    /// Tokens used in Phase 1 (bootstrap: profile + active hot/warm).
    pub bootstrap_tokens: usize,
    /// Tokens used in Phase 2 (scenario-specific hot + tag-matched warm).
    pub scenario_tokens: usize,
    /// Tokens used in Phase 3 (on-demand semantic search fill).
    pub on_demand_tokens: usize,
}

/// Result of loading memories for context injection.
#[derive(Debug)]
pub struct MemoryContext {
    /// Loaded memory files (within token budget).
    pub memories: Vec<MemoryFile>,
    /// Total tokens used.
    pub tokens_used: usize,
    /// Per-phase token breakdown.
    pub phase_breakdown: PhaseBreakdown,
}
