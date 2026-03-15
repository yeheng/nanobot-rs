//! Tool system
//!
//! Provides various tools for the agent to interact with the environment:
//! - `exec`: Shell command execution with sandbox support (via nanobot-sandbox)
//! - `filesystem`: File read/write/edit operations
//! - `web_fetch`: Web content fetching
//! - `web_search`: Web search
//! - `memory_search`: Memory search
//! - `history_search`: Conversation history search
//! - `message`: Send messages to users
//! - `cron`: Scheduled tasks
//! - `spawn`: Spawn sub-agents
//! - `spawn_parallel`: Parallel sub-agent spawning

mod base;
mod cron;
mod filesystem;
mod history_search;
mod memory_search;
mod message;
mod registry;
mod shell;
mod spawn;
mod spawn_parallel;
mod web_fetch;
mod web_search;

pub use base::{simple_schema, Tool, ToolError, ToolMetadata, ToolResult};
pub use cron::CronTool;
pub use filesystem::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
pub use history_search::HistorySearchTool;
pub use memory_search::MemorySearchTool;
pub use message::MessageTool;
pub use registry::ToolRegistry;
pub use shell::ExecTool;
pub use spawn::SpawnTool;
pub use spawn_parallel::SpawnParallelTool;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;

// Re-export sandbox types from nanobot-sandbox for backward compatibility
pub use nanobot_sandbox::ProcessManager;
pub use nanobot_sandbox::SandboxConfig;
