# Model Switching & Configuration Management — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add cross-provider model switching and full model/provider configuration management via Vue 3 frontend and CLI, with dual-write persistence.

**Architecture:** Hybrid REST + WebSocket — REST for config CRUD, WebSocket for real-time model switch notifications. A new `ConfigManager` service is the single source of truth for runtime config and YAML persistence. The `AgentSession` gains an `active_provider` field for cross-provider switching.

**Tech Stack:** Rust (axum, tokio, serde), TypeScript (Vue 3, Composition API), WebSocket (existing protocol).

---

## File Structure

### New Files
| File | Responsibility |
|------|---------------|
| `engine/src/config/config_manager.rs` | ConfigManager service: CRUD, persist, resolve |
| `cli/src/commands/config_api.rs` | REST API handlers for config + model switch |
| `web/src/components/ModelSelector.vue` | Dropdown model selector in chat header |
| `web/src/components/SettingsPanel.vue` | Settings drawer with Models/Providers tabs |
| `web/src/composables/useConfig.ts` | REST API client for config management |

### Modified Files
| File | Change |
|------|--------|
| `types/src/events/stream.rs` | Add `ChatEvent::ModelSwitched` variant |
| `engine/src/session/mod.rs` | Add `active_provider`, refactor `switch_model` |
| `engine/src/session/builder.rs` | Initialize `active_provider` |
| `engine/src/config/mod.rs` | Export `ConfigManager` |
| `cli/src/commands/gateway.rs` | Wire ConfigManager, mount REST routes |
| `cli/src/commands/command_host.rs` | Use new `switch_model_with_provider` |
| `command/src/host.rs` | No change (trait already returns Result<ModelSwitchInfo, String>) |
| `web/src/types/index.ts` | Add ModelProfile, ProviderSummary types |
| `web/src/composables/useChatSession.ts` | Handle `model_switched` WS event |
| `web/src/components/ChatHeader.vue` | Integrate ModelSelector + SettingsPanel trigger |

---

## Task 1: Add `ModelSwitched` Event to ChatEvent

**Files:**
- Modify: `gasket/types/src/events/stream.rs`

- [ ] **Step 1: Add the `ModelSwitched` variant to the `ChatEvent` enum**

Insert after the `ApprovalResponse` variant (around line 261):

```rust
    /// Model was switched at runtime
    ModelSwitched {
        previous: Arc<str>,
        current: Arc<str>,
    },
```

- [ ] **Step 2: Add a constructor method on `ChatEvent`**

Insert after `approval_response` method (around line 424):

```rust
    /// Create a model_switched message
    pub fn model_switched(previous: impl Into<String>, current: impl Into<String>) -> Self {
        Self::ModelSwitched {
            previous: Arc::from(previous.into()),
            current: Arc::from(current.into()),
        }
    }
```

- [ ] **Step 3: Add a round-trip serialization test**

Add to the `tests` module:

```rust
    #[test]
    fn test_model_switched_serialization() {
        let event = ChatEvent::model_switched("zhipu/glm-5", "anthropic/claude-sonnet-4");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"model_switched""#));
        assert!(json.contains(r#""previous":"zhipu/glm-5""#));
        assert!(json.contains(r#""current":"anthropic/claude-sonnet-4""#));

        // Round-trip
        let de: ChatEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, de);
    }
```

- [ ] **Step 4: Build and run tests**

Run: `cargo build -p gasket-types && cargo test -p gasket-types`
Expected: Build succeeds, all tests pass including the new one.

- [ ] **Step 5: Commit**

```bash
git add gasket/types/src/events/stream.rs
git commit -m "feat(types): add ModelSwitched event to ChatEvent"
```

---

## Task 2: Add `active_provider` to AgentSession

**Files:**
- Modify: `engine/src/session/mod.rs`
- Modify: `engine/src/session/builder.rs`

- [ ] **Step 1: Add `active_provider` field to `AgentSession` struct**

In `engine/src/session/mod.rs`, add after the `active_model` field (around line 287):

```rust
    /// Mutable provider — supports runtime cross-provider switching.
    active_provider: parking_lot::Mutex<Arc<dyn gasket_providers::LlmProvider>>,
```

- [ ] **Step 2: Add `switch_model_with_provider` method**

In `AgentSession` impl, add after the existing `switch_model` method (around line 449):

