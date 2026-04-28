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

    let sqlite_store = gasket_engine::SqliteStore::new()
        .await
        .expect("Failed to open SqliteStore");

    let pool = sqlite_store.pool();

    gasket_engine::config::init_config(config.clone());
    gasket_storage::init_db(sqlite_store);

    // Initialize wiki stores if wiki directory exists
    let wiki_root = workspace.join("wiki");
    let (page_store, page_index) = if wiki_root.exists() {
        let ps = PageStore::new(pool.clone(), wiki_root.clone());
        if let Err(e) = gasket_engine::create_wiki_tables(&pool).await {
            tracing::warn!("Failed to create wiki tables: {}", e);
        }
        let pi = match gasket_storage::wiki::TantivyPageIndex::open(wiki_root.join(".tantivy")) {
            Ok(idx) => Some(Arc::new(gasket_engine::wiki::PageIndex::new(Arc::new(idx)))),
            Err(e) => {
                tracing::warn!("Tantivy index open failed, search disabled: {}", e);
                None
            }
        };
        (Some(ps), pi)
    } else {
        (None, None)
    };

    // Initialize embedding recall if configured
    #[cfg(feature = "embedding")]
    let (history_search, _embedding_indexer) = if let Some(ref emb_cfg) = config.embedding {
        let event_store = gasket_engine::EventStore::new(pool.clone());
        match gasket_engine::session::history::builder::setup_embedding_recall(
            &event_store,
            emb_cfg,
        )
        .await
        {
            Ok((searcher, indexer)) => {
                let params = gasket_engine::tools::HistorySearchParams {
                    searcher: searcher.clone(),
                    config: emb_cfg.recall.clone(),
                    event_store: event_store.clone(),
                };
                (Some(params), Some(indexer))
            }
            Err(e) => {
                tracing::warn!("Failed to initialize embedding recall: {}", e);
                (None, None)
            }
        }
    } else {
        (None, None)
    };
    // (non-embedding builds skip semantic recall initialization)

    let tools = build_tool_registry(ToolRegistryConfig {
        subagent_spawner: None,
        extra_tools: vec![],
        page_store,
        page_index,
        provider: Some(provider_info.provider.clone()),
        model: Some(provider_info.model.clone()),
        #[cfg(feature = "embedding")]
        history_search,
    });

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
