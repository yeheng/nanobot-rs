//! Tool system
//!
//! Provides various tools for the agent to interact with the environment:
//! - `exec`: Shell command execution with sandbox support (via gasket-sandbox)
//! - `filesystem`: File read/write/edit operations
//! - `web_fetch`: Web content fetching
//! - `web_search`: Web search
//! - `memory_search`: Memory search
//! - `memorize`: Write structured long-term memories
//! - `message`: Send messages to users
//! - `cron`: Scheduled tasks
//! - `spawn`: Spawn sub-agents
//! - `spawn_parallel`: Parallel sub-agent spawning
//! - `script`: External script tools with YAML manifests

mod builder;
mod context;
mod create_plan;
mod cron;
mod filesystem;
mod format;
mod history_query;
mod http;
mod memorize;
mod memory_search;
mod message;
mod registry;
mod search_sops;
mod shell;
mod spawn;
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
pub use builder::{build_tool_registry, resolve_exec_workspace, ToolRegistryConfig};
pub use context::ContextTool;
pub use create_plan::CreatePlanTool;
pub use cron::CronTool;
pub use filesystem::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
pub use format::format_subagent_response;
pub use history_query::HistoryQueryTool;
pub use http::build_client_with_proxy;
pub use memorize::MemorizeTool;
pub use memory_search::MemorySearchTool;
pub use message::MessageTool;
pub use registry::ToolRegistry;
pub use search_sops::search_sops;
pub use shell::ExecTool;
pub use spawn::SpawnTool;
pub use spawn_parallel::SpawnParallelTool;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
pub use wiki_decay::WikiDecayTool;
pub use wiki_refresh::WikiRefreshTool;
pub use wiki_tools::{WikiReadTool, WikiSearchTool, WikiWriteTool};
