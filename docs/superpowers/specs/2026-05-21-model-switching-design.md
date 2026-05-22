# Model Switching & Configuration Management Design

## Overview

Add cross-provider model switching and full model/provider configuration management via both the Vue 3 web frontend and CLI. Changes persist to `config.yaml` and apply to runtime immediately (dual-write).

## Requirements

1. **Cross-provider model switching**: Switch from `zhipu/glm-5` to `anthropic/claude-sonnet-4` — creates new provider instance, updates active model.
2. **Full configuration management**: View, add, edit, delete model profiles (`agents.models`) and provider settings via UI.
3. **Dual-write persistence**: Every config change updates both runtime state and `~/.gasket/config.yaml`.
4. **Both frontend and CLI**: Vue 3 web UI + CLI slash commands.

## Architecture: Hybrid REST + WebSocket

- **REST API** for config CRUD (stateless, idempotent).
- **WebSocket events** for runtime model switch notifications.
- **ConfigManager** service as the single source of truth for read/write/sync.

## Backend Changes

### ConfigManager Service

New file: `engine/src/config/config_manager.rs`

```rust
pub struct ConfigManager {
    config_path: PathBuf,                  // ~/.gasket/config.yaml
    current: RwLock<Config>,               // runtime config snapshot
    provider_registry: ProviderRegistry,   // provider instance pool
}
```

Methods:
- `list_models()` → `HashMap<String, ModelProfile>` from `agents.models`
- `get_model(&str)` → `Option<ModelProfile>`
- `create_model(name, profile)` → validate, insert, persist
- `update_model(name, profile)` → validate, update, persist
- `delete_model(name)` → remove, persist
- `switch_model(model_id)` → parse `provider/model`, create provider, update session
- `list_providers()` → provider configs (API key masked)
- `update_provider(name, config)` → update, persist, re-create provider instance

Persist logic: serialize `Config` back to YAML, write to `config_path` atomically (write to temp file, rename).

### REST API Endpoints

Mounted on the existing axum router in `gateway.rs`:

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/config/models` | List all model profiles |
| POST | `/api/config/models` | Create model profile `{ name, profile }` |
| PUT | `/api/config/models/:name` | Update model profile |
| DELETE | `/api/config/models/:name` | Delete model profile |
| GET | `/api/config/providers` | List providers (key masked) |
| PUT | `/api/config/providers/:name` | Update provider config |
| GET | `/api/model/current` | Get active model info |
| POST | `/api/model/switch` | Switch model `{ model_id }` |

### Runtime Model Switch (Core Refactor)

Current `switch_model()` only changes the model name string. Refactor to:

1. Parse `model_id` into `(provider_name, model_name)` via `ModelSpec`.
2. Look up or create provider instance via `ProviderRegistry::get_or_create()`.
3. Build new `AgentConfig` with the resolved provider and model.
4. Update `AgentSession.active_model` AND the provider reference.
5. Broadcast `model_switched` WebSocket event with `{ previous, current }`.
6. Return `ModelSwitchInfo`.

The `AgentSession` struct needs an `active_provider: Mutex<Arc<dyn LlmProvider>>` field alongside the existing `active_model: Mutex<String>`.

### WebSocket Event

New event type `model_switched`:
```json
{
  "type": "model_switched",
  "data": { "previous": "zhipu/glm-5", "current": "anthropic/claude-sonnet-4" }
}
```

Broadcast to all connected WebSocket clients after a successful switch.

## Frontend Changes (Vue 3)

### Model Selector in ChatHeader

File: `web/src/components/ChatHeader.vue`

- Add a dropdown showing current model (e.g., `zhipu/glm-5`).
- Dropdown lists all `agents.models` profiles + a "Custom..." option.
- On selection: `POST /api/model/switch` → update local state.
- Listen for `model_switched` WebSocket event to sync across tabs.

### Settings Panel

New file: `web/src/components/SettingsPanel.vue`

A slide-out drawer with two tabs:

**Tab 1 - Models:**
- Table of model profiles: name, provider, model, temperature, max_tokens, thinking_enabled.
- Inline editing (click cell to edit).
- Add / Delete buttons.
- Save button calls `PUT /api/config/models/:name` per row.

**Tab 2 - Providers:**
- Table of providers: name, api_base, api_key (masked), default_model.
- Inline editing.
- Save button calls `PUT /api/config/providers/:name`.

Trigger: settings icon in ChatHeader.

### New Composable

File: `web/src/composables/useConfig.ts`

- `fetchModels()` → `GET /api/config/models`
- `createModel(name, profile)` → `POST /api/config/models`
- `updateModel(name, profile)` → `PUT /api/config/models/:name`
- `deleteModel(name)` → `DELETE /api/config/models/:name`
- `fetchProviders()` → `GET /api/config/providers`
- `updateProvider(name, config)` → `PUT /api/config/providers/:name`
- `getCurrentModel()` → `GET /api/model/current`
- `switchModel(modelId)` → `POST /api/model/switch`

### TypeScript Types

Add to `web/src/types/index.ts`:

```typescript
interface ModelProfile {
  provider: string;
  model: string;
  temperature?: number;
  max_tokens?: number;
  thinking_enabled?: boolean;
}

interface ProviderConfig {
  provider_type: string;
  api_base: string;
  api_key?: string;  // masked in responses
  default_model: string;
  proxy_url?: string;
}
```

## CLI Enhancement

Enhance the `/model` command in the existing `CliCommandHost`:

| Command | Description |
|---------|-------------|
| `/model` | Show current active model |
| `/model list` | List all model profiles from `agents.models` |
| `/model switch <id>` | Switch to model (profile name or `provider/model`) |
| `/model add <name> --provider <p> --model <m>` | Add a new model profile |
| `/model remove <name>` | Remove a model profile |

## Error Handling

- Invalid model ID → 400 with descriptive message.
- Unknown provider → 400 with list of available providers.
- Config file write failure → log error, revert runtime state, return 500.
- Provider connection test → optional `dry_run` flag on switch to validate API key before committing.

## Files to Create/Modify

### New Files
- `engine/src/config/config_manager.rs` — ConfigManager service
- `engine/src/config/persist.rs` — YAML persistence helpers
- `web/src/components/SettingsPanel.vue` — Settings drawer
- `web/src/components/ModelSelector.vue` — Model dropdown
- `web/src/composables/useConfig.ts` — Config API composable

### Modified Files
- `engine/src/config/app_config.rs` — ModelRegistry/ProviderRegistry refinements
- `engine/src/session/mod.rs` — Add `active_provider` field to AgentSession
- `engine/src/session/config.rs` — switch_model refactor
- `gasket/cli/src/commands/gateway.rs` — Mount new REST routes
- `gasket/cli/src/commands/registry.rs` — CLI `/model` command enhancement
- `gasket/channels/src/websocket.rs` — `model_switched` event broadcast
- `web/src/components/ChatHeader.vue` — Model selector integration
- `web/src/types/index.ts` — New TypeScript types
- `web/src/composables/useChatSession.ts` — Handle model_switched event

## Success Criteria

1. User can switch from `zhipu/glm-5` to `anthropic/claude-sonnet-4` via web UI dropdown — subsequent messages use the new provider/model.
2. User can add/edit/delete model profiles via settings panel — changes persist to `config.yaml`.
3. User can update provider API keys via settings panel — changes persist and new provider instance works.
4. CLI `/model switch anthropic/claude-sonnet-4` works identically to the web UI.
5. WebSocket clients receive `model_switched` event when model changes.
6. Restart after config change preserves all settings.
