//! Phase-aware tool set — filters ToolRegistry definitions per phase.

use std::sync::Arc;

use gasket_providers::ToolDefinition;

use super::agent_phase::AgentPhase;
use crate::tools::ToolContext;
use crate::tools::ToolRegistry;

/// Phase-aware wrapper around [`ToolRegistry`] that filters tool definitions
/// based on the current [`AgentPhase`].
///
/// Phases with a non-empty `allowed_tools()` list only expose those tools in
/// their definitions. Phases with an empty list (e.g. Execute, Done) expose
/// all registered tools — i.e. no filtering is applied.
pub struct PhasedToolSet {
    registry: Arc<ToolRegistry>,
    phase: AgentPhase,
}

impl PhasedToolSet {
    /// Create a new phased tool set.
    pub fn new(registry: Arc<ToolRegistry>, phase: AgentPhase) -> Self {
        Self { registry, phase }
    }

    /// Return a new `PhasedToolSet` sharing the same registry but with a
    /// different phase. This is cheap — it only bumps the `Arc` ref-count.
    pub fn for_phase(&self, new_phase: AgentPhase) -> Self {
        Self {
            registry: self.registry.clone(),
            phase: new_phase,
        }
    }

    /// The current phase.
    pub fn phase(&self) -> AgentPhase {
        self.phase
    }

    /// Return tool definitions filtered by the current phase.
    ///
    /// If the phase allows all tools (empty `allowed_tools()`), every
    /// definition from the underlying registry is returned.
    pub fn get_definitions(&self) -> Vec<ToolDefinition> {
        let allowed = self.phase.allowed_tools();
        let all_defs = self.registry.get_definitions();
        if allowed.is_empty() {
            return all_defs;
        }
        all_defs
            .into_iter()
            .filter(|def| allowed.contains(&def.function.name.as_str()))
            .collect()
    }

    /// Return the filtered tool names. Used in tests only.
    #[cfg(test)]
    fn definition_names(&self) -> Vec<&str> {
        let allowed = self.phase.allowed_tools();
        let all_names = self.registry.list();
        if allowed.is_empty() {
            return all_names;
        }
        all_names
            .into_iter()
            .filter(|n| allowed.contains(n))
            .collect()
    }

    /// Look up a tool by name, delegating to the underlying registry.
    pub fn get(&self, name: &str) -> Option<&dyn crate::tools::Tool> {
        self.registry.get(name)
    }

    /// Execute a tool by name, delegating to the underlying registry.
    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> crate::tools::ToolResult {
        self.registry.execute(name, args, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::Value;

    struct FakeTool {
        name: &'static str,
    }

    #[async_trait]
    impl crate::tools::Tool for FakeTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "fake"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn execute(
            &self,
            _args: Value,
            _ctx: &crate::tools::ToolContext,
        ) -> crate::tools::ToolResult {
            Ok("ok".to_string())
        }
    }

    fn make_registry() -> Arc<ToolRegistry> {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(FakeTool { name: "wiki_search" }));
        reg.register(Box::new(FakeTool { name: "wiki_read" }));
        reg.register(Box::new(FakeTool { name: "shell" }));
        reg.register(Box::new(FakeTool { name: "write_file" }));
        reg.register(Box::new(FakeTool { name: "phase_transition" }));
        Arc::new(reg)
    }

    #[test]
    fn test_research_phase_filters_tools() {
        let registry = make_registry();
        let tool_set = PhasedToolSet::new(registry, AgentPhase::Research);
        let names: Vec<&str> = tool_set.definition_names();
        assert!(names.contains(&"wiki_search"));
        assert!(names.contains(&"wiki_read"));
        assert!(names.contains(&"phase_transition"));
        assert!(!names.contains(&"shell"));
        assert!(!names.contains(&"write_file"));
    }

    #[test]
    fn test_execute_phase_returns_all_tools() {
        let registry = make_registry();
        let tool_set = PhasedToolSet::new(registry, AgentPhase::Execute);
        let names: Vec<&str> = tool_set.definition_names();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"shell"));
    }

    #[test]
    fn test_for_phase_changes_filter() {
        let registry = make_registry();
        let research = PhasedToolSet::new(registry.clone(), AgentPhase::Research);
        assert_eq!(research.definition_names().len(), 3);
        let execute = research.for_phase(AgentPhase::Execute);
        assert_eq!(execute.definition_names().len(), 5);
    }

    #[test]
    fn test_delegates_execution_to_registry() {
        let registry = make_registry();
        let tool_set = PhasedToolSet::new(registry.clone(), AgentPhase::Execute);
        assert!(tool_set.get("shell").is_some());
    }
}
