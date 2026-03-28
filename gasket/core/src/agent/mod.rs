//! Agent module - re-exports from gasket-engine

pub use gasket_engine::{
    process_message, run_subagent, AgentConfig, AgentContext, AgentExecutor, AgentLoop,
    AgentResponse, CompressionActor, EmbeddingService, ExecutionResult, ExecutorOptions,
    MemoryStore, ModelResolver, PipelineContext, SessionKeyGuard, StreamEvent, SubagentManager,
    SubagentTracker, SummarizationService, TrackerError,
};

pub mod memory {
    pub use gasket_engine::MemoryStore;
}

pub mod subagent {
    pub use gasket_engine::{run_subagent, ModelResolver, SessionKeyGuard, SubagentManager};
}
