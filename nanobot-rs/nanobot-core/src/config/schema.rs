//! Configuration schema
//!
//! Compatible with Python nanobot's config format (now uses YAML)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// LLM providers configuration
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    /// Agent configuration
    #[serde(default)]
    pub agents: AgentsConfig,

    /// Channel configurations
    #[serde(default)]
    pub channels: ChannelsConfig,

    /// Tools configuration
    #[serde(default)]
    pub tools: ToolsConfig,
}

/// Provider configuration (OpenAI, OpenRouter, Anthropic, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    /// API key for the provider
    #[serde(default, alias = "apiKey")]
    pub api_key: Option<String>,

    /// API base URL (for custom endpoints)
    #[serde(default, alias = "apiBase")]
    pub api_base: Option<String>,

    /// Whether this provider supports thinking/reasoning mode
    /// (e.g., zhipu/glm-5, deepseek/deepseek-reasoner)
    /// If not set, defaults to known providers list
    #[serde(default, alias = "supportsThinking")]
    pub supports_thinking: Option<bool>,
}

/// Known providers that support thinking/reasoning mode
pub const THINKING_CAPABLE_PROVIDERS: &[&str] = &[
    "zhipu",        // GLM-5
    "zhipu_coding", // GLM-5 Coding
    "deepseek",     // DeepSeek R1
    "moonshot",     // Kimi K2.5 (partial support)
];

impl ProviderConfig {
    /// Check if this provider supports thinking mode
    pub fn supports_thinking(&self, provider_name: &str) -> bool {
        // Explicit configuration takes precedence
        if let Some(supported) = self.supports_thinking {
            return supported;
        }
        // Fall back to known providers list
        THINKING_CAPABLE_PROVIDERS.contains(&provider_name)
    }
}

/// Agents configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsConfig {
    /// Default agent settings
    #[serde(default)]
    pub defaults: AgentDefaults,
}

/// Default agent settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentDefaults {
    /// Model to use
    #[serde(default)]
    pub model: Option<String>,

    /// Temperature for generation
    #[serde(default = "default_temperature")]
    pub temperature: f32,

    /// Maximum tokens to generate
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,

    /// Maximum tool call iterations
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,

    /// Memory window size
    #[serde(default = "default_memory_window")]
    pub memory_window: usize,

    /// Enable thinking/reasoning mode for deep reasoning models (GLM-5, DeepSeek R1, etc.)
    #[serde(default)]
    pub thinking_enabled: bool,

    /// Enable streaming mode for progressive output (default: true)
    #[serde(default = "default_streaming")]
    pub streaming: bool,
}

fn default_temperature() -> f32 {
    0.7
}
fn default_max_tokens() -> u32 {
    4096
}
fn default_max_iterations() -> u32 {
    20
}
fn default_memory_window() -> usize {
    50
}
fn default_streaming() -> bool {
    true
}

/// Channels configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelsConfig {
    /// Telegram channel
    #[serde(default)]
    pub telegram: Option<TelegramConfig>,

    /// Discord channel
    #[serde(default)]
    pub discord: Option<DiscordConfig>,

    /// Slack channel
    #[serde(default)]
    pub slack: Option<SlackConfig>,

    /// Feishu channel
    #[serde(default)]
    pub feishu: Option<FeishuConfig>,

    /// Email channel
    #[serde(default)]
    pub email: Option<EmailConfig>,
}

/// Telegram channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Bot token
    pub token: String,

    /// Allowed user IDs
    #[serde(default)]
    pub allow_from: Vec<String>,
}

/// Discord channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Bot token
    pub token: String,

    /// Allowed user IDs
    #[serde(default)]
    pub allow_from: Vec<String>,
}

/// Slack channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Bot token (xoxb-...)
    pub bot_token: String,

    /// App token (xapp-...)
    pub app_token: String,

    /// Allowed user IDs
    #[serde(default, alias = "allowFrom")]
    pub allow_from: Vec<String>,

    /// Group policy: mention, open, or allowlist
    #[serde(default)]
    pub group_policy: Option<String>,
}

