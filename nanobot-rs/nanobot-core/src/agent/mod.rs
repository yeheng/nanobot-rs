//! Agent module: core processing engine

pub mod context;
pub mod executor;
pub mod history_processor;
pub mod loop_;
pub mod memory;
pub mod prompt;
pub mod request;
pub mod skill_loader;
pub mod stream;
pub mod subagent;
pub mod summarization;

pub use context::{AgentContext, PersistentContext, StatelessContext};
pub use executor::ToolExecutor;
pub use history_processor::{count_tokens, process_history, HistoryConfig, ProcessedHistory};
pub use loop_::{AgentConfig, AgentLoop, AgentResponse};
pub use memory::MemoryStore;
pub use stream::StreamEvent;
pub use subagent::SubagentManager;
