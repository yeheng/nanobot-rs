//! Agent module: core processing engine

pub mod context;
pub mod context_v2;
pub mod executor;
pub mod executor_core;
pub mod history_processor;
pub mod history_query;
pub mod loop_;
pub mod memory;
pub mod pipeline;
pub mod prompt;
pub mod request;
pub mod skill_loader;
pub mod stream;
pub mod stream_buffer;
pub mod subagent;
pub mod subagent_tracker;
pub mod summarization;

pub use context::{AgentContext, PersistentContext, StatelessContext};
pub use context_v2::{
    AgentContext as AgentContextV2, CompressionTask, PersistentContext as PersistentContextV2,
};
pub use executor::ToolExecutor;
pub use executor_core::{AgentExecutor, ExecutionResult, ExecutorOptions};
pub use history_processor::{count_tokens, process_history, HistoryConfig, ProcessedHistory};
pub use history_query::{
    HistoryQuery, HistoryQueryBuilder, HistoryResult, HistoryRetriever, QueryOrder, ResultMeta,
    SemanticQuery, TimeRange,
};
pub use loop_::{AgentConfig, AgentLoop, AgentResponse};
pub use memory::MemoryStore;
pub use pipeline::{process_message, PipelineContext};
pub use stream::StreamEvent;
pub use stream_buffer::BufferedEvents;
pub use subagent::{run_subagent, SessionKeyGuard, SubagentManager};
pub use subagent_tracker::{SubagentResult, SubagentTracker, TrackerError};
