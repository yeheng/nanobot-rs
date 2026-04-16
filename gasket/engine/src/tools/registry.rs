//! Tool registry for managing and executing tools

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use serde_json::Value;
use tracing::{debug, instrument};

use super::{Tool, ToolContext, ToolError, ToolMetadata, ToolResult};
use gasket_providers::LlmProvider;
use gasket_providers::ToolDefinition;
use gasket_storage::top_k_similar;

/// Cached tool embeddings: mapping from tool name to embedding vector.
type ToolEmbeddings = Vec<(String, Vec<f32>)>;
#[cfg(feature = "local-embedding")]
use gasket_storage::TextEmbedder;

/// A tool bundled with its optional metadata.
struct RegisteredTool {
    tool: Arc<dyn Tool>,
    metadata: Option<ToolMetadata>,
}

impl Clone for RegisteredTool {
    fn clone(&self) -> Self {
        // Deep-clone PluginTool so that each registry clone can hold
        // independent engine references (tool_registry / provider).
        if let Some(plugin_tool) = self
            .tool
            .as_any()
            .downcast_ref::<crate::plugin::PluginTool>()
        {
            Self {
                tool: Arc::new(plugin_tool.clone()),
                metadata: self.metadata.clone(),
            }
        } else {
            Self {
                tool: self.tool.clone(),
                metadata: self.metadata.clone(),
            }
        }
    }
}

/// Registry for managing tools with semantic routing support.
///
/// The registry stores tool embeddings for fast Top-K retrieval based on
/// cosine similarity to the user query. Embeddings are computed once at
/// startup and cached in memory.
pub struct ToolRegistry {
    items: HashMap<String, RegisteredTool>,
    /// Cached embeddings: (tool_name, embedding_vector)
    embeddings: Arc<OnceLock<ToolEmbeddings>>,
    /// Names of plugins registered via `discover_plugins`.
    plugin_names: Vec<String>,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            items: self.items.clone(),
            embeddings: Arc::new(OnceLock::new()),
            plugin_names: self.plugin_names.clone(),
        }
    }
}

