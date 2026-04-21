//! Core type definitions for the wiki system (ported from memory).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Token budget configuration for wiki context injection.
///
/// Defines the maximum token budget for different phases of context loading,
/// ensuring the AI doesn't exceed context window limits.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TokenBudget {
    /// Budget for Phase 1 (bootstrap: profile + hot pages).
    #[serde(default = "default_bootstrap")]
    pub bootstrap: usize,
    /// Budget for Phase 2 (scenario-specific search results).
    #[serde(default = "default_scenario")]
    pub scenario: usize,
    /// Budget for Phase 3 (on-demand semantic search fill).
    #[serde(default = "default_on_demand")]
    pub on_demand: usize,
    /// Total cap across all phases.
    #[serde(default = "default_total_cap")]
    pub total_cap: usize,
}

fn default_bootstrap() -> usize {
    1500
}
fn default_scenario() -> usize {
    1500
}
fn default_on_demand() -> usize {
    1000
}
fn default_total_cap() -> usize {
    4000
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self {
            bootstrap: default_bootstrap(),
            scenario: default_scenario(),
            on_demand: default_on_demand(),
            total_cap: default_total_cap(),
        }
    }
}

impl TokenBudget {
    pub fn new(bootstrap: usize, scenario: usize, on_demand: usize, total_cap: usize) -> Self {
        Self {
            bootstrap,
            scenario,
            on_demand,
            total_cap,
        }
    }

    pub fn total_budget(&self) -> usize {
        self.total_cap
            .min(self.bootstrap + self.scenario + self.on_demand)
    }
}

/// Wiki page access frequency classification.
///
/// Frequency tracks how recently and often a page is accessed, influencing
/// retention decisions and retrieval ordering. Higher frequency pages are
/// prioritized in search results and protected from cleanup.
///
/// **Machine runtime state only** — never serialized to Markdown frontmatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Frequency {
    /// Frequently accessed (within last 24 hours)
    ///
    /// Rank: 3 - highest priority for retention
    Hot,
    /// Moderately accessed (within last 7 days)
    ///
    /// Rank: 2 - standard retention priority
    Warm,
    /// Rarely accessed (within last 30 days)
    ///
    /// Rank: 1 - lower retention priority
    Cold,
    /// Not accessed recently (older than 30 days)
    ///
    /// Rank: 0 - candidate for archival
    #[default]
    Archived,
}

impl PartialOrd for Frequency {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Frequency {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl Frequency {
    /// Get the rank value for this frequency (higher = more recent).
    pub fn rank(self) -> u8 {
        match self {
            Frequency::Hot => 3,
            Frequency::Warm => 2,
            Frequency::Cold => 1,
            Frequency::Archived => 0,
        }
    }

    /// Parse a string into a Frequency, defaulting to Archived if unknown.
    pub fn from_str_lossy(s: &str) -> Frequency {
        match s.to_lowercase().as_str() {
            "hot" => Frequency::Hot,
            "warm" => Frequency::Warm,
            "cold" => Frequency::Cold,
            _ => Frequency::Archived,
        }
    }
}

impl fmt::Display for Frequency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Frequency::Hot => write!(f, "hot"),
            Frequency::Warm => write!(f, "warm"),
            Frequency::Cold => write!(f, "cold"),
            Frequency::Archived => write!(f, "archived"),
        }
    }
}

impl FromStr for Frequency {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hot" => Ok(Frequency::Hot),
            "warm" => Ok(Frequency::Warm),
            "cold" => Ok(Frequency::Cold),
            "archived" => Ok(Frequency::Archived),
            _ => Err(format!("unknown frequency: {}", s)),
        }
    }
}
