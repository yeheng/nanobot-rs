//! Configuration types for gasket-engine

pub mod app_config;
pub mod global;
mod tools;

use std::path::PathBuf;

#[cfg(feature = "embedding")]
pub use app_config::EmbeddingConfig;
pub use app_config::{
    config_path, load_config, AgentDefaults, AgentsConfig, Config, ConfigLoader, ModelConfig,
    ModelProfile, ModelRegistry, ProviderConfig, ProviderRegistry, ProviderType,
};
pub use global::{get_config, init_config};
pub use tools::{
    CommandPolicyConfig, ExecToolConfig, ResourceLimitsConfig, SandboxConfig, ToolsConfig,
    WebToolsConfig,
};

// Re-export channel config types (merged from gasket-core facade)
pub use gasket_channels::{
    ChannelsConfig, DingTalkConfig, DiscordConfig, FeishuConfig, SlackConfig, TelegramConfig,
};

/// Get the gasket config directory
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gasket")
}
