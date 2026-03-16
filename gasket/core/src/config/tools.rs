//! Tools configuration schemas
//!
//! Configuration for various tools (Web, Exec, etc.)

use serde::{Deserialize, Serialize};

/// Tools configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    /// Restrict file operations to workspace
    #[serde(default, alias = "restrictToWorkspace")]
    pub restrict_to_workspace: bool,

    /// Web tools configuration
    #[serde(default)]
    pub web: WebToolsConfig,

    /// Exec tool configuration
    #[serde(default)]
    pub exec: ExecToolConfig,

    /// Maximum number of results returned by history_search (default: 15)
    #[serde(default = "default_history_search_limit", alias = "historySearchLimit")]
    pub history_search_limit: usize,
}

// ── Web Tools ─────────────────────────────────────────────────────────────

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

    /// HTTP proxy URL (e.g., "http://127.0.0.1:7890")
    #[serde(default, alias = "httpProxy")]
    pub http_proxy: Option<String>,

    /// HTTPS proxy URL (e.g., "http://127.0.0.1:7890")
    #[serde(default, alias = "httpsProxy")]
    pub https_proxy: Option<String>,

    /// SOCKS5 proxy URL (e.g., "socks5://127.0.0.1:1080")
    #[serde(default, alias = "socks5Proxy")]
    pub socks5_proxy: Option<String>,

    /// Whether to use system proxy settings from environment variables
    /// (HTTP_PROXY, HTTPS_PROXY, ALL_PROXY). Default: true
    #[serde(default = "default_true", alias = "useEnvProxy")]
    pub use_env_proxy: bool,
}

// ── Exec Tool ─────────────────────────────────────────────────────────────

/// Exec tool configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecToolConfig {
    /// Default timeout in seconds
    #[serde(default = "default_exec_timeout")]
    pub timeout: u64,

    /// Workspace directory for agent file operations (default: $HOME/.gasket)
    #[serde(default)]
    pub workspace: Option<String>,

    /// Sandbox configuration
    #[serde(default)]
    pub sandbox: SandboxConfig,

    /// Command policy (allowlist/denylist)
    #[serde(default)]
    pub policy: CommandPolicyConfig,

    /// Resource limits
    #[serde(default)]
    pub limits: ResourceLimitsConfig,
}

/// Sandbox execution backend configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Enable sandbox (default: false, opt-in)
    #[serde(default)]
    pub enabled: bool,

    /// Sandbox backend (currently only "bwrap" supported)
    #[serde(default = "default_sandbox_backend")]
    pub backend: String,

    /// Size of /tmp tmpfs inside sandbox in MB (default: 64)
    #[serde(default = "default_tmp_size_mb")]
    pub tmp_size_mb: u32,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: default_sandbox_backend(),
            tmp_size_mb: default_tmp_size_mb(),
        }
    }
}

/// Command allowlist/denylist policy configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommandPolicyConfig {
    /// Allowed command binaries (first token). Empty = allow all.
    #[serde(default)]
    pub allowlist: Vec<String>,

    /// Denied command patterns (substring match). Empty = deny none.
    #[serde(default)]
    pub denylist: Vec<String>,
}

/// Resource limits for shell execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimitsConfig {
    /// Maximum memory in MB (default: 512)
    #[serde(default = "default_max_memory_mb")]
    pub max_memory_mb: u32,

    /// Maximum CPU time in seconds (default: 60)
    #[serde(default = "default_max_cpu_secs")]
    pub max_cpu_secs: u32,

    /// Maximum output size in bytes (default: 1MB)
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
}

impl Default for ResourceLimitsConfig {
    fn default() -> Self {
        Self {
            max_memory_mb: default_max_memory_mb(),
            max_cpu_secs: default_max_cpu_secs(),
            max_output_bytes: default_max_output_bytes(),
        }
    }
}

// ── Default Functions ─────────────────────────────────────────────────────

fn default_true() -> bool {
    true
}

fn default_sandbox_backend() -> String {
    "bwrap".to_string()
}

fn default_tmp_size_mb() -> u32 {
    64
}

fn default_max_memory_mb() -> u32 {
    512
}

fn default_max_cpu_secs() -> u32 {
    60
}

fn default_max_output_bytes() -> usize {
    1_048_576 // 1 MB
}

fn default_exec_timeout() -> u64 {
    120
}

fn default_history_search_limit() -> usize {
    15
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_config_backward_compatible() {
        // Old config with only timeout should still parse
        let yaml = r#"
timeout: 60
"#;
        let exec: ExecToolConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(exec.timeout, 60);
        assert!(!exec.sandbox.enabled);
        assert!(exec.policy.allowlist.is_empty());
        assert_eq!(exec.limits.max_memory_mb, 512);
    }

    #[test]
    fn test_exec_config_with_sandbox() {
        let yaml = r#"
timeout: 120
workspace: /home/user/workspace
sandbox:
  enabled: true
  backend: bwrap
  tmp_size_mb: 128
policy:
  allowlist:
    - ls
    - cat
    - git
  denylist:
    - "rm -rf /"
    - mkfs
limits:
  max_memory_mb: 1024
  max_cpu_secs: 30
  max_output_bytes: 2097152
"#;
        let exec: ExecToolConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(exec.sandbox.enabled);
        assert_eq!(exec.sandbox.backend, "bwrap");
        assert_eq!(exec.sandbox.tmp_size_mb, 128);
        assert_eq!(exec.workspace, Some("/home/user/workspace".to_string()));
        assert_eq!(exec.policy.allowlist, vec!["ls", "cat", "git"]);
        assert_eq!(exec.policy.denylist, vec!["rm -rf /", "mkfs"]);
        assert_eq!(exec.limits.max_memory_mb, 1024);
        assert_eq!(exec.limits.max_cpu_secs, 30);
        assert_eq!(exec.limits.max_output_bytes, 2097152);
    }

    #[test]
    fn test_web_tools_config_with_proxy() {
        let yaml = r#"
searchProvider: brave
braveApiKey: test-brave-key
httpProxy: http://127.0.0.1:7890
httpsProxy: http://127.0.0.1:7890
socks5Proxy: socks5://127.0.0.1:1080
useEnvProxy: false
"#;
        let web: WebToolsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(web.search_provider, Some("brave".to_string()));
        assert_eq!(web.brave_api_key, Some("test-brave-key".to_string()));
        assert_eq!(web.http_proxy, Some("http://127.0.0.1:7890".to_string()));
        assert_eq!(web.https_proxy, Some("http://127.0.0.1:7890".to_string()));
        assert_eq!(
            web.socks5_proxy,
            Some("socks5://127.0.0.1:1080".to_string())
        );
        assert!(!web.use_env_proxy);
    }

    #[test]
    fn test_web_tools_config_defaults() {
        let yaml = r#"
braveApiKey: test-key
"#;
        let web: WebToolsConfig = serde_yaml::from_str(yaml).unwrap();
        // use_env_proxy defaults to true
        assert!(web.use_env_proxy);
        // Proxy fields should be None when not specified
        assert!(web.http_proxy.is_none());
        assert!(web.https_proxy.is_none());
        assert!(web.socks5_proxy.is_none());
    }

    #[test]
    fn test_tools_config_full() {
        let yaml = r#"
restrictToWorkspace: true
web:
  searchProvider: brave
  braveApiKey: test-key
exec:
  timeout: 60
"#;
        let tools: ToolsConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(tools.restrict_to_workspace);
        assert_eq!(tools.web.search_provider, Some("brave".to_string()));
        assert_eq!(tools.exec.timeout, 60);
    }
}