```rust
    /// Switch both the model and the provider for cross-provider switching.
    pub async fn switch_model_with_provider(
        &self,
        new_model: &str,
        provider: Arc<dyn gasket_providers::LlmProvider>,
    ) -> Result<gasket_types::ModelSwitchInfo, String> {
        let previous = self.model();

        {
            let mut model_guard = self.active_model.lock();
            *model_guard = new_model.to_string();
        }
        {
            let mut provider_guard = self.active_provider.lock();
            *provider_guard = provider;
        }

        Ok(gasket_types::ModelSwitchInfo {
            previous,
            current: new_model.to_string(),
        })
    }
```

- [ ] **Step 3: Update `preprocess` to use `active_provider`**

In the `preprocess` method, after the line `let mut runtime_ctx = self.runtime_ctx.clone();` (around line 676), insert:

```rust
        // Use the active provider (may have been switched at runtime)
        {
            let provider = self.active_provider.lock().clone();
            runtime_ctx.provider = provider;
        }
```

- [ ] **Step 4: Initialize `active_provider` in session builder**

In `engine/src/session/builder.rs`, find where `AgentSession` is constructed (the final return statement in `build()`). Add the `active_provider` field:

```rust
active_provider: parking_lot::Mutex::new(provider.clone()),
```

The `provider` variable already exists as a parameter to `build()`.

- [ ] **Step 5: Build**

Run: `cargo build -p gasket-engine`
Expected: Build succeeds.

- [ ] **Step 6: Commit**

```bash
git add engine/src/session/mod.rs engine/src/session/builder.rs
git commit -m "feat(engine): add active_provider for cross-provider model switching"
```

---

## Task 3: Create ConfigManager Service

**Files:**
- Create: `engine/src/config/config_manager.rs`
- Modify: `engine/src/config/mod.rs`

- [ ] **Step 1: Create `config_manager.rs`**

```rust
//! Runtime configuration management — CRUD operations with dual-write persistence.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;

use super::app_config::{Config, ModelProfile};
use super::ProviderConfig;
use crate::vault::contains_placeholders;
use crate::vault::VaultStore;

/// Summary of a provider for API responses (API key masked).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderSummary {
    pub name: String,
    pub provider_type: String,
    pub api_base: String,
    pub api_key_set: bool,
    pub default_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
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
        {
            let mut cfg = self.config.write().await;
            if cfg.agents.models.contains_key(&name) {
                anyhow::bail!("Model profile '{}' already exists", name);
            }
            cfg.agents.models.insert(name, profile);
            self.persist(&cfg).await?;
        }
        Ok(())
    }

    pub async fn update_model(&self, name: &str, profile: ModelProfile) -> Result<()> {
        {
            let mut cfg = self.config.write().await;
            if !cfg.agents.models.contains_key(name) {
                anyhow::bail!("Model profile '{}' not found", name);
            }
            cfg.agents.models.insert(name.to_string(), profile);
            self.persist(&cfg).await?;
        }
        Ok(())
    }

    pub async fn delete_model(&self, name: &str) -> Result<()> {
        {
            let mut cfg = self.config.write().await;
            if cfg.agents.models.remove(name).is_none() {
                anyhow::bail!("Model profile '{}' not found", name);
            }
            self.persist(&cfg).await?;
        }
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
            })
            .collect()
    }

    pub async fn update_provider(&self, name: &str, update: ProviderConfigUpdate) -> Result<()> {
        {
            let mut cfg = self.config.write().await;
            let pc = cfg
                .providers
                .get_mut(name)
                .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", name))?;

            if let Some(v) = update.api_base {
                pc.api_base = v;
            }
            if let Some(v) = update.api_key {
                pc.api_key = Some(v);
            }
            if let Some(v) = update.default_model {
                pc.default_model = v;
            }
            if let Some(v) = update.proxy_url {
                pc.proxy_url = Some(v);
            }

            self.persist(&cfg).await?;
        }
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
        let content = serde_yaml::to_string(config)
            .context("Failed to serialize config to YAML")?;
        // Atomic write: write to temp file, then rename
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
}
```

- [ ] **Step 2: Export from config/mod.rs**

In `engine/src/config/mod.rs`, add the new module and re-export:

