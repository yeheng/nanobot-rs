//! Memory store trait and core types

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────
//  Core types
// ──────────────────────────────────────────────

/// Metadata attached to a memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetadata {
    /// Provenance of the memory (e.g. "user", "agent", "system").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Categorical tags for filtering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Extensible key-value pairs.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extra: serde_json::Value,
}

impl Default for MemoryMetadata {
    fn default() -> Self {
        Self {
            source: None,
            tags: Vec::new(),
            extra: serde_json::Value::Null,
        }
    }
}

/// A single memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier.
    pub id: String,

    /// The stored content.
    pub content: String,

    /// Structured metadata.
    pub metadata: MemoryMetadata,

    /// When the entry was first created.
    pub created_at: DateTime<Utc>,

    /// When the entry was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Composable query for searching memories.
#[derive(Debug, Clone, Default)]
pub struct MemoryQuery {
    /// Full-text / semantic search query.
    pub text: Option<String>,

    /// Filter by tags (AND semantics — entry must have all listed tags).
    pub tags: Vec<String>,

    /// Filter by source.
    pub source: Option<String>,

    /// Maximum number of results.
    pub limit: Option<usize>,

    /// Number of results to skip (pagination).
    pub offset: Option<usize>,
}

// ──────────────────────────────────────────────
//  MemoryStore trait
// ──────────────────────────────────────────────

/// Abstract storage interface for structured memories.
///
/// Implementations must be safe to share across async tasks.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Save a memory entry. If an entry with the same id exists, it is replaced.
    async fn save(&self, entry: &MemoryEntry) -> anyhow::Result<()>;

    /// Retrieve a memory entry by id.
    async fn get(&self, id: &str) -> anyhow::Result<Option<MemoryEntry>>;

    /// Delete a memory entry by id. Returns `true` if the entry existed.
    async fn delete(&self, id: &str) -> anyhow::Result<bool>;

    /// Search memories matching a query.
    async fn search(&self, query: &MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>>;
}
