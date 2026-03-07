//! Pipeline configuration types.
//!
//! These types are embedded in the top-level `Config` and parsed from YAML.
//! When `pipeline.enabled` is false (or absent), the entire subsystem is a no-op.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::graph::PipelineGraph;

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

    /// Custom pipeline graph. When `None`, `use_default_template` controls
    /// whether the built-in 三省六部 graph is used.
    #[serde(default)]
    pub graph: Option<PipelineGraph>,
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
            graph: None,
        }
    }
}

impl PipelineConfig {
    /// Resolve the effective pipeline graph from configuration.
    ///
    /// Resolution logic:
    /// - `graph: Some(g)` → validate and use the custom graph
    /// - `graph: None` + `use_default_template: true` → `PipelineGraph::default_sansheng()`
    /// - `graph: None` + `use_default_template: false` → error
    pub fn resolve_graph(&self) -> anyhow::Result<PipelineGraph> {
        if let Some(ref g) = self.graph {
            g.validate().map_err(|errors| {
                anyhow::anyhow!(
                    "Pipeline graph validation failed:\n  - {}",
                    errors.join("\n  - ")
                )
            })?;
            Ok(g.clone())
        } else if self.use_default_template {
            Ok(PipelineGraph::default_sansheng())
        } else {
            Err(anyhow::anyhow!(
                "Pipeline is enabled with useDefaultTemplate=false but no custom graph is provided"
            ))
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
        assert!(cfg.graph.is_none());
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

    #[test]
    fn test_resolve_graph_default_template() {
        let cfg = PipelineConfig::default();
        let graph = cfg.resolve_graph().unwrap();
        assert_eq!(graph.entry_state, "triage");
        assert!(graph.terminal_states.contains("done"));
    }

    #[test]
    fn test_resolve_graph_no_template_no_graph_errors() {
        let cfg = PipelineConfig {
            use_default_template: false,
            ..Default::default()
        };
        assert!(cfg.resolve_graph().is_err());
    }

    #[test]
    fn test_resolve_graph_custom() {
        let graph = PipelineGraph::default_sansheng();
        let cfg = PipelineConfig {
            graph: Some(graph),
            ..Default::default()
        };
        let resolved = cfg.resolve_graph().unwrap();
        assert_eq!(resolved.entry_state, "triage");
    }

    #[test]
    fn test_parse_yaml_with_custom_graph() {
        let yaml = r#"
enabled: true
useDefaultTemplate: false
graph:
  entryState: analysis
  terminalStates: [done]
  activeStates: [analysis, development, testing]
  syncRoles: [lead, reviewer]
  gates:
    code_review:
      rejectTo: development
  transitions:
    pending: [analysis]
    analysis: [development]
    development: [code_review]
    code_review: [testing, development]
    testing: [done, blocked]
    blocked: [development, analysis]
    done: []
  stateRoles:
    analysis: lead
    development: developer
    code_review: reviewer
    testing: tester
    blocked: lead
"#;
        let cfg: PipelineConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.graph.is_some());
        let graph = cfg.resolve_graph().unwrap();
        assert_eq!(graph.entry_state, "analysis");
        assert!(graph.can_transition("development", "code_review"));
        assert!(graph.is_sync_role("lead"));
        assert!(!graph.is_sync_role("developer"));
    }
}