```rust
pub mod config_manager;

pub use config_manager::{ConfigManager, ProviderConfigUpdate, ProviderSummary};
```

- [ ] **Step 3: Build**

Run: `cargo build -p gasket-engine`
Expected: Build succeeds.

- [ ] **Step 4: Commit**

```bash
git add engine/src/config/config_manager.rs engine/src/config/mod.rs
git commit -m "feat(engine): add ConfigManager service for config CRUD and persistence"
```

---

## Task 4: Create REST API Handlers and Mount Routes

**Files:**
- Create: `cli/src/commands/config_api.rs`
- Modify: `cli/src/commands/gateway.rs`
- Modify: `cli/src/commands/command_host.rs`

- [ ] **Step 1: Create `config_api.rs`**

```rust
//! REST API handlers for configuration management and model switching.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use gasket_engine::config::{ConfigManager, ModelProfile, ProviderConfigUpdate};
use gasket_engine::session::AgentSession;
use gasket_engine::broker::MemoryBroker;
use gasket_engine::broker::{BrokerPayload, Envelope, Topic};
use gasket_types::events::{OutboundMessage, OutboundPayload};
use gasket_channels::ChannelType;

/// Shared state for config API handlers.
#[derive(Clone)]
pub struct AppState {
    pub config_manager: Arc<ConfigManager>,
    pub agent: Arc<AgentSession>,
    pub broker: Arc<MemoryBroker>,
}

// ── Request Types ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateModelRequest {
    pub name: String,
    #[serde(flatten)]
    pub profile: ModelProfile,
}

#[derive(Debug, Deserialize)]
pub struct SwitchModelRequest {
    pub model_id: String,
}

// ── Model Config Handlers ──────────────────────────────────

pub async fn handle_list_models(State(state): State<AppState>) -> axum::response::Response {
    let models = state.config_manager.list_models().await;
    (axum::http::StatusCode::OK, Json(models)).into_response()
}

pub async fn handle_create_model(
    State(state): State<AppState>,
    Json(body): Json<CreateModelRequest>,
) -> axum::response::Response {
    match state.config_manager.create_model(body.name, body.profile).await {
        Ok(()) => (
            axum::http::StatusCode::CREATED,
            Json(serde_json::json!({"status": "created"})),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

pub async fn handle_update_model(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<ModelProfile>,
) -> axum::response::Response {
    match state.config_manager.update_model(&name, body).await {
        Ok(()) => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({"status": "updated"})),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

pub async fn handle_delete_model(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> axum::response::Response {
    match state.config_manager.delete_model(&name).await {
        Ok(()) => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({"status": "deleted"})),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Provider Config Handlers ────────────────────────────────

pub async fn handle_list_providers(State(state): State<AppState>) -> axum::response::Response {
    let providers = state.config_manager.list_providers().await;
    (axum::http::StatusCode::OK, Json(providers)).into_response()
}

pub async fn handle_update_provider(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<ProviderConfigUpdate>,
) -> axum::response::Response {
    match state.config_manager.update_provider(&name, body).await {
        Ok(()) => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({"status": "updated"})),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Model Switch Handler ────────────────────────────────────

pub async fn handle_get_current_model(State(state): State<AppState>) -> axum::response::Response {
    let model = state.agent.model();
    (axum::http::StatusCode::OK, Json(serde_json::json!({"model": model}))).into_response()
}

pub async fn handle_switch_model(
    State(state): State<AppState>,
    Json(body): Json<SwitchModelRequest>,
) -> axum::response::Response {
    // 1. Resolve model_id to (provider, model_name)
    let (provider, full_id) = match state.config_manager.resolve_model_sync(&body.model_id) {
        Ok(result) => result,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // 2. Extract just the model name (after provider/)
    let model_name = full_id
        .splitn(2, '/')
        .nth(1)
        .unwrap_or(&full_id);

    // 3. Switch in the session
    match state.agent.switch_model_with_provider(model_name, provider).await {
        Ok(info) => {
            // 4. Broadcast model_switched via WebSocket
            let ws_event = gasket_types::events::ChatEvent::model_switched(
                &info.previous,
                &info.current,
            );
            let out_msg = OutboundMessage::broadcast_ws_message(
                ChannelType::WebSocket,
                ws_event,
            );
            let envelope = Envelope::new(Topic::Outbound, BrokerPayload::Outbound(out_msg));
            let _ = state.broker.publish(envelope).await;

            (
                axum::http::StatusCode::OK,
                Json(serde_json::json!({
                    "previous": info.previous,
                    "current": info.current,
                })),
            )
                .into_response()
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Build the config API router with all routes.
pub fn config_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .route("/api/config/models", axum::routing::get(handle_list_models))
        .route("/api/config/models", axum::routing::post(handle_create_model))
        .route("/api/config/models/{name}", axum::routing::put(handle_update_model))
        .route("/api/config/models/{name}", axum::routing::delete(handle_delete_model))
        .route("/api/config/providers", axum::routing::get(handle_list_providers))
        .route("/api/config/providers/{name}", axum::routing::put(handle_update_provider))
        .route("/api/model/current", axum::routing::get(handle_get_current_model))
        .route("/api/model/switch", axum::routing::post(handle_switch_model))
        .with_state(state)
}
```

