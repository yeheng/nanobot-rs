//! DingTalk (钉钉) webhook handler
//!
//! Provides Axum routes for handling DingTalk callbacks.
//!
//! The webhook handler is **decoupled** from `DingTalkChannel`: it only needs
//! the platform config (for allowlist checks) and an `InboundSender`.
//! No `Arc<RwLock<Channel>>` needed.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::IntoResponse,
    Router,
};
use tracing::{debug, error};

use super::handlers;
use crate::bus::events::InboundMessage;
use crate::bus::ChannelType;
use crate::channels::dingtalk::{DingTalkCallbackMessage, DingTalkConfig};
use crate::channels::middleware::InboundSender;

/// State for DingTalk webhook routes.
///
/// Holds only the data needed for inbound processing — no channel lock.
#[derive(Clone)]
pub struct DingTalkState {
    pub config: Arc<DingTalkConfig>,
    pub inbound_sender: InboundSender,
}

impl DingTalkState {
    /// Create from config and inbound sender
    pub fn from_config(config: DingTalkConfig, inbound_sender: InboundSender) -> Self {
        Self {
            config: Arc::new(config),
            inbound_sender,
        }
    }
}

/// Create a router for DingTalk webhook endpoints
pub fn create_dingtalk_routes(state: DingTalkState, path: Option<&str>) -> Router {
    let path = path.unwrap_or("/dingtalk/callback");
    Router::new()
        .route(path, axum::routing::get(handle_get).post(handle_post))
        .with_state(state)
}

/// Handle GET request (unexpected for DingTalk)
async fn handle_get(
    _state: State<DingTalkState>,
    _query: Query<serde_json::Value>,
) -> impl IntoResponse {
    debug!("DingTalk GET request (unexpected)");
    handlers::bad_request("Use POST for DingTalk webhooks")
}

/// Handle POST request (message callback)
async fn handle_post(
    State(state): State<DingTalkState>,
    _headers: HeaderMap,
    _query: Query<serde_json::Value>,
    body: bytes::Bytes,
) -> impl IntoResponse {
    debug!("DingTalk callback POST request");

    // Parse the callback message
    let message: DingTalkCallbackMessage = match serde_json::from_slice(&body) {
        Ok(m) => m,
        Err(e) => {
            return handlers::bad_request(&format!("Invalid request body: {}", e));
        }
    };

    match process_callback_message(&state.config, &state.inbound_sender, message).await {
        Ok(()) => {
            debug!("DingTalk callback processed successfully");
            handlers::json_response(
                axum::http::StatusCode::OK,
                &serde_json::json!({"msg": "success"}),
            )
        }
        Err(e) => {
            error!("DingTalk callback processing failed: {}", e);
            handlers::json_response(
                axum::http::StatusCode::OK,
                &serde_json::json!({"msg": "success"}),
            )
        }
    }
}

// ── Standalone webhook processing (no Channel dependency) ───

/// Process an incoming DingTalk callback message.
///
/// Only needs `DingTalkConfig` (for allowlist) and `InboundSender`.
async fn process_callback_message(
    config: &DingTalkConfig,
    inbound_sender: &InboundSender,
    message: DingTalkCallbackMessage,
) -> anyhow::Result<()> {
    // Check allowlist
    if !config.allow_from.is_empty() {
        let sender_id = message.sender_id.clone();
        if !config.allow_from.contains(&sender_id) {
            debug!(
                "Ignoring message from unauthorized DingTalk user: {}",
                sender_id
            );
            return Ok(());
        }
    }

    // Only handle text messages
    if message.msgtype != "text" {
        debug!("Ignoring non-text DingTalk message: {}", message.msgtype);
        return Ok(());
    }

    let content = message.text.content.clone();
    debug!("Received DingTalk message: {}", content);

    let metadata = serde_json::to_value(&message).ok();

    let inbound = InboundMessage {
        channel: ChannelType::Dingtalk,
        sender_id: message.sender_id.clone(),
        chat_id: message.conversation_id.clone(),
        content,
        media: None,
        metadata,
        timestamp: chrono::Utc::now(),
        trace_id: None,
    };

    inbound_sender.send(inbound).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn create_test_sender() -> InboundSender {
        let (tx, _rx) = mpsc::channel(100);
        InboundSender::new(tx)
    }

    fn create_test_config() -> DingTalkConfig {
        DingTalkConfig {
            webhook_url: "https://oapi.dingtalk.com/robot/send?access_token=test123".to_string(),
            secret: Some("test_secret".to_string()),
            access_token: None,
            allow_from: vec![],
        }
    }

    #[test]
    fn test_dingtalk_state_creation() {
        let config = create_test_config();
        let state = DingTalkState::from_config(config, create_test_sender());
        assert!(Arc::strong_count(&state.config) >= 1);
    }

    #[test]
    fn test_create_dingtalk_routes_default_path() {
        let config = create_test_config();
        let state = DingTalkState::from_config(config, create_test_sender());
        let _router = create_dingtalk_routes(state, None);
    }

    #[test]
    fn test_create_dingtalk_routes_custom_path() {
        let config = create_test_config();
        let state = DingTalkState::from_config(config, create_test_sender());
        let _router = create_dingtalk_routes(state, Some("/custom/dingtalk"));
    }
}
