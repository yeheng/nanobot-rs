//! Core type definitions for the memory system.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Memory scenario classification.
///
/// Scenarios determine the directory structure under `~/.gasket/memory/` and
/// influence lifecycle policies (e.g., decay, archival). Each scenario has
/// distinct semantics for when memories are created and how they're used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scenario {
    /// Static profile data (name, preferences, contact info)
    ///
    /// Stored in `~/.gasket/memory/profile/`
    /// Exempt from decay - these are long-lived user facts
    Profile,

    /// Active task/context memories (current projects, ongoing work)
    ///
    /// Stored in `~/.gasket/memory/active/`
    /// Subject to normal decay - aged out when no longer accessed
    Active,

    /// General knowledge and facts (learned information, reference data)
    ///
    /// Stored in `~/.gasket/memory/knowledge/`
    /// Subject to normal decay - aged out when not accessed
    Knowledge,

    /// Decision records with rationale (made choices, trade-offs considered)
    ///
    /// Stored in `~/.gasket/memory/decisions/`
    /// Exempt from decay - decisions are permanent records
    Decisions,

    /// Episodic memories (past conversations, events, experiences)
    ///
    /// Stored in `~/.gasket/memory/episodes/`
    /// Subject to normal decay - aged out over time
    Episodes,

    /// Reference material (docs, links, external resources)
    ///
    /// Stored in `~/.gasket/memory/reference/`
    /// Exempt from decay - reference material is permanent
    Reference,
}

impl Scenario {
    /// Get the directory name for this scenario.
    pub fn dir_name(self) -> &'static str {
        match self {
            Scenario::Profile => "profile",
            Scenario::Active => "active",
            Scenario::Knowledge => "knowledge",
            Scenario::Decisions => "decisions",
            Scenario::Episodes => "episodes",
            Scenario::Reference => "reference",
        }
    }

    /// Get all scenario variants in order.
    pub fn all() -> &'static [Scenario] {
        &[
            Scenario::Profile,
            Scenario::Active,
            Scenario::Knowledge,
            Scenario::Decisions,
            Scenario::Episodes,
            Scenario::Reference,
        ]
    }

    /// Check if this scenario is exempt from decay.
    ///
    /// Profile, Decisions, and Reference memories are permanent records.
    pub fn is_exempt_from_decay(self) -> bool {
        matches!(
            self,
            Scenario::Profile | Scenario::Decisions | Scenario::Reference
        )
    }

    /// Parse a directory name into a Scenario.
    pub fn from_dir_name(s: &str) -> Option<Scenario> {
        match s {
            "profile" => Some(Scenario::Profile),
            "active" => Some(Scenario::Active),
            "knowledge" => Some(Scenario::Knowledge),
            "decisions" => Some(Scenario::Decisions),
            "episodes" => Some(Scenario::Episodes),
            "reference" => Some(Scenario::Reference),
            _ => None,
        }
    }
}

impl fmt::Display for Scenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.dir_name())
    }
}

/// Memory access frequency classification.
///
/// Frequency tracks how recently and often a memory is accessed, influencing
/// retention decisions and retrieval ordering. Higher frequency memories are
/// prioritized in search results and protected from cleanup.
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

/// Metadata for a memory file.
///
/// Stored as frontmatter in each Markdown file, this metadata tracks
/// lifecycle information, access patterns, and token budgets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMeta {
    /// Unique memory ID (UUID v4)
    pub id: String,

    /// Human-readable title
    pub title: String,

    /// Memory type (freeform tag for categorization)
    #[serde(default)]
    pub r#type: String,

    /// Scenario classification
    pub scenario: Scenario,

    /// User-defined tags for retrieval
    #[serde(default)]
    pub tags: Vec<String>,

    /// Access frequency classification
    #[serde(default)]
    pub frequency: Frequency,

    /// Number of times this memory has been accessed
    #[serde(default)]
    pub access_count: u64,

    /// Creation timestamp
    pub created: String,

    /// Last update timestamp
    pub updated: String,

    /// Last access timestamp
    #[serde(default)]
    pub last_accessed: String,

    /// Whether this memory auto-expires (for time-sensitive data)
    #[serde(default)]
    pub auto_expire: bool,

    /// Expiration timestamp (if auto_expire is true)
    #[serde(default)]
    pub expires: Option<String>,

    /// Token count (estimated via tiktoken)
    #[serde(default)]
    pub tokens: usize,

    /// ID of superseding memory (if this has been replaced)
    #[serde(default)]
    pub superseded_by: Option<String>,
}

