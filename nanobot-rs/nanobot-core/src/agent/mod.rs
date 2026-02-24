//! Agent module: core processing engine

pub mod context;
pub mod executor;
pub mod history_processor;
pub mod loop_;
pub mod memory;
pub mod subagent;
pub mod task_store;
pub mod task_store_sqlite;

pub use context::ContextBuilder;
pub use executor::ToolExecutor;
pub use history_processor::{
    CombinedStrategy, DirectInjectStrategy, HistoryConfig, HistoryStrategy, ProcessedHistory,
    RelevanceFilterStrategy, StrategyFactory, SummarizeStrategy, TokenBudgetStrategy,
    TruncateStrategy,
};
pub use loop_::{AgentConfig, AgentLoop, AgentResponse, StreamCallback, StreamEvent};
pub use memory::MemoryStore;
pub use subagent::{
    SubagentConfig, SubagentManager, SubagentTask, TaskNotification, TaskPriority, TaskStatus,
};