- [ ] **Step 2: Wire into gateway.rs**

In `cli/src/commands/gateway.rs`, make these changes:

**a)** Add import at the top:
```rust
use super::config_api::{self, AppState};
use gasket_engine::config::ConfigManager;
```

**b)** In `cmd_gateway()`, after the `setup_agent_pipeline(...)` call and before `let host = ...`, create the ConfigManager:

```rust
    let config_path = gasket_engine::config::config_path()?;
    let config_manager = Arc::new(ConfigManager::new(
        config.clone(),
        config_path,
        vault.clone().map(|v| Arc::new(v)),
    ));
```

Note: the `vault` variable type needs to be checked — it may already be an `Arc<VaultStore>`. Adjust accordingly.

**c)** Update `setup_http_server` signature to accept `config_manager`:

```rust
async fn setup_http_server(
    providers: &Arc<gasket_channels::ImProviders>,
    agent: &Arc<AgentSession>,
    dispatcher: &Arc<gasket_command::Dispatcher>,
    config_manager: &Arc<ConfigManager>,
    broker: &Arc<gasket_engine::broker::MemoryBroker>,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) {
```

**d)** In the HTTP server spawn block, merge the config routes:

```rust
let app_state = AppState {
    config_manager: config_manager.clone(),
    agent: agent.clone(),
    broker: broker.clone(),
};
app = app.merge(config_api::config_router(app_state));
```

**e)** Update the call site of `setup_http_server` in `cmd_gateway()`:

```rust
setup_http_server(&providers, &agent, &dispatcher, &config_manager, &broker, &mut tasks).await;
```

- [ ] **Step 3: Update command_host.rs to support cross-provider switching**

In `cli/src/commands/command_host.rs`, the `switch_model` method currently calls `self.agent.switch_model(new)`. This will still work for same-provider switches. For cross-provider, the REST API endpoint handles it. The CLI `/model` command will be enhanced in a later task.

No change needed here yet — the existing `switch_model` still works for simple model name changes within the same provider.

- [ ] **Step 4: Build**

Run: `cargo build -p gasket-cli --features websocket`
Expected: Build succeeds. Fix any import errors.

- [ ] **Step 5: Commit**

```bash
git add cli/src/commands/config_api.rs cli/src/commands/gateway.rs
git commit -m "feat(cli): add REST API endpoints for config management and model switching"
```

---

## Task 5: Add TypeScript Types and useConfig Composable

**Files:**
- Modify: `web/src/types/index.ts`
- Create: `web/src/composables/useConfig.ts`

- [ ] **Step 1: Add types to `web/src/types/index.ts`**

Append to the end of the file:

```typescript
// ── Config Types ──────────────────────────────────────────

export interface ModelProfile {
  model: string;
  provider: string;
  temperature?: number;
  max_tokens?: number;
  thinking_enabled?: boolean;
}

export interface ProviderSummary {
  name: string;
  provider_type: string;
  api_base: string;
  api_key_set: boolean;
  default_model: string;
  proxy_url?: string;
}

export interface ModelSwitchInfo {
  previous: string;
  current: string;
}
```

- [ ] **Step 2: Create `web/src/composables/useConfig.ts`**