/// Feishu channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// App ID
    #[serde(alias = "appId")]
    pub app_id: String,

    /// App Secret
    #[serde(alias = "appSecret")]
    pub app_secret: String,

    /// Verification token for webhook validation
    #[serde(default, alias = "verificationToken")]
    pub verification_token: Option<String>,

    /// Encrypt key for event decryption
    #[serde(default, alias = "encryptKey")]
    pub encrypt_key: Option<String>,

    /// Allowed users/groups (empty = allow all)
    #[serde(default, alias = "allowFrom")]
    pub allow_from: Vec<String>,
}

/// Email channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    /// Enable this channel
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// IMAP server host
    #[serde(default, alias = "imapHost")]
    pub imap_host: Option<String>,

    /// IMAP server port (default: 993)
    #[serde(default = "default_imap_port", alias = "imapPort")]
    pub imap_port: u16,

    /// IMAP username
    #[serde(default, alias = "imapUsername")]
    pub imap_username: Option<String>,

    /// IMAP password
    #[serde(default, alias = "imapPassword")]
    pub imap_password: Option<String>,

    /// SMTP server host
    #[serde(default, alias = "smtpHost")]
    pub smtp_host: Option<String>,

    /// SMTP server port (default: 587)
    #[serde(default = "default_smtp_port", alias = "smtpPort")]
    pub smtp_port: u16,

    /// SMTP username
    #[serde(default, alias = "smtpUsername")]
    pub smtp_username: Option<String>,

    /// SMTP password
    #[serde(default, alias = "smtpPassword")]
    pub smtp_password: Option<String>,

    /// From email address
    #[serde(default, alias = "fromAddress")]
    pub from_address: Option<String>,

    /// Allowed senders (empty = allow all)
    #[serde(default, alias = "allowFrom")]
    pub allow_from: Vec<String>,

    /// User consent for email access
    #[serde(default)]
    pub consent_granted: bool,
}

fn default_imap_port() -> u16 {
    993
}

fn default_smtp_port() -> u16 {
    587
}

fn default_true() -> bool {
    true
}

/// Tools configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    /// Restrict file operations to workspace
    #[serde(default, alias = "restrictToWorkspace")]
    pub restrict_to_workspace: bool,

    /// Web tools configuration
    #[serde(default)]
    pub web: WebToolsConfig,

    /// MCP servers
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,

    /// Exec tool configuration
    #[serde(default)]
    pub exec: ExecToolConfig,
}

/// Web tools configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebToolsConfig {
    /// Provider to use for web search (brave, tavily, exa, firecrawl)
    #[serde(default, alias = "searchProvider")]
    pub search_provider: Option<String>,

    /// Brave Search API key for web_search tool
    #[serde(default, alias = "braveApiKey")]
    pub brave_api_key: Option<String>,

    /// Tavily Search API key
    #[serde(default, alias = "tavilyApiKey")]
    pub tavily_api_key: Option<String>,

    /// Exa Search API key
    #[serde(default, alias = "exaApiKey")]
    pub exa_api_key: Option<String>,

    /// Firecrawl API key
    #[serde(default, alias = "firecrawlApiKey")]
    pub firecrawl_api_key: Option<String>,
}

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Command to run (for stdio transport)
    #[serde(default)]
    pub command: Option<String>,

    /// Arguments for the command
    #[serde(default)]
    pub args: Option<Vec<String>>,

    /// URL for HTTP transport
    #[serde(default)]
    pub url: Option<String>,
}

/// Exec tool configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecToolConfig {
    /// Default timeout in seconds
    #[serde(default = "default_exec_timeout")]
    pub timeout: u64,
}

fn default_exec_timeout() -> u64 {
    120
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_config() {
        let yaml = "";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.providers.is_empty());
    }

    #[test]
    fn test_parse_provider_config() {
        let yaml = r#"
providers:
  openrouter:
    api_key: sk-or-v1-xxx
agents:
  defaults:
    model: anthropic/claude-opus-4-5
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.providers.get("openrouter").unwrap().api_key,
            Some("sk-or-v1-xxx".to_string())
        );
        assert_eq!(
            config.agents.defaults.model,
            Some("anthropic/claude-opus-4-5".to_string())
        );
    }
}