impl Default for MemoryMeta {
    fn default() -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: String::new(),
            r#type: String::new(),
            scenario: Scenario::Active,
            tags: Vec::new(),
            frequency: Frequency::Warm,
            access_count: 0,
            created: now.clone(),
            updated: now.clone(),
            last_accessed: now,
            auto_expire: false,
            expires: None,
            tokens: 0,
            superseded_by: None,
        }
    }
}

/// A complete memory file with metadata and content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFile {
    /// Memory metadata (frontmatter)
    #[serde(rename = "meta")]
    pub metadata: MemoryMeta,

    /// Markdown content
    pub content: String,
}

impl MemoryFile {
    /// Create a new memory file with generated ID and timestamps.
    pub fn new(scenario: Scenario, title: impl Into<String>, content: impl Into<String>) -> Self {
        let title = title.into();
        let content = content.into();
        let now = Utc::now().to_rfc3339();

        Self {
            metadata: MemoryMeta {
                id: uuid::Uuid::new_v4().to_string(),
                title,
                scenario,
                created: now.clone(),
                updated: now.clone(),
                last_accessed: now,
                ..Default::default()
            },
            content,
        }
    }

    /// Get the estimated token count for this memory.
    pub fn token_count(&self) -> usize {
        self.metadata.tokens
    }
}

/// Query parameters for memory search.
///
/// Used to filter and rank memories during retrieval operations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryQuery {
    /// Full-text search query
    #[serde(default)]
    pub text: Option<String>,

    /// Tag filters (AND semantics - all must match)
    #[serde(default)]
    pub tags: Vec<String>,

    /// Scenario filter
    #[serde(default)]
    pub scenario: Option<Scenario>,

    /// Maximum tokens to return
    #[serde(default)]
    pub max_tokens: Option<usize>,
}

impl MemoryQuery {
    /// Create a new empty query.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the text search query.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Add a tag filter.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Set the scenario filter.
    pub fn with_scenario(mut self, scenario: Scenario) -> Self {
        self.scenario = Some(scenario);
        self
    }

    /// Set the max token budget.
    pub fn with_max_tokens(mut self, tokens: usize) -> Self {
        self.max_tokens = Some(tokens);
        self
    }
}

/// A memory search result with relevance scoring.
///
/// Returned by memory retrieval operations, providing both the metadata
/// and a relevance score for ranking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHit {
    /// File path relative to `~/.gasket/memory/`
    pub path: String,

    /// Scenario classification
    pub scenario: Scenario,

    /// Memory title
    pub title: String,

    /// Associated tags
    #[serde(default)]
    pub tags: Vec<String>,

    /// Access frequency
    #[serde(default)]
    pub frequency: Frequency,

    /// Relevance score (0.0 to 1.0, higher is better)
    pub score: f32,

    /// Token count
    pub tokens: usize,
}

/// Token budget configuration for memory operations.
///
/// Defines the maximum token budget for different memory operations,
/// ensuring the AI doesn't exceed context window limits.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TokenBudget {
    /// Budget for bootstrap/context initialization (default: 700)
    #[serde(default = "default_bootstrap")]
    pub bootstrap: usize,

    /// Budget for scenario-specific memories (default: 1500)
    #[serde(default = "default_scenario")]
    pub scenario: usize,

    /// Budget for on-demand memory retrieval (default: 1000)
    #[serde(default = "default_on_demand")]
    pub on_demand: usize,

    /// Total cap across all memory operations (default: 3200)
    #[serde(default = "default_total_cap")]
    pub total_cap: usize,
}

fn default_bootstrap() -> usize {
    700
}

fn default_scenario() -> usize {
    1500
}

fn default_on_demand() -> usize {
    1000
}