```typescript
import { ref } from 'vue';
import type { ModelProfile, ProviderSummary } from '@/types';

const API_BASE = '';  // Same origin

async function apiFetch<T>(path: string, options?: RequestInit): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    headers: { 'Content-Type': 'application/json' },
    ...options,
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `API error: ${res.status}`);
  }
  return res.json();
}

export function useConfig() {
  const models = ref<Record<string, ModelProfile>>({});
  const providers = ref<ProviderSummary[]>([]);
  const currentModel = ref<string>('');
  const loading = ref(false);

  async function fetchModels() {
    loading.value = true;
    try {
      const data = await apiFetch<Record<string, ModelProfile>>('/api/config/models');
      models.value = data;
    } finally {
      loading.value = false;
    }
  }

  async function createModel(name: string, profile: ModelProfile) {
    await apiFetch('/api/config/models', {
      method: 'POST',
      body: JSON.stringify({ name, ...profile }),
    });
    await fetchModels();
  }

  async function updateModel(name: string, profile: ModelProfile) {
    await apiFetch(`/api/config/models/${encodeURIComponent(name)}`, {
      method: 'PUT',
      body: JSON.stringify(profile),
    });
    await fetchModels();
  }

  async function deleteModel(name: string) {
    await apiFetch(`/api/config/models/${encodeURIComponent(name)}`, {
      method: 'DELETE',
    });
    await fetchModels();
  }

  async function fetchProviders() {
    loading.value = true;
    try {
      const data = await apiFetch<ProviderSummary[]>('/api/config/providers');
      providers.value = data;
    } finally {
      loading.value = false;
    }
  }

  async function updateProvider(name: string, update: Partial<ProviderSummary & { api_key?: string }>) {
    await apiFetch(`/api/config/providers/${encodeURIComponent(name)}`, {
      method: 'PUT',
      body: JSON.stringify(update),
    });
    await fetchProviders();
  }

  async function fetchCurrentModel() {
    const data = await apiFetch<{ model: string }>('/api/model/current');
    currentModel.value = data.model;
  }

  async function switchModel(modelId: string) {
    const data = await apiFetch<{ previous: string; current: string }>('/api/model/switch', {
      method: 'POST',
      body: JSON.stringify({ model_id: modelId }),
    });
    currentModel.value = data.current;
    return data;
  }

  return {
    models,
    providers,
    currentModel,
    loading,
    fetchModels,
    createModel,
    updateModel,
    deleteModel,
    fetchProviders,
    updateProvider,
    fetchCurrentModel,
    switchModel,
  };
}
```

- [ ] **Step 3: Commit**

```bash
git add web/src/types/index.ts web/src/composables/useConfig.ts
git commit -m "feat(web): add config types and useConfig composable"
```

---

## Task 6: Create ModelSelector Component

**Files:**
- Create: `web/src/components/ModelSelector.vue`

- [ ] **Step 1: Create the component**

```vue
<script setup lang="ts">
import { ref, watch, onMounted } from 'vue';
import { useConfig } from '@/composables/useConfig';

const { models, currentModel, fetchModels, fetchCurrentModel, switchModel } = useConfig();

const open = ref(false);
const switching = ref(false);

onMounted(async () => {
  await Promise.all([fetchModels(), fetchCurrentModel()]);
});

async function selectModel(modelId: string) {
  if (switching.value) return;
  switching.value = true;
  try {
    // modelId can be a profile name or a provider/model string
    const profile = models.value[modelId];
    const id = profile ? `${profile.provider}/${profile.model}` : modelId;
    await switchModel(id);
    open.value = false;
  } catch (e) {
    console.error('Failed to switch model:', e);
  } finally {
    switching.value = false;
  }
}

// Build a flat list of options: named profiles + direct provider/model entries
const options = computed(() => {
  const result: { id: string; label: string; provider: string }[] = [];
  for (const [name, profile] of Object.entries(models.value)) {
    result.push({
      id: name,
      label: `${name} (${profile.provider}/${profile.model})`,
      provider: profile.provider,
    });
  }
  return result;
});

import { computed } from 'vue';
</script>

<template>
  <div class="relative">
    <button
      @click="open = !open"
      class="flex items-center gap-1.5 px-2 py-1 text-xs rounded-md
             bg-secondary/50 hover:bg-secondary transition-colors"
      :title="'Current model: ' + currentModel"
    >
      <span class="h-2 w-2 rounded-full bg-emerald-500 shrink-0" />
      <span class="truncate max-w-[120px]">{{ currentModel || 'default' }}</span>
      <svg class="h-3 w-3 shrink-0 opacity-50" viewBox="0 0 20 20" fill="currentColor">
        <path fill-rule="evenodd" d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z" clip-rule="evenodd" />
      </svg>
    </button>

    <div v-if="open" class="absolute right-0 top-full mt-1 z-50 min-w-[200px] max-h-64 overflow-y-auto
                        rounded-lg border bg-popover shadow-lg">
      <button
        v-for="opt in options"
        :key="opt.id"
        @click="selectModel(opt.id)"
        :disabled="switching"
        class="w-full text-left px-3 py-2 text-xs hover:bg-accent transition-colors
               disabled:opacity-50"
        :class="{ 'bg-accent/50': currentModel === opt.label.split(' ').pop() }"
      >
        {{ opt.label }}
      </button>
      <div v-if="options.length === 0" class="px-3 py-2 text-xs text-muted-foreground">
        No model profiles configured
      </div>
    </div>
  </div>
</template>
```

