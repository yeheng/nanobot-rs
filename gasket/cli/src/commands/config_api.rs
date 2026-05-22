//! REST API handlers for configuration management and model switching.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use gasket_engine::config::{ConfigManager, ModelProfile, ProviderConfigUpdate};
use gasket_engine::session::AgentSession;
use gasket_engine::broker::{BrokerPayload, Envelope, MemoryBroker, Topic};
use gasket_types::events::OutboundMessage;
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
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub thinking_enabled: Option<bool>,
}

impl From<CreateModelRequest> for ModelProfile {
    fn from(req: CreateModelRequest) -> Self {
        ModelProfile {
            provider: req.provider,
            model: req.model,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            thinking_enabled: req.thinking_enabled,
        }
    }
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
    let name = body.name.clone();
    let profile = ModelProfile::from(body);
    match state.config_manager.create_model(name, profile).await {
        Ok(()) => (
            axum::http::StatusCode::CREATED,
            Json(json!({"status": "created"})),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
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
            Json(json!({"status": "updated"})),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
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
            Json(json!({"status": "deleted"})),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
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
            Json(json!({"status": "updated"})),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Model Switch Handler ────────────────────────────────────

pub async fn handle_get_current_model(State(state): State<AppState>) -> axum::response::Response {
    let model = state.agent.model();
    (
        axum::http::StatusCode::OK,
        Json(json!({"model": model})),
    )
        .into_response()
}

pub async fn handle_switch_model(
    State(state): State<AppState>,
    Json(body): Json<SwitchModelRequest>,
) -> axum::response::Response {
    // 1. Resolve model_id to (provider, full_id)
    let (provider, full_id) = match state.config_manager.resolve_model_sync(&body.model_id) {
        Ok(result) => result,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error": e.to_string()})),
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
    match state
        .agent
        .switch_model_with_provider(model_name, provider)
        .await
    {
        Ok(info) => {
            // 4. Broadcast model_switched via WebSocket
            let ws_event =
                gasket_types::events::ChatEvent::model_switched(&info.previous, &info.current);
            let out_msg =
                OutboundMessage::broadcast_ws_message(ChannelType::WebSocket, ws_event);
            let envelope = Envelope::new(Topic::Outbound, BrokerPayload::Outbound(out_msg));
            let _ = state.broker.publish(envelope).await;

            (
                axum::http::StatusCode::OK,
                Json(json!({
                    "previous": info.previous,
                    "current": info.current,
                })),
            )
                .into_response()
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Build the config API router with all routes.
pub fn config_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .route("/api/config/models", axum::routing::get(handle_list_models))
        .route(
            "/api/config/models",
            axum::routing::post(handle_create_model),
        )
        .route(
            "/api/config/models/{name}",
            axum::routing::put(handle_update_model),
        )
        .route(
            "/api/config/models/{name}",
            axum::routing::delete(handle_delete_model),
        )
        .route(
            "/api/config/providers",
            axum::routing::get(handle_list_providers),
        )
        .route(
            "/api/config/providers/{name}",
            axum::routing::put(handle_update_provider),
        )
        .route(
            "/api/model/current",
            axum::routing::get(handle_get_current_model),
        )
        .route(
            "/api/model/switch",
            axum::routing::post(handle_switch_model),
        )
        .with_state(state)
}
