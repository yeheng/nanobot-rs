//! Provider 构建和查找逻辑

use std::sync::Arc;

use anyhow::Result;
use gasket_engine::config::{Config, ProviderType};
use gasket_engine::providers::{LlmProvider, ModelSpec, OpenAICompatibleProvider};

/// Provider information returned by find_provider
pub struct ProviderInfo {
    /// The provider instance
    pub provider: Arc<dyn LlmProvider>,
    /// The model name to use
    pub model: String,
    /// Provider name (e.g., "zhipu", "deepseek")
    pub provider_name: String,
    /// Whether this provider supports thinking/reasoning mode for the selected model
    pub supports_thinking: bool,
    /// Pricing configuration (if configured)
    pub pricing: Option<(f64, f64, String)>, // (input_price, output_price, currency)
}

/// Build a provider instance from its name and config.
///
/// # Errors
///
/// Returns an error if the provider configuration is invalid (e.g., missing api_base).
pub fn build_provider(
    name: &str,
    api_key: &str,
    provider_config: &gasket_engine::config::ProviderConfig,
    model: &str,
) -> Result<Arc<dyn LlmProvider>> {
    // Validate api_base is configured
    if provider_config.api_base.is_empty() {
        anyhow::bail!(
            "Provider '{}' is missing required 'api_base' configuration. \
             Please add 'api_base' to your provider config in ~/.gasket/config.yaml",
            name
        );
    }

    let proxy_enabled = provider_config.proxy_enabled();

    // Create provider based on provider_type
    match provider_config.provider_type {
        ProviderType::Gemini => {
            // Gemini provider (requires feature flag)
            #[cfg(feature = "provider-gemini")]
            {
                let provider = gasket_engine::providers::GeminiProvider::with_config(
                    api_key.to_string(),
                    Some(provider_config.api_base.clone()),
                    Some(model.to_string()),
                    proxy_enabled,
                );
                Ok(Arc::new(provider))
            }
            #[cfg(not(feature = "provider-gemini"))]
            {
                anyhow::bail!(
                    "Gemini provider is not compiled in. Rebuild with --features provider-gemini"
                );
            }
        }
        ProviderType::Anthropic | ProviderType::Openai => {
            // OpenAI-compatible provider (includes Anthropic's /v1 endpoint)
            match name {
                // MiniMax requires special handling for group_id header
                "minimax" => {
                    let provider = OpenAICompatibleProvider::minimax(
                        api_key,
                        provider_config.api_base.clone(),
                        model,
                        None,
                        proxy_enabled,
                    );
                    Ok(Arc::new(provider))
                }
                // GitHub Copilot requires special handling for OAuth token management
                #[cfg(feature = "provider-copilot")]
                "copilot" => Ok(Arc::new(
                    gasket_engine::providers::CopilotProvider::with_proxy(
                        api_key,
                        Some(provider_config.api_base.clone()),
                        Some(model.to_string()),
                        proxy_enabled,
                    ),
                )),
                // All other providers use the generic from_name constructor
                _ => {
                    let provider = OpenAICompatibleProvider::from_name(
                        name,
                        api_key,
                        provider_config.api_base.clone(),
                        Some(model.to_string()),
                        proxy_enabled,
                    );
                    Ok(Arc::new(provider))
                }
            }
        }
    }
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
        "gemini" => "gemini-2.0-flash",
        _ => "gpt-4o",
    }
}

/// Default provider search order when no explicit provider is specified.
const DEFAULT_PROVIDER_ORDER: &[&str] = &[
    "openrouter",
    "deepseek",
    "openai",
    "anthropic",
    "litellm",
    "ollama",
];

/// Find an available provider from the configuration.
///
/// Searches in order of preference defined by `DEFAULT_PROVIDER_ORDER`,
/// then falls back to any other configured provider.
fn find_default_provider(config: &Config) -> Option<String> {
    // First, try providers in the preferred order
    for &name in DEFAULT_PROVIDER_ORDER {
        if let Some(cfg) = config.providers.get(name) {
            if cfg.is_available(name) {
                return Some(name.to_string());
            }
        }
    }

    // Fallback: scan all providers for any available one
    config
        .providers
        .iter()
        .find(|(_, cfg)| cfg.is_available(""))
        .map(|(name, _)| name.clone())
}

/// Find a configured provider.
///
/// The model field supports `provider_id/model_id` format (parsed via
/// `ModelSpec`) to select a specific provider. For example:
///   - `"deepseek/deepseek-chat"` → use the deepseek provider with model deepseek-chat
///   - `"zhipu/glm-4"`           → use the zhipu provider with model glm-4
///   - `"deepseek-chat"`          → legacy behaviour, use default provider
pub fn find_provider(config: &Config) -> Result<ProviderInfo> {
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

    // Determine provider name: explicit in spec, or find default
    let provider_name = match spec.provider() {
        Some(name) => {
            // Explicit provider in model spec — validate it exists and is available
            let cfg = config.providers.get(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "Provider '{}' specified in model '{}' is not configured",
                    name,
                    spec
                )
            })?;

            if !cfg.is_available(name) {
                anyhow::bail!("Provider '{}' is configured but missing API key", name);
            }
            name.to_string()
        }
        None => {
            // No explicit provider — find an available default
            find_default_provider(config).ok_or_else(|| {
                anyhow::anyhow!(
                    "No available provider configured. Run 'gasket onboard' and add your API key to ~/.gasket/config.yaml"
                )
            })?
        }
    };

    // Get provider config (guaranteed to exist at this point)
    let provider_config = config
        .providers
        .get(&provider_name)
        .expect("provider should exist after validation");

    // Resolve API key (empty string for local providers)
    let api_key = provider_config.api_key.as_deref().unwrap_or("");

    // Resolve model name
    let default_model = get_default_model_for_provider(&provider_name);
    let model = if spec.model().is_empty() {
        default_model.to_string()
    } else {
        spec.model().to_string()
    };

    let provider = build_provider(&provider_name, api_key, provider_config, &model)?;

    // Check thinking support for the specific model
    let supports_thinking = provider_config.thinking_enabled_for_model(&model);

    // Get pricing configuration if available (model-level overrides provider-level)
    let pricing = provider_config.get_pricing_for_model(&model).map(|p| {
        (
            p.price_input_per_million,
            p.price_output_per_million,
            p.currency,
        )
    });

    Ok(ProviderInfo {
        provider,
        model,
        provider_name,
        supports_thinking,
        pricing,
    })
}
