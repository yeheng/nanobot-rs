//! Configuration management
//!
//! Compatible with Python nanobot config format (`~/.nanobot/config.yaml`)
//!
//! ## Module Structure
//!
//! - [`schema`] - Root configuration and re-exports
//! - [`loader`] - Configuration loading from files
//! - [`resolver`] - Vault placeholder resolution
//! - [`provider`] - LLM provider configuration (OpenAI, Anthropic, etc.)
//! - [`agent`] - Agent default settings
//! - [`channel`] - Messaging channels (Telegram, Discord, Slack, etc.)
//! - [`tools`] - Tool configuration (Web, MCP, Exec)
//! - [`model_registry`] - Model profile registry for dynamic model switching

mod agent;
mod channel;
mod loader;
mod model_registry;
mod provider;
mod resolver;
mod schema;
mod tools;

pub use agent::{AgentDefaults, AgentDefaults as AgentConfig, AgentsConfig, ModelProfile};
pub use loader::{config_dir, config_path, load_config, ConfigLoader};
pub use model_registry::ModelRegistry;
pub use provider::{ApiCompatibility, ProviderConfig, ProviderType};
pub use resolver::{resolve_string_placeholders, VaultPlaceholderResolve, VAULT_PASSWORD_ENV};
pub use schema::*;
