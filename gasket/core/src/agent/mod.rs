//! Agent module: core processing engine

pub mod compression;
pub mod context;
pub mod executor;
pub mod executor_core;
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

pub use compression::{CompressionActor, EmbeddingService, SummarizationService};
// New enum-based AgentContext (replaces trait-based version)
pub use context::AgentContext;
pub use context::{CompressionTask, PersistentContext};
pub use executor::ToolExecutor;
pub use executor_core::{AgentExecutor, ExecutionResult, ExecutorOptions};
pub use gasket_history::{
    count_tokens, process_history, HistoryConfig, HistoryQuery, HistoryQueryBuilder, HistoryResult,
    HistoryRetriever, ProcessedHistory, QueryOrder, ResultMeta, SemanticQuery, TimeRange,
};
pub use loop_::{AgentConfig, AgentLoop, AgentResponse};
pub use memory::MemoryStore;
pub use pipeline::{process_message, PipelineContext};
pub use stream::StreamEvent;
pub use stream_buffer::BufferedEvents;
pub use subagent::{run_subagent, SessionKeyGuard, SubagentManager};
pub use subagent_tracker::{SubagentResult, SubagentTracker, TrackerError};