impl ToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
            embeddings: Arc::new(OnceLock::new()),
            plugin_names: Vec::new(),
        }
    }

    fn is_plugin(tool: &dyn Tool) -> bool {
        tool.as_any()
            .downcast_ref::<crate::plugin::PluginTool>()
            .is_some()
    }

    /// Register a tool
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        let is_plugin = Self::is_plugin(tool.as_ref());
        let tool = Arc::from(tool);
        debug!("Registering tool: {}", name);
        if is_plugin {
            self.plugin_names.push(name.clone());
        }
        // Invalidate cached embeddings when tools change
        self.embeddings = Arc::new(OnceLock::new());
        self.items.insert(
            name,
            RegisteredTool {
                tool,
                metadata: None,
            },
        );
    }

    /// Register a tool with associated metadata
    pub fn register_with_metadata(&mut self, tool: Box<dyn Tool>, meta: ToolMetadata) {
        let name = tool.name().to_string();
        let is_plugin = Self::is_plugin(tool.as_ref());
        let tool = Arc::from(tool);
        debug!(
            "Registering tool with metadata: {} (category: {:?})",
            name, meta.category
        );
        if is_plugin {
            self.plugin_names.push(name.clone());
        }
        // Invalidate cached embeddings when tools change
        self.embeddings = Arc::new(OnceLock::new());
        self.items.insert(
            name,
            RegisteredTool {
                tool,
                metadata: Some(meta),
            },
        );
    }

    /// Set metadata for an already-registered tool
    pub fn set_metadata(&mut self, name: &str, meta: ToolMetadata) {
        if let Some(entry) = self.items.get_mut(name) {
            entry.metadata = Some(meta);
        }
    }

    /// Inject engine references into all registered plugins.
    pub fn link_engine_refs(&mut self, registry: Arc<Self>, provider: Arc<dyn LlmProvider>) {
        for name in &self.plugin_names {
            if let Some(entry) = self.items.get_mut(name) {
                if let Some(plugin_tool) = entry
                    .tool
                    .as_any()
                    .downcast_ref::<crate::plugin::PluginTool>()
                {
                    let updated = plugin_tool
                        .clone()
                        .with_engine_refs(registry.clone(), provider.clone());
                    entry.tool = Arc::new(updated);
                }
            }
        }
    }

    /// Initialize embeddings for all registered tools.
    ///
    /// This should be called once after all tools are registered.
    /// Uses the tool's description text to generate embeddings.
    #[cfg(feature = "local-embedding")]
    pub fn initialize_embeddings(&self, embedder: &TextEmbedder) {
        if self.embeddings.get().is_some() {
            debug!("Tool embeddings already initialized, skipping");
            return;
        }

        let mut embeddings = Vec::with_capacity(self.items.len());
        for (name, entry) in &self.items {
            match embedder.embed(entry.tool.description()) {
                Ok(vec) => {
                    embeddings.push((name.clone(), vec));
                    debug!("Generated embedding for tool: {}", name);
                }
                Err(e) => {
                    tracing::warn!("Failed to embed tool '{}': {}", name, e);
                }
            }
        }

        if self.embeddings.set(embeddings).is_err() {
            debug!("Tool embeddings were already set by another thread");
        } else {
            use tracing::info;

            info!("Initialized embeddings for {} tools", self.items.len());
        }
    }

    /// Get the top-K most relevant tools for a query.
    ///
    /// Uses cosine similarity between the query embedding and cached tool embeddings.
    /// Returns tool definitions ready for LLM consumption.
    pub fn get_top_k(&self, query_vec: &[f32], k: usize) -> Vec<ToolDefinition> {
        let embeddings = match self.embeddings.get() {
            Some(e) => e,
            None => {
                // Fallback: return all tools if embeddings not initialized
                debug!("Tool embeddings not initialized, returning all tools");
                return self.get_definitions();
            }
        };

        let top_names = top_k_similar(query_vec, embeddings, k);
        top_names
            .into_iter()
            .filter_map(|(name, _score)| {
                self.items.get(name).map(|entry| {
                    ToolDefinition::function(
                        entry.tool.name(),
                        entry.tool.description(),
                        entry.tool.parameters(),
                    )
                })
            })
            .collect()
    }

    /// Get metadata for a tool
    pub fn get_metadata(&self, name: &str) -> Option<&ToolMetadata> {
        self.items.get(name).and_then(|e| e.metadata.as_ref())
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.items.get(name).map(|e| e.tool.as_ref())
    }

    /// Get all tool definitions for LLM
    pub fn get_definitions(&self) -> Vec<ToolDefinition> {
        self.items
            .values()
            .map(|entry| {
                ToolDefinition::function(
                    entry.tool.name(),
                    entry.tool.description(),
                    entry.tool.parameters(),
                )
            })
            .collect()
    }

    /// Execute a tool by name with context
    #[instrument(skip(self, args, ctx))]
    pub async fn execute(&self, name: &str, args: Value, ctx: &ToolContext) -> ToolResult {
        let entry = self
            .items
            .get(name)
            .ok_or_else(|| ToolError::NotFound(format!("Tool not found: {}", name)))?;

        debug!("Executing tool: {} with args: {:?}", name, args);
        entry.tool.execute(args, ctx).await
    }

    /// List all registered tool names
    pub fn list(&self) -> Vec<&str> {
        self.items.keys().map(|s| s.as_str()).collect()
    }

    /// List tools by category (from metadata)
    pub fn list_by_category(&self, category: &str) -> Vec<&str> {
        self.items
            .iter()
            .filter(|(_, e)| e.metadata.as_ref().is_some_and(|m| m.category == category))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// List tools that require approval (from metadata)
    pub fn list_requiring_approval(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter(|(_, e)| e.metadata.as_ref().is_some_and(|m| m.requires_approval))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// List tools that are mutating (from metadata)
    pub fn list_mutating(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter(|(_, e)| e.metadata.as_ref().is_some_and(|m| m.is_mutating))
            .map(|(name, _)| name.as_str())
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
