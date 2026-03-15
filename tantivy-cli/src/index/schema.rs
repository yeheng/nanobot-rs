//! Schema types for index definition.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Field type definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    /// Full-text indexed (tokenized).
    Text,
    /// Exact match (not tokenized).
    String,
    /// 64-bit integer.
    I64,
    /// 64-bit float.
    F64,
    /// ISO 8601 timestamp.
    DateTime,
    /// Multiple string values (tags, labels).
    StringArray,
    /// Nested JSON (stored only, not indexed).
    Json,
}

/// Field definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    /// Field name.
    pub name: String,
    /// Field type.
    #[serde(rename = "type")]
    pub field_type: FieldType,
    /// Include in search index.
    #[serde(default = "default_indexed")]
    pub indexed: bool,
    /// Return in search results.
    #[serde(default = "default_stored")]
    pub stored: bool,
}

fn default_indexed() -> bool {
    true
}

fn default_stored() -> bool {
    true
}

impl FieldDef {
    /// Create a new text field.
    pub fn text(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: FieldType::Text,
            indexed: true,
            stored: true,
        }
    }

    /// Create a new string field.
    pub fn string(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: FieldType::String,
            indexed: true,
            stored: true,
        }
    }

    /// Create a new datetime field.
    pub fn datetime(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: FieldType::DateTime,
            indexed: true,
            stored: true,
        }
    }

    /// Create a new string array field.
    pub fn string_array(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: FieldType::StringArray,
            indexed: true,
            stored: true,
        }
    }
}

/// Index schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSchema {
    /// Index name.
    pub name: String,
    /// Field definitions.
    pub fields: Vec<FieldDef>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Index configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexConfig {
    /// Default TTL for documents (e.g., "7d", "30d").
    #[serde(default)]
    pub default_ttl: Option<String>,
    /// Auto-compaction settings.
    #[serde(default)]
    pub auto_compact: Option<AutoCompactConfig>,
}

/// Auto-compaction configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoCompactConfig {
    /// Enable auto-compaction.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Trigger when deleted ratio exceeds this value.
    #[serde(default = "default_deleted_ratio")]
    pub deleted_ratio_threshold: f32,
    /// Trigger when segment count exceeds this value.
    #[serde(default = "default_max_segments")]
    pub max_segments: usize,
}

fn default_true() -> bool {
    true
}

fn default_deleted_ratio() -> f32 {
    0.2
}

fn default_max_segments() -> usize {
    10
}

impl Default for AutoCompactConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            deleted_ratio_threshold: 0.2,
            max_segments: 10,
        }
    }
}

impl IndexSchema {
    /// Create a new index schema.
    pub fn new(name: impl Into<String>, fields: Vec<FieldDef>) -> Self {
        Self {
            name: name.into(),
            fields,
            created_at: Utc::now(),
        }
    }

    /// Get a field by name.
    pub fn get_field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
}
