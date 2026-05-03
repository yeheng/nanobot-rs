//! Shared types used across command-related crates.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::SessionKey;

/// One row in the output of `/sessions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionSummary {
    pub key: SessionKey,
    pub message_count: usize,
    pub last_active: Option<DateTime<Utc>>,
}

/// Result of a successful `/model <id>` switch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelSwitchInfo {
    pub previous: String,
    pub current: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn round_trip_session_summary() {
        let original = SessionSummary {
            key: SessionKey::new(crate::ChannelType::Cli, "interactive"),
            message_count: 42,
            last_active: Some(Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap()),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: SessionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_model_switch_info() {
        let original = ModelSwitchInfo {
            previous: "openai/gpt-4.1".into(),
            current: "openrouter/anthropic/claude-4.5-sonnet".into(),
        };
        let yaml = serde_yaml::to_string(&original).unwrap();
        let decoded: ModelSwitchInfo = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(original, decoded);
    }
}
