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

pub mod agent;
pub mod bus;
pub mod channels;
pub mod config;
pub mod cron;
pub mod heartbeat;
pub mod mcp;
pub mod providers;
pub mod session;
pub mod tools;

pub use config::Config;
pub use providers::LlmProvider;
pub use tools::{Tool, ToolRegistry};
