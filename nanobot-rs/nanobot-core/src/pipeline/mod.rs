//! Configurable multi-agent pipeline subsystem.
//!
//! This module implements a data-driven multi-agent collaboration framework.
//! The pipeline topology (states, transitions, role mappings, gate checks)
//! is defined by a `PipelineGraph`, which can be loaded from configuration
//! or from built-in presets (e.g., the 三省六部 template).
//!
//! The built-in 三省六部 preset implements a hierarchical governance model
//! inspired by the Chinese imperial system:
//!
//! - **Triage (太子)**: Analyzes and classifies incoming requests.
//! - **Governance layer**: Zhongshu (中书省/Planning) → Menxia (门下省/Review) → Shangshu (尚书省/Dispatch)
//! - **Execution layer**: Six ministries — Li (礼/Docs), Hu (户/Data), Bing (兵/Ops),
//!   Xing (刑/Compliance), Gong (工/Dev), Dianzhong (殿中/HR)
//!
//! The entire subsystem is **opt-in**: when `config.pipeline.enabled` is false
//! (or absent), zero code is executed and no tables are created.

pub mod config;
pub mod graph;
pub mod models;
pub mod orchestrator;
pub mod permission;
pub mod stall_detector;
pub mod store;

// Re-exports for convenience
pub use config::PipelineConfig;
pub use graph::{GateConfig, PipelineGraph};
pub use models::{FlowLogEntry, PipelineTask, ProgressEntry, TaskPriority};
pub use orchestrator::{OrchestratorActor, PipelineEvent};
pub use permission::PermissionMatrix;
pub use stall_detector::StallDetector;
pub use store::PipelineStore;
