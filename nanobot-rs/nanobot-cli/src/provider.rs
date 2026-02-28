//! Provider 构建和查找逻辑

use std::sync::Arc;

use anyhow::Result;
use nanobot_core::config::Config;
use nanobot_core::providers::{
    LlmProvider, ModelSpec, OpenAICompatibleProvider, ProviderMetadata, ProviderRegistry,
};

/// Provider information returned by find_provider
pub struct ProviderInfo {
    /// The provider instance
    pub provider: Arc<dyn LlmProvider>,
    /// The model name to use
    pub model: String,
    /// Provider name (e.g., "zhipu", "deepseek")
    pub provider_name: String,
    /// Whether this provider supports thinking/reasoning mode
    pub supports_thinking: bool,
}

/// Local providers that don't require an API key
const LOCAL_PROVIDERS: &[&str] = &["ollama", "litellm"];

/// Build a provider instance from its name and config.
pub fn build_provider(
    name: &str,
    api_key: &str,
    provider_config: &nanobot_core::config::ProviderConfig,
    model: &str,
) -> Arc<dyn LlmProvider> {
    match name {
        // MiniMax requires special handling for group_id header
        "minimax" => Arc::new(OpenAICompatibleProvider::minimax(
            api_key,
            provider_config.api_base.clone(),
            model,
            None,
        )),
        // GitHub Copilot requires special handling for OAuth token management
        "copilot" => Arc::new(nanobot_core::providers::CopilotProvider::new(
            api_key,
            provider_config.api_base.clone(),
            Some(model.to_string()),
        )),
        // All other providers use the generic from_name constructor
        _ => Arc::new(OpenAICompatibleProvider::from_name(
            name,
            api_key,
            provider_config.api_base.clone(),
            Some(model.to_string()),
        )),
    }
}

/// Build a ProviderRegistry from configuration.
///
/// Iterates through all configured providers, instantiates them, and registers
/// them in the registry with appropriate metadata.
pub fn build_provider_registry(config: &Config) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();

    for (name, provider_config) in &config.providers {
        // Check if provider has credentials
        // Local providers (ollama, litellm) don't require API keys
        let (available, api_key) = if LOCAL_PROVIDERS.contains(&name.as_str()) {
            // Local provider - always available, optional API key for litellm
            let key = provider_config.api_key.as_deref().unwrap_or("");
            (true, key)
        } else if let Some(key) = &provider_config.api_key {
            (true, key.as_str())
        } else {
            (false, "")
        };

        // Get default model for this provider (use provider name as fallback hint)
        let default_model = get_default_model_for_provider(name);

        // Build metadata
        let metadata = ProviderMetadata {
            name: name.to_string(),
            api_base: provider_config.api_base.clone(),
            default_model: default_model.to_string(),
            available,
            missing_config: if available {
                vec![]
            } else {
                vec!["API key not configured".to_string()]
            },
        };

        // Build and register provider if available
        if available {
            let provider = build_provider(name, api_key, provider_config, default_model);
            registry.register(provider, metadata);
        }
    }

    // Set default provider based on preference order
    let default_order = [
        "openrouter",
        "deepseek",
        "openai",
        "anthropic",
        "litellm",
        "ollama",
    ];
    for default_name in default_order {
        if registry.contains(default_name) && registry.set_default(default_name).is_ok() {
            break;
        }
    }

    registry
}

/// Get the default model name for a provider.
pub fn get_default_model_for_provider(name: &str) -> &'static str {
    match name {
        "deepseek" => "deepseek-chat",
        "openrouter" => "anthropic/claude-4.5-sonnet",
        "anthropic" => "claude-4-6-sonnet",
        "zhipu" => "glm-5",
        "dashscope" => "Qwen/Qwen3.5-397B-A17B",
        "moonshot" => "kimi-k2.5",
        "minimax" => "MiniMax-M2.5",
        "ollama" => "llama3",
        "litellm" => "gpt-4o", // LiteLLM proxies to configured models
        "copilot" => "gpt-4o",
        _ => "gpt-4o",
    }
}

/// Find a configured provider using the ProviderRegistry.
///
/// The model field supports `provider_id/model_id` format (parsed via
/// `ModelSpec`) to select a specific provider. For example:
///   - `"deepseek/deepseek-chat"` → use the deepseek provider with model deepseek-chat
///   - `"zhipu/glm-4"`           → use the zhipu provider with model glm-4
///   - `"deepseek-chat"`          → legacy behaviour, use default provider
pub fn find_provider(config: &Config) -> Result<ProviderInfo> {
    let registry = build_provider_registry(config);

    let raw_model = config
        .agents
        .defaults
        .model
        .clone()
        .unwrap_or_else(|| "gpt-4o".to_string());

    // Parse once into a strongly-typed ModelSpec
    let spec: ModelSpec = raw_model
        .parse()
        .expect("ModelSpec::from_str is infallible");

    // Try to get provider by prefix if specified in model
    let provider = if let Some(provider_name) = spec.provider() {
        registry.get(provider_name).ok_or_else(|| {
            anyhow::anyhow!(
                "Provider '{}' specified in model '{}' is not configured or unavailable",
                provider_name,
                spec
            )
        })?
    } else {
        // Use registry's default provider detection
        registry.get_default().ok_or_else(|| {
            anyhow::anyhow!(
                "No available provider configured. Run 'nanobot onboard' and add your API key to ~/.nanobot/config.yaml"
            )
        })?
    };

    let provider_name = provider.name().to_string();
    let supports_thinking = config
        .providers
        .get(&provider_name)
        .map(|p| p.supports_thinking())
        .unwrap_or(false);

    Ok(ProviderInfo {
        provider,
        model: spec.model().to_string(),
        provider_name,
        supports_thinking,
    })
}
