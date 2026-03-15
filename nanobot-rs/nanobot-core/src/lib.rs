//! nanobot-core: A lightweight AI assistant framework
//!
//! This crate provides the core functionality for nanobot:
//! - Agent loop for processing messages
//! - Tool system for executing actions
//! - LLM provider abstraction
//! - Session and memory management
//! - Channel integrations
//! - Cron scheduling
//! - Heartbeat service
//! - MCP (Model Context Protocol) support
//! - Skills system for dynamic skill loading
//! - Webhook server for receiving callbacks
//! - Workspace template management

pub mod agent;
pub mod bus;
pub mod channels;
pub mod config;
pub mod cron;
pub mod crypto;
pub mod error;
pub mod heartbeat;
pub mod hooks;
pub mod mcp;
pub mod memory;
pub mod providers;
pub mod search;

pub mod session;
pub mod skills;
pub mod token_tracker;
pub mod tools;
pub mod vault;
#[cfg(feature = "webhook")]
pub mod webhook;
pub mod workspace;

pub use config::Config;
pub use error::{AgentError, ChannelError, McpError, PipelineError, ProviderError};
pub use providers::{GeminiProvider, LlmProvider, OpenAICompatibleProvider};
pub use skills::{Skill, SkillMetadata, SkillsLoader, SkillsRegistry};
pub use tools::{MessageTool, Tool, ToolRegistry};
pub use vault::{InjectionReport, VaultError, VaultInjector, VaultStore};

// Re-export outbound routing types for convenience
pub use channels::{OutboundSender, OutboundSenderRegistry};
