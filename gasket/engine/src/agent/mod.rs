//! Agent module: core processing engine

pub mod compactor;
pub mod context;
pub mod executor;
pub mod executor_core;
pub mod history_coordinator;
pub mod indexing;
pub mod loop_;
pub mod memory;
pub mod memory_manager;
pub mod prompt;
pub mod request;
pub mod skill_loader;
pub mod stream;
pub mod stream_buffer;
pub mod subagent;
pub mod subagent_tracker;

// New enum-based AgentContext (replaces trait-based version)
pub use compactor::ContextCompactor;
pub use context::AgentContext;
pub use context::PersistentContext;
pub use executor::ToolExecutor;
pub use executor_core::{AgentExecutor, ExecutionResult, ExecutorOptions};
pub use gasket_storage::{
    count_tokens, process_history, HistoryConfig, HistoryQuery, HistoryQueryBuilder, HistoryResult,
    HistoryRetriever, ProcessedHistory, QueryOrder, ResultMeta, SemanticQuery, TimeRange,
};
pub use history_coordinator::ContextMessage;
pub use indexing::IndexingService;
pub use loop_::{AgentConfig, AgentLoop, AgentResponse};
pub use memory::MemoryStore;
pub use memory_manager::{MemoryContext, MemoryManager, PhaseBreakdown};
pub use stream::StreamEvent;
pub use stream_buffer::BufferedEvents;
pub use subagent::{run_subagent, ModelResolver, SessionKeyGuard, SubagentManager};
pub use subagent_tracker::{SubagentTracker, TrackerError};