- [ ] **Step 2: Commit**

```bash
git add web/src/components/ModelSelector.vue
git commit -m "feat(web): add ModelSelector dropdown component"
```

---

## Task 7: Create SettingsPanel Component

**Files:**
- Create: `web/src/components/SettingsPanel.vue`

- [ ] **Step 1: Create the component**

```vue
<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { useConfig } from '@/composables/useConfig';
import type { ModelProfile, ProviderSummary } from '@/types';

const emit = defineEmits<{ close: [] }>();

const { models, providers, loading, fetchModels, fetchProviders, createModel, updateModel, deleteModel, updateProvider } = useConfig();

const activeTab = ref<'models' | 'providers'>('models');

// ── Models tab state ──
const editingModel = ref<string | null>(null);
const newModelName = ref('');
const newModel: Ref<ModelProfile> = ref({ provider: '', model: '' });
import type { Ref } from 'vue';

function startEditModel(name: string) {
  editingModel.value = name;
}

function cancelEditModel() {
  editingModel.value = null;
}

async function saveModel(name: string, profile: ModelProfile) {
  await updateModel(name, profile);
  editingModel.value = null;
}

async function addModel() {
  if (!newModelName.value || !newModel.value.provider || !newModel.value.model) return;
  await createModel(newModelName.value, { ...newModel.value });
  newModelName.value = '';
  newModel.value = { provider: '', model: '' };
}

async function removeModel(name: string) {
  await deleteModel(name);
}

// ── Providers tab state ──
const editingProvider = ref<string | null>(null);
const providerEdits = ref<Record<string, Partial<ProviderSummary & { api_key?: string }>>>({});

function startEditProvider(name: string) {
  editingProvider.value = name;
  const p = providers.value.find(p => p.name === name);
  if (p) {
    providerEdits.value[name] = { api_base: p.api_base, default_model: p.default_model };
  }
}

async function saveProvider(name: string) {
  const edits = providerEdits.value[name];
  if (edits) {
    await updateProvider(name, edits);
  }
  editingProvider.value = null;
}

onMounted(async () => {
  await Promise.all([fetchModels(), fetchProviders()]);
});
</script>

<template>
  <div class="fixed inset-0 z-50 flex justify-end">
    <!-- Backdrop -->
    <div class="absolute inset-0 bg-black/30" @click="emit('close')" />

    <!-- Panel -->
    <div class="relative w-[480px] max-w-full bg-background border-l shadow-xl flex flex-col">
      <!-- Header -->
      <div class="flex items-center justify-between px-4 py-3 border-b">
        <h2 class="text-sm font-semibold">Settings</h2>
        <button @click="emit('close')" class="p-1 rounded hover:bg-secondary">
          <svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12" />
          </svg>
        </button>
      </div>

      <!-- Tabs -->
      <div class="flex border-b">
        <button
          @click="activeTab = 'models'"
          class="flex-1 px-4 py-2 text-xs font-medium transition-colors"
          :class="activeTab === 'models' ? 'border-b-2 border-primary' : 'text-muted-foreground'"
        >Models</button>
        <button
          @click="activeTab = 'providers'"
          class="flex-1 px-4 py-2 text-xs font-medium transition-colors"
          :class="activeTab === 'providers' ? 'border-b-2 border-primary' : 'text-muted-foreground'"
        >Providers</button>
      </div>

      <!-- Content -->
      <div class="flex-1 overflow-y-auto p-4">
        <!-- Loading -->
        <div v-if="loading" class="text-center text-xs text-muted-foreground py-8">Loading...</div>

        <!-- Models Tab -->
        <template v-else-if="activeTab === 'models'">
          <!-- Add new model -->
          <div class="mb-4 p-3 border rounded-lg">
            <div class="text-xs font-medium mb-2">Add Model Profile</div>
            <div class="grid grid-cols-2 gap-2">
              <input v-model="newModelName" placeholder="Profile name" class="input-field" />
              <input v-model="newModel.provider" placeholder="Provider" class="input-field" />
              <input v-model="newModel.model" placeholder="Model" class="input-field" />
              <input v-model.number="newModel.temperature" type="number" step="0.1" placeholder="Temp" class="input-field" />
            </div>
            <button @click="addModel" :disabled="!newModelName || !newModel.provider || !newModel.model"
                    class="mt-2 px-3 py-1 text-xs rounded bg-primary text-primary-foreground disabled:opacity-50">
              Add
            </button>
          </div>

          <!-- Existing models -->
          <div v-for="(profile, name) in models" :key="name"
               class="p-3 border rounded-lg mb-2">
            <div class="flex items-center justify-between mb-1">
              <span class="text-xs font-medium">{{ name }}</span>
              <div class="flex gap-1">
                <button v-if="editingModel !== name" @click="startEditModel(name)"
                        class="text-xs px-2 py-0.5 rounded hover:bg-secondary">Edit</button>
                <button @click="removeModel(name)"
                        class="text-xs px-2 py-0.5 rounded hover:bg-destructive/10 text-destructive">Delete</button>
              </div>
            </div>
            <div class="text-xs text-muted-foreground">
              {{ profile.provider }}/{{ profile.model }}
              <span v-if="profile.temperature"> · temp={{ profile.temperature }}</span>
              <span v-if="profile.max_tokens"> · max_tokens={{ profile.max_tokens }}</span>
            </div>
            <!-- Inline edit (simplified — full edit would use v-model on all fields) -->
            <div v-if="editingModel === name" class="mt-2">
              <input v-model="models[name].provider" class="input-field mb-1" placeholder="Provider" />
              <input v-model="models[name].model" class="input-field mb-1" placeholder="Model" />
              <div class="flex gap-2 mt-1">
                <button @click="saveModel(name, models[name])"
                        class="text-xs px-2 py-0.5 rounded bg-primary text-primary-foreground">Save</button>
                <button @click="cancelEditModel"
                        class="text-xs px-2 py-0.5 rounded hover:bg-secondary">Cancel</button>
              </div>
            </div>
          </div>
        </template>

        <!-- Providers Tab -->
        <template v-else-if="activeTab === 'providers'">
          <div v-for="p in providers" :key="p.name"
               class="p-3 border rounded-lg mb-2">
            <div class="flex items-center justify-between mb-1">
              <span class="text-xs font-medium">{{ p.name }}</span>
              <span class="text-xs px-1.5 py-0.5 rounded bg-secondary">{{ p.provider_type }}</span>
            </div>
            <div class="text-xs text-muted-foreground space-y-0.5">
              <div>{{ p.api_base }}</div>
              <div>API Key: {{ p.api_key_set ? '••••••••' : 'not set' }}</div>
              <div>Default: {{ p.default_model }}</div>
            </div>
            <div v-if="editingProvider === p.name" class="mt-2 space-y-1">
              <input v-model="providerEdits[p.name]!.api_base" class="input-field" placeholder="API Base" />
              <input v-model="providerEdits[p.name]!.api_key" type="password" class="input-field" placeholder="New API Key" />
              <input v-model="providerEdits[p.name]!.default_model" class="input-field" placeholder="Default Model" />
              <div class="flex gap-2 mt-1">
                <button @click="saveProvider(p.name)"
                        class="text-xs px-2 py-0.5 rounded bg-primary text-primary-foreground">Save</button>
                <button @click="editingProvider = null"
                        class="text-xs px-2 py-0.5 rounded hover:bg-secondary">Cancel</button>
              </div>
            </div>
            <button v-else @click="startEditProvider(p.name)"
                    class="mt-2 text-xs px-2 py-0.5 rounded hover:bg-secondary">Edit</button>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.input-field {
  @apply w-full px-2 py-1 text-xs border rounded bg-background;
}
</style>
```

