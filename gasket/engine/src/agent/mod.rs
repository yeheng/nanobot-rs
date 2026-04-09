//! Agent module: core processing engine
//!
//! Organized into subsystems:
//! - core: Main loop, context, config
//! - execution: LLM request execution, tool handling
//! - memory: Memory loading, context compression
//! - history: History retrieval, indexing, context building
//! - streaming: Stream events, buffering
//! - subagents: Subagent orchestration and tracking

pub mod core;
pub mod execution;
pub mod history;
pub mod memory;
pub mod streaming;
pub mod subagents;

// ── Re-exports for backward compatibility ──

// Core
pub use core::{AgentConfig, AgentContext, AgentLoop, AgentResponse, PersistentContext};

// Execution
pub use execution::{
    AgentExecutor, ExecutionResult, ExecutorOptions, RequestHandler, ToolExecutor,
};

// Memory
pub use memory::{
    ContextCompactor, MemoryContext, MemoryManager, MemoryProvider, MemoryStore, PhaseBreakdown,
};

// History
pub use history::{
    build_default_hooks, BuildOutcome, ChatRequest, ContextBuilder, ContextMessage, IndexingQueue,
    IndexingService, Priority, QueueError,
};

// Streaming
pub use streaming::{BufferedEvents, StreamEvent};

// Subagents
pub use subagents::{
    run_subagent, ModelResolver, SessionKeyGuard, SubagentManager, SubagentTracker, TrackerError,
};

// Re-export from storage for convenience
pub use gasket_storage::{
    count_tokens, process_history, HistoryConfig, HistoryQuery, HistoryQueryBuilder, HistoryResult,
    HistoryRetriever, ProcessedHistory, QueryOrder, ResultMeta, SemanticQuery, TimeRange,
};
