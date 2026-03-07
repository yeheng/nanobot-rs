//! Multi-agent pipeline subsystem (三省六部).
//!
//! This module implements a hierarchical multi-agent collaboration framework
//! inspired by the Chinese imperial governance system:
//!
//! - **Triage (太子)**: Analyzes and classifies incoming requests.
//! - **Governance layer**: Zhongshu (中书省/Planning) → Menxia (门下省/Review) → Shangshu (尚书省/Dispatch)
//! - **Execution layer**: Six ministries — Li (礼/Docs), Hu (户/Data), Bing (兵/Ops),
//!   Xing (刑/Compliance), Gong (工/Dev), Dianzhong (殿中/HR)
//!
//! The entire subsystem is **opt-in**: when `config.pipeline.enabled` is false
//! (or absent), zero code is executed and no tables are created.

pub mod config;
pub mod models;
pub mod orchestrator;
pub mod permission;
pub mod stall_detector;
pub mod state_machine;
pub mod store;

// Re-exports for convenience
pub use config::PipelineConfig;
pub use models::{FlowLogEntry, PipelineTask, ProgressEntry, TaskPriority};
pub use orchestrator::{OrchestratorActor, PipelineEvent};
pub use permission::PermissionMatrix;
pub use stall_detector::StallDetector;
pub use state_machine::TaskState;
pub use store::PipelineStore;
