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

mod base;
mod cron;
mod filesystem;
mod memorize;
mod memory_decay;
mod memory_refresh;
mod memory_search;
mod message;
mod registry;
mod shell;
mod spawn;
mod spawn_parallel;
mod web_fetch;
mod web_search;

// Re-export tool trait and base types from gasket-types
pub use gasket_types::{
    simple_schema, SubagentResult, SubagentSpawner, Tool, ToolContext, ToolError, ToolMetadata,
    ToolResult,
};

// Re-export tool implementations
pub use cron::CronTool;
pub use filesystem::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
pub use memorize::MemorizeTool;
pub use memory_decay::MemoryDecayTool;
pub use memory_refresh::MemoryRefreshTool;
pub use memory_search::MemorySearchTool;
pub use message::MessageTool;
pub use registry::ToolRegistry;
pub use shell::ExecTool;
pub use spawn::SpawnTool;
pub use spawn_parallel::SpawnParallelTool;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;

// Re-export sandbox types from gasket-sandbox for backward compatibility
pub use gasket_sandbox::ProcessManager;
pub use gasket_sandbox::SandboxConfig;
