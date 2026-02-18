//! Agent module: core processing engine

pub mod context;
pub mod loop_;
pub mod memory;
pub mod subagent;

pub use context::ContextBuilder;
pub use loop_::{AgentConfig, AgentLoop};
pub use memory::MemoryStore;
pub use subagent::{SubagentConfig, SubagentManager, SubagentTask, TaskNotification, TaskPriority, TaskStatus};