fn default_total_cap() -> usize {
    3200
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
    /// Create a new token budget with custom values.
    pub fn new(bootstrap: usize, scenario: usize, on_demand: usize, total_cap: usize) -> Self {
        Self {
            bootstrap,
            scenario,
            on_demand,
            total_cap,
        }
    }

    /// Get the total available budget (min of total_cap and sum of parts).
    pub fn total_budget(&self) -> usize {
        self.total_cap.min(self.bootstrap + self.scenario + self.on_demand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_roundtrip() {
        // Test dir_name -> from_dir_name roundtrip
        for scenario in Scenario::all() {
            let dir_name = scenario.dir_name();
            let parsed = Scenario::from_dir_name(dir_name);
            assert_eq!(Some(*scenario), parsed, "roundtrip failed for {:?}", scenario);
        }

        // Test invalid input
        assert_eq!(None, Scenario::from_dir_name("invalid"));
    }

    #[test]
    fn scenario_exempt_from_decay() {
        // Exempt scenarios
        assert!(Scenario::Profile.is_exempt_from_decay());
        assert!(Scenario::Decisions.is_exempt_from_decay());
        assert!(Scenario::Reference.is_exempt_from_decay());

        // Non-exempt scenarios
        assert!(!Scenario::Active.is_exempt_from_decay());
        assert!(!Scenario::Knowledge.is_exempt_from_decay());
        assert!(!Scenario::Episodes.is_exempt_from_decay());
    }

    #[test]
    fn frequency_ordering() {
        // Test that frequency ordering matches rank
        assert!(Frequency::Hot > Frequency::Warm);
        assert!(Frequency::Warm > Frequency::Cold);
        assert!(Frequency::Cold > Frequency::Archived);

        // Test rank values
        assert_eq!(3, Frequency::Hot.rank());
        assert_eq!(2, Frequency::Warm.rank());
        assert_eq!(1, Frequency::Cold.rank());
        assert_eq!(0, Frequency::Archived.rank());
    }

    #[test]
    fn frequency_from_str_lossy() {
        // Valid inputs
        assert_eq!(Frequency::Hot, Frequency::from_str_lossy("hot"));
        assert_eq!(Frequency::Warm, Frequency::from_str_lossy("WARM"));
        assert_eq!(Frequency::Cold, Frequency::from_str_lossy("Cold"));
        assert_eq!(Frequency::Archived, Frequency::from_str_lossy("archived"));

        // Invalid input defaults to Archived
        assert_eq!(Frequency::Archived, Frequency::from_str_lossy("invalid"));
    }

    #[test]
    fn frequency_from_str() {
        // Valid inputs
        assert_eq!(Ok(Frequency::Hot), Frequency::from_str("hot"));
        assert_eq!(Ok(Frequency::Warm), Frequency::from_str("WARM"));

        // Invalid input returns error
        assert!(Frequency::from_str("invalid").is_err());
    }

    #[test]
    fn memory_meta_default() {
        let meta = MemoryMeta::default();

        // Should have valid UUID
        assert!(uuid::Uuid::parse_str(&meta.id).is_ok());

        // Should have current timestamp
        let parsed = chrono::DateTime::parse_from_rfc3339(&meta.created);
        assert!(parsed.is_ok());

        // Default scenario should be Active
        assert_eq!(Scenario::Active, meta.scenario);

        // Default frequency should be Warm
        assert_eq!(Frequency::Warm, meta.frequency);

        // Should start with zero access count
        assert_eq!(0, meta.access_count);
    }

    #[test]
    fn memory_file_new() {
        let memory = MemoryFile::new(
            Scenario::Knowledge,
            "Test Memory",
            "# Content\n\nThis is test content.",
        );

        // Should have valid UUID
        assert!(uuid::Uuid::parse_str(&memory.metadata.id).is_ok());

        // Should preserve title and content
        assert_eq!("Test Memory", memory.metadata.title);
        assert_eq!("# Content\n\nThis is test content.", memory.content);

        // Should have correct scenario
        assert_eq!(Scenario::Knowledge, memory.metadata.scenario);

        // Should have current timestamp
        let parsed = chrono::DateTime::parse_from_rfc3339(&memory.metadata.created);
        assert!(parsed.is_ok());
    }

    #[test]
    fn memory_query_builder() {
        let query = MemoryQuery::new()
            .with_text("search query")
            .with_tag("important")
            .with_tag("reference")
            .with_scenario(Scenario::Reference)
            .with_max_tokens(1000);

        assert_eq!(Some("search query".to_string()), query.text);
        assert_eq!(vec!["important", "reference"], query.tags);
        assert_eq!(Some(Scenario::Reference), query.scenario);
        assert_eq!(Some(1000), query.max_tokens);
    }

    #[test]
    fn token_budget_default() {
        let budget = TokenBudget::default();

        assert_eq!(700, budget.bootstrap);
        assert_eq!(1500, budget.scenario);
        assert_eq!(1000, budget.on_demand);
        assert_eq!(3200, budget.total_cap);
    }

    #[test]
    fn token_budget_total() {
        let budget = TokenBudget::default();

        // Total should be capped by total_cap
        assert_eq!(3200, budget.total_budget());

        // If we reduce total_cap, total should be limited
        let small_budget = TokenBudget {
            total_cap: 1000,
            ..budget
        };
        assert_eq!(1000, small_budget.total_budget());
    }
}
