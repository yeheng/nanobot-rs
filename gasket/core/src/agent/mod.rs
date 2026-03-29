//! Agent module - re-exports from gasket-engine

pub use gasket_engine::{
    run_subagent, AgentConfig, AgentContext, AgentExecutor, AgentLoop, AgentResponse,
    CompressionActor, EmbeddingService, ExecutionResult, ExecutorOptions, MemoryStore,
    ModelResolver, SessionKeyGuard, StreamEvent, SubagentManager, SubagentTracker,
    SummarizationService, TrackerError,
};

pub mod memory {
    pub use gasket_engine::MemoryStore;
}

pub mod subagent {
    pub use gasket_engine::{run_subagent, ModelResolver, SessionKeyGuard, SubagentManager};
}
