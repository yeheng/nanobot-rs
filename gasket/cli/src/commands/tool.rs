//! Tool execution CLI — run a tool directly without going through the agent loop.

use std::sync::Arc;

use anyhow::{Context, Result};
use gasket_engine::config::load_config;
use gasket_engine::tools::{build_tool_registry, ToolContext, ToolRegistryConfig};
use gasket_engine::wiki::PageStore;

/// Execute a tool directly via CLI.
///
/// Example: gasket tool execute evolution '{"threshold": 20}'
pub async fn cmd_tool_execute(name: String, args: String) -> Result<()> {
    let config = load_config().await.context("Failed to load config")?;
    let vault = crate::provider::setup_vault(&config)?;
    let provider_info = crate::provider::find_provider(&config, vault.as_deref())
        .context("No provider available")?;

    let workspace = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".gasket");

    let memory_store = gasket_engine::session::MemoryStore::new().await;
    let sqlite_store = memory_store.sqlite_store().clone();

    // Initialize wiki stores if wiki directory exists
    let wiki_root = workspace.join("wiki");
    let (page_store, page_index) = if wiki_root.exists() {
        let ps = Arc::new(PageStore::new(sqlite_store.pool(), wiki_root.clone()));
        let pi = match gasket_engine::wiki::PageIndex::open(wiki_root.join(".tantivy")) {
            Ok(idx) => Some(Arc::new(idx)),
            Err(e) => {
                tracing::warn!("Tantivy index open failed, search disabled: {}", e);
                None
            }
        };
        (Some(ps), pi)
    } else {
        (None, None)
    };

    let mut tools = build_tool_registry(ToolRegistryConfig {
        config: config.clone(),
        workspace: workspace.clone(),
        subagent_spawner: None,
        extra_tools: vec![],
        sqlite_store: Some(sqlite_store),
        page_store,
        page_index,
        provider: Some(provider_info.provider.clone()),
        model: Some(provider_info.model.clone()),
    });

    let tools_arc = Arc::new(tools.clone());
    tools.link_engine_refs(tools_arc, provider_info.provider.clone());

    let args_json: serde_json::Value = serde_json::from_str(&args)
        .with_context(|| format!("Failed to parse tool arguments as JSON: {}", args))?;

    let ctx = ToolContext::default();

    match tools.execute(&name, args_json, &ctx).await {
        Ok(result) => {
            println!("✓ Tool '{}' executed successfully.\n", name);
            println!("{}", result);
            Ok(())
        }
        Err(e) => {
            anyhow::bail!("Tool '{}' failed: {}", name, e);
        }
    }
}
