//! Tool system
//!
//! Provides various tools for the agent to interact with the environment:
//! - `exec`: Shell command execution with sandbox support (via gasket-sandbox)
//! - `filesystem`: File read/write/edit operations
//! - `web_fetch`: Web content fetching
//! - `web_search`: Web search
//! - `wiki_search`: Wiki search
//! - `wiki_write`: Write wiki pages
//! - `wiki_decay`: Run wiki frequency decay
//! - `wiki_refresh`: Refresh wiki index from disk
//! - `message`: Send messages to users
//! - `cron`: Scheduled tasks
//! - `spawn`: Spawn sub-agents
//! - `spawn_parallel`: Parallel sub-agent spawning
//! - `script`: External script tools with YAML manifests
//! - `new_session`: Generate a new session key and clear history

mod builder;
mod clear_session;
mod context;
mod create_plan;
mod cron;
mod evolution;
mod filesystem;
mod format;
mod history_query;
#[cfg(feature = "embedding")]
mod history_search;
mod http;
mod message;
mod new_session;
mod provider;
mod registry;
mod search_sops;
mod shell;
mod spawn;
mod spawn_common;
mod spawn_parallel;
mod web_fetch;
mod web_search;
mod wiki_decay;
mod wiki_refresh;
mod wiki_tools;

// Re-export tool trait and base types from gasket-types
pub use gasket_types::{
    simple_schema, SubagentResult, SubagentSpawner, Tool, ToolContext, ToolError, ToolMetadata,
    ToolResult,
};

// Re-export tool implementations
#[cfg(feature = "embedding")]
pub use builder::HistorySearchParams;
pub use builder::{build_tool_registry, resolve_exec_workspace, ToolRegistryConfig};
pub use clear_session::ClearSessionTool;
pub use context::ContextTool;
pub use create_plan::CreatePlanTool;
pub use cron::CronTool;
pub use evolution::{EvolutionConfig, EvolutionTool};
pub use filesystem::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
pub use format::{extract_json_array, format_subagent_response, truncate_for_display};
pub use history_query::HistoryQueryTool;
#[cfg(feature = "embedding")]
pub use history_search::HistorySearchTool;
pub use http::build_client_with_proxy;
pub use message::MessageTool;
pub use new_session::NewSessionTool;
pub use provider::{CoreToolProvider, SystemToolProvider, ToolProvider, WikiToolProvider};
pub use registry::ToolRegistry;
pub use search_sops::{search_sops, SearchSopsTool};
pub use shell::ExecTool;
pub use spawn::SpawnTool;
pub use spawn_parallel::SpawnParallelTool;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
pub use wiki_decay::WikiDecayTool;
pub use wiki_refresh::WikiRefreshTool;
pub use wiki_tools::{WikiDeleteTool, WikiReadTool, WikiSearchTool, WikiWriteTool};