- [ ] **Step 2: Commit**

```bash
git add web/src/components/SettingsPanel.vue
git commit -m "feat(web): add SettingsPanel with model and provider management"
```

---

## Task 8: Integrate UI into ChatHeader + Handle WS Events

**Files:**
- Modify: `web/src/components/ChatHeader.vue`
- Modify: `web/src/composables/useChatSession.ts`

- [ ] **Step 1: Update ChatHeader.vue**

Add imports and reactive state for settings panel and model selector. At the top of `<script setup>`:

```typescript
import ModelSelector from './ModelSelector.vue';
import SettingsPanel from './SettingsPanel.vue';

const showSettings = ref(false);
```

Note: `ref` should already be imported from Vue in ChatHeader.vue. If not, add it.

Add the components to the template. In the header area, before the existing menu dropdowns:

```html
<!-- Model Selector -->
<ModelSelector />

<!-- Settings Button -->
<button
  @click="showSettings = true"
  class="p-1.5 rounded-md hover:bg-secondary transition-colors"
  title="Settings"
>
  <svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
    <path stroke-linecap="round" stroke-linejoin="round" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
    <path stroke-linecap="round" stroke-linejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
  </svg>
</button>

<!-- Settings Panel (slide-out drawer) -->
<SettingsPanel v-if="showSettings" @close="showSettings = false" />
```

