//! Runtime configuration management — CRUD operations with dual-write persistence.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;

use super::app_config::{Config, ModelProfile};
use super::ProviderConfig;
use crate::vault::{contains_placeholders, VaultStore};

/// Summary of a provider for API responses (sensitive fields masked).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderSummary {
    pub name: String,
    pub provider_type: String,
    pub api_base: String,
    pub api_key_set: bool,
    pub default_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_username: Option<String>,
    pub proxy_password_set: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_currency: Option<String>,
    pub supports_thinking: bool,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra_headers: HashMap<String, String>,
}

/// Centralized configuration management — runtime state + YAML persistence.
pub struct ConfigManager {
    config_path: PathBuf,
    config: RwLock<Config>,
    vault: Option<Arc<VaultStore>>,
}

impl ConfigManager {
    pub fn new(config: Config, config_path: PathBuf, vault: Option<Arc<VaultStore>>) -> Self {
        Self {
            config_path,
            config: RwLock::new(config),
            vault,
        }
    }

    // ── Model Profile CRUD ───────────────────────────────────

    pub async fn list_models(&self) -> HashMap<String, ModelProfile> {
        self.config.read().await.agents.models.clone()
    }

    pub async fn get_model(&self, name: &str) -> Option<ModelProfile> {
        self.config.read().await.agents.models.get(name).cloned()
    }

    pub async fn create_model(&self, name: String, profile: ModelProfile) -> Result<()> {
        let mut cfg = self.config.write().await;
        if cfg.agents.models.contains_key(&name) {
            anyhow::bail!("Model profile '{}' already exists", name);
        }
        cfg.agents.models.insert(name, profile);
        self.persist(&cfg).await?;
        Ok(())
    }

    pub async fn update_model(&self, name: &str, profile: ModelProfile) -> Result<()> {
        let mut cfg = self.config.write().await;
        if !cfg.agents.models.contains_key(name) {
            anyhow::bail!("Model profile '{}' not found", name);
        }
        cfg.agents.models.insert(name.to_string(), profile);
        self.persist(&cfg).await?;
        Ok(())
    }

    pub async fn delete_model(&self, name: &str) -> Result<()> {
        let mut cfg = self.config.write().await;
        if cfg.agents.models.remove(name).is_none() {
            anyhow::bail!("Model profile '{}' not found", name);
        }
        self.persist(&cfg).await?;
        Ok(())
    }

    // ── Provider Config ──────────────────────────────────────

    pub async fn list_providers(&self) -> Vec<ProviderSummary> {
        let cfg = self.config.read().await;
        cfg.providers
            .iter()
            .map(|(name, pc)| ProviderSummary {
                name: name.clone(),
                provider_type: format!("{:?}", pc.provider_type).to_lowercase(),
                api_base: pc.api_base.clone(),
                api_key_set: pc.api_key.is_some(),
                default_model: pc.default_model.clone(),
                proxy_url: pc.proxy_url.clone(),
                proxy_username: pc.proxy_username.clone(),
                proxy_password_set: pc.proxy_password.is_some(),
                client_id: pc.client_id.clone(),
                default_currency: pc.default_currency.clone(),
                supports_thinking: pc.supports_thinking,
                extra_headers: pc.extra_headers.clone(),
            })
            .collect()
    }

