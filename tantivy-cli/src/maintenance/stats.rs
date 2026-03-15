//! Index health status.

use serde::{Deserialize, Serialize};

/// Index health status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IndexHealth {
    /// Normal operation.
    Healthy,
    /// High deleted document ratio, needs compaction.
    NeedsCompaction,
    /// Approaching size limits.
    Warning,
    /// Index corruption detected.
    Error,
}
