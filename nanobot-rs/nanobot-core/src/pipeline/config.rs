//! Pipeline configuration types.
//!
//! These types are embedded in the top-level `Config` and parsed from YAML.
//! When `pipeline.enabled` is false (or absent), the entire subsystem is a no-op.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level pipeline configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    /// Master switch — the pipeline subsystem is completely dormant when false.
    #[serde(default)]
    pub enabled: bool,

    /// Load the built-in 三省六部 role template as defaults.
    #[serde(default = "default_true", alias = "useDefaultTemplate")]
    pub use_default_template: bool,

    /// Per-role definitions (merged on top of defaults when templates are used).
    #[serde(default)]
    pub roles: HashMap<String, AgentRoleDef>,

    /// Maximum review round-trips before escalation (default 3).
    #[serde(default = "default_max_reviews", alias = "maxReviews")]
    pub max_reviews: u32,

    /// Seconds of heartbeat silence before a task is considered stalled (default 60).
    #[serde(default = "default_stall_timeout", alias = "stallTimeoutSecs")]
    pub stall_timeout_secs: u64,

    /// Global model override for all pipeline agents.
    #[serde(default)]
    pub model: Option<String>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            use_default_template: true,
            roles: HashMap::new(),
            max_reviews: default_max_reviews(),
            stall_timeout_secs: default_stall_timeout(),
            model: None,
        }
    }
}

/// Definition of a single agent role inside the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRoleDef {
    /// Human-readable description of the role.
    #[serde(default)]
    pub description: String,

    /// Roles this agent is allowed to delegate to.
    #[serde(default, alias = "allowedAgents")]
    pub allowed_agents: Vec<String>,

    /// Path to a custom SOUL.md file; overrides the built-in template.
    #[serde(default, alias = "soulPath")]
    pub soul_path: Option<String>,

    /// Model override for this specific role.
    #[serde(default)]
    pub model: Option<String>,

    /// States for which this role is responsible.
    #[serde(default, alias = "responsibleStates")]
    pub responsible_states: Vec<String>,
}

// ── defaults ───────────────────────────────────────────────────────

fn default_true() -> bool {
    true
}

fn default_max_reviews() -> u32 {
    3
}

fn default_stall_timeout() -> u64 {
    60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = PipelineConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.use_default_template);
        assert_eq!(cfg.max_reviews, 3);
        assert_eq!(cfg.stall_timeout_secs, 60);
    }

    #[test]
    fn test_parse_yaml_minimal() {
        let yaml = r#"
enabled: true
"#;
        let cfg: PipelineConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.enabled);
        assert!(cfg.use_default_template);
        assert_eq!(cfg.max_reviews, 3);
    }

    #[test]
    fn test_parse_yaml_full() {
        let yaml = r#"
enabled: true
useDefaultTemplate: false
maxReviews: 5
stallTimeoutSecs: 120
model: anthropic/claude-sonnet-4-20250514
roles:
  zhongshu:
    description: "Planning"
    allowedAgents: ["menxia"]
    model: "anthropic/claude-opus-4-20250514"
  gong:
    description: "Development"
    allowedAgents: ["shangshu"]
    soulPath: "/custom/gong.md"
"#;
        let cfg: PipelineConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.enabled);
        assert!(!cfg.use_default_template);
        assert_eq!(cfg.max_reviews, 5);
        assert_eq!(cfg.stall_timeout_secs, 120);
        assert_eq!(cfg.roles.len(), 2);

        let zs = cfg.roles.get("zhongshu").unwrap();
        assert_eq!(zs.allowed_agents, vec!["menxia"]);
        assert_eq!(
            zs.model.as_deref(),
            Some("anthropic/claude-opus-4-20250514")
        );

        let gong = cfg.roles.get("gong").unwrap();
        assert_eq!(gong.soul_path.as_deref(), Some("/custom/gong.md"));
    }
}