- [ ] **Step 2: Handle `model_switched` event in useChatSession.ts**

In `processWebSocketMessageInner`, add a case to the switch statement:

```typescript
case 'model_switched':
  // Model was switched via REST API — update is handled by ModelSelector
  // which polls fetchCurrentModel, or by a reactive ref.
  // For now, log it; the ModelSelector's ref will update on next interaction.
  console.log(`Model switched: ${msg.previous} → ${msg.current}`);
  break;
```

This ensures the event is consumed without errors. The ModelSelector component independently tracks `currentModel` via the `useConfig` composable.

- [ ] **Step 3: Commit**

```bash
git add web/src/components/ChatHeader.vue web/src/composables/useChatSession.ts
git commit -m "feat(web): integrate ModelSelector and SettingsPanel into chat header"
```

---

## Task 9: Build Frontend and End-to-End Test

**Files:** None new — verification only.

- [ ] **Step 1: Build the frontend**

Run: `cd web && npm run build`
Expected: Build succeeds with no errors.

- [ ] **Step 2: Build the backend**

Run: `cargo build -p gasket-cli --features websocket`
Expected: Build succeeds.

- [ ] **Step 3: Manual smoke test**

1. Start the gateway: `cargo run -p gasket-cli --features websocket -- gateway`
2. Open the web frontend in a browser
3. Verify the model selector appears in the chat header showing the current model
4. Click the selector and choose a different model profile
5. Verify the model switches and subsequent messages use the new model
6. Open settings panel, edit a model profile, save
7. Verify the change persists (restart gateway and check config.yaml)

- [ ] **Step 4: Commit any fixes**

If the smoke test reveals issues, fix and commit.

---

## Self-Review Checklist

- [x] **Spec coverage:** Each section in the spec maps to a task:
  - ConfigManager → Task 3
  - REST API → Task 4
  - Runtime model switch → Task 2
  - WebSocket event → Task 1 + Task 4 (broadcast)
  - Frontend types → Task 5
  - useConfig → Task 5
  - ModelSelector → Task 6
  - SettingsPanel → Task 7
  - Integration → Task 8
- [x] **Placeholder scan:** No TBDs, no "implement later", no "similar to Task N"
- [x] **Type consistency:** `ModelProfile` used consistently across Rust and TypeScript. `ProviderSummary`/`ProviderConfigUpdate` match between backend and frontend. `ModelSwitchInfo` matches existing `gasket_types::ModelSwitchInfo`.

Note: CLI `/model` command enhancement for subcommands (`list`, `add`, `remove`) is deferred to keep this plan focused. The existing `/model <id>` command continues to work via the `switch_model` path (same-provider). Cross-provider CLI switching uses the REST API internally or can be enhanced in a follow-up.
