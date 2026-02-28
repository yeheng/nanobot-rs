//! Agent module: core processing engine

pub mod context;
pub mod executor;
pub mod history_processor;
pub mod loop_;
pub mod memory;
pub mod request;
pub mod skill_loader;
pub mod stream;
pub mod subagent;
pub mod summarization;
pub mod task_executor;
pub mod task_store;

pub use context::ContextBuilder;
pub use executor::ToolExecutor;
pub use history_processor::{count_tokens, process_history, HistoryConfig, ProcessedHistory};
pub use loop_::{AgentConfig, AgentLoop, AgentResponse};
pub use memory::MemoryStore;
pub use stream::{StreamCallback, StreamEvent};
pub use subagent::{
    SubagentConfig, SubagentManager, SubagentTask, TaskNotification, TaskPriority, TaskStatus,
};