    pub async fn update_provider(&self, name: &str, update: ProviderConfigUpdate) -> Result<()> {
        let mut cfg = self.config.write().await;
        let pc = cfg
            .providers
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", name))?;

        if let Some(v) = update.api_base {
            pc.api_base = v;
        }
        if let Some(v) = update.api_key {
            pc.api_key = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = update.default_model {
            pc.default_model = v;
        }
        if let Some(v) = update.proxy_url {
            pc.proxy_url = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = update.proxy_username {
            pc.proxy_username = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = update.proxy_password {
            pc.proxy_password = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = update.client_id {
            pc.client_id = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = update.default_currency {
            pc.default_currency = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = update.supports_thinking {
            pc.supports_thinking = v;
        }
        if let Some(v) = update.extra_headers {
            pc.extra_headers = v;
        }

        self.persist(&cfg).await?;
        Ok(())
    }

    // ── Model Resolution ─────────────────────────────────────

    /// Resolve a model_id string to a (provider, model_name) pair.
    ///
    /// Resolution order:
    /// 1. Named profile from `agents.models`
    /// 2. `provider/model` format
    /// 3. Bare provider name (uses provider's default_model)
    pub fn resolve_model_sync(
        &self,
        model_id: &str,
    ) -> Result<(Arc<dyn gasket_providers::LlmProvider>, String)> {
        let cfg = self.config.blocking_read();

        // 1. Named profile
        if let Some(profile) = cfg.agents.models.get(model_id) {
            let pc = cfg
                .providers
                .get(&profile.provider)
                .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", profile.provider))?;
            let model = profile.model.clone();
            let provider = self.build_provider(&profile.provider, pc, &model)?;
            return Ok((provider, format!("{}/{}", profile.provider, model)));
        }

        // 2. provider/model format
        let parts: Vec<&str> = model_id.splitn(2, '/').collect();
        if parts.len() == 2 {
            let provider_name = parts[0];
            let model_name = parts[1];
            if let Some(pc) = cfg.providers.get(provider_name) {
                let provider = self.build_provider(provider_name, pc, model_name)?;
                return Ok((provider, model_id.to_string()));
            }
        }

        // 3. Bare provider name
        if let Some(pc) = cfg.providers.get(model_id) {
            let model = if pc.default_model.is_empty() {
                "default".to_string()
            } else {
                pc.default_model.clone()
            };
            let provider = self.build_provider(model_id, pc, &model)?;
            return Ok((provider, format!("{}/{}", model_id, model)));
        }

        anyhow::bail!(
            "Cannot resolve model '{}'. Available providers: {}",
            model_id,
            cfg.providers.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    }

    fn build_provider(
        &self,
        name: &str,
        config: &ProviderConfig,
        model: &str,
    ) -> Result<Arc<dyn gasket_providers::LlmProvider>> {
        let raw_api_key = config.api_key.as_deref().unwrap_or("");
        let api_key = self.resolve_api_key(raw_api_key)?;
        gasket_providers::build_provider(name, &api_key, config, model)
    }

    fn resolve_api_key(&self, raw: &str) -> Result<String> {
        if !contains_placeholders(raw) {
            return Ok(raw.to_string());
        }
        match self.vault.as_ref() {
            Some(v) => v
                .resolve_text(raw)
                .map_err(|e| anyhow::anyhow!("Vault resolution failed: {}", e)),
            None => anyhow::bail!(
                "Config contains vault placeholder(s) but no vault is available."
            ),
        }
    }

    /// Get the current default model from config.
    pub async fn get_default_model(&self) -> Option<String> {
        self.config.read().await.agents.defaults.model.clone()
    }

    // ── Persistence ──────────────────────────────────────────

    async fn persist(&self, config: &Config) -> Result<()> {
        let content =
            serde_yaml::to_string(config).context("Failed to serialize config to YAML")?;
        let tmp_path = self.config_path.with_extension("yaml.tmp");
        tokio::fs::write(&tmp_path, &content)
            .await
            .context("Failed to write config temp file")?;
        tokio::fs::rename(&tmp_path, &self.config_path)
            .await
            .context("Failed to rename config temp file")?;
        tracing::info!("Config persisted to {:?}", self.config_path);
        Ok(())
    }
}

/// Update payload for provider config — all fields optional (partial update).
/// Send empty string to clear optional fields (api_key, proxy_url, etc.).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderConfigUpdate {
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default, alias = "defaultModel")]
    pub default_model: Option<String>,
    #[serde(default, alias = "proxyUrl")]
    pub proxy_url: Option<String>,
    #[serde(default, alias = "proxyUsername")]
    pub proxy_username: Option<String>,
    #[serde(default, alias = "proxyPassword")]
    pub proxy_password: Option<String>,
    #[serde(default, alias = "clientId")]
    pub client_id: Option<String>,
    #[serde(default, alias = "defaultCurrency")]
    pub default_currency: Option<String>,
    #[serde(default, alias = "supportsThinking")]
    pub supports_thinking: Option<bool>,
    #[serde(default, alias = "extraHeaders")]
    pub extra_headers: Option<HashMap<String, String>>,
}
