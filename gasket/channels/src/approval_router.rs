//! Approval request/response router for WebSocket-driven tool confirmations.
//!
//! Bridges the gap between:
//! - Backend tool execution (which blocks waiting for approval)
//! - Frontend confirmation dialogs (which send `approval_response` over WebSocket)
//!
//! Also stores "remembered" approvals so that future calls from the same session
//! for the same tool can skip the confirmation dialog.

use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use gasket_types::{SessionKey, ToolApprovalResponse};

/// Router that pairs pending approval requests with their responses.
///
/// Uses a `DashMap` so `register` / `resolve` are lock-free on different keys.
#[derive(Debug)]
pub struct ApprovalRouter {
    pending: DashMap<uuid::Uuid, oneshot::Sender<ToolApprovalResponse>>,
    /// Remembered approvals keyed by `(session_key, tool_name)`.
    /// A value of `true` means "always approve"; `false` means "always deny".
    remembered: DashMap<(String, String), bool>,
}

impl ApprovalRouter {
    /// Create a new router.
    pub fn new() -> Self {
        Self {
            pending: DashMap::new(),
            remembered: DashMap::new(),
        }
    }

    /// Check whether a session has a remembered decision for a given tool.
    ///
    /// Returns `Some(true)` if the tool should be auto-approved,
    /// `Some(false)` if it should be auto-denied,
    /// or `None` if there is no remembered rule.
    pub fn is_remembered(&self, session_key: &SessionKey, tool_name: &str) -> Option<bool> {
        let key = (session_key.to_string(), tool_name.to_string());
        self.remembered.get(&key).map(|v| *v)
    }

    /// Store a remembered decision for a session + tool pair.
    pub fn remember(&self, session_key: &SessionKey, tool_name: &str, approved: bool) {
        let key = (session_key.to_string(), tool_name.to_string());
        if approved {
            info!(
                "Remembering approval for session {} tool '{}'",
                session_key, tool_name
            );
        } else {
            info!(
                "Remembering denial for session {} tool '{}'",
                session_key, tool_name
            );
        }
        self.remembered.insert(key, approved);
    }

    /// Remove all remembered decisions for a given session.
    pub fn forget_session(&self, session_key: &SessionKey) {
        let prefix = session_key.to_string();
        self.remembered.retain(|k, _| k.0 != prefix);
    }

    /// Register a new pending request and return a receiver that will be
    /// fulfilled when [`resolve`](Self::resolve) is called with the matching ID.
    pub fn register(&self, id: uuid::Uuid) -> oneshot::Receiver<ToolApprovalResponse> {
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);
        rx
    }

    /// Resolve a pending request with the user's response.
    ///
    /// Returns `true` if a pending receiver was found and notified.
    pub fn resolve(&self, response: &ToolApprovalResponse) -> bool {
        let id = match uuid::Uuid::parse_str(&response.request_id) {
            Ok(id) => id,
            Err(e) => {
                warn!("ApprovalRouter: invalid request_id UUID: {}", e);
                return false;
            }
        };

        match self.pending.remove(&id) {
            Some((_, sender)) => {
                debug!("ApprovalRouter: resolved request {}", id);
                let _ = sender.send(response.clone());
                true
            }
            None => {
                warn!(
                    "ApprovalRouter: no pending request for id {} (maybe timed out)",
                    id
                );
                false
            }
        }
    }

    /// Wait for a response with a timeout.
    ///
    /// Returns `Ok(response)` on success, `Err("timeout")` if the user did not
    /// respond within the given duration.
    pub async fn wait_for_response(
        &self,
        id: uuid::Uuid,
        timeout: Duration,
        rx: oneshot::Receiver<ToolApprovalResponse>,
    ) -> Result<ToolApprovalResponse, String> {
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                // Sender dropped without sending — treat as denied
                self.pending.remove(&id);
                Err("Approval channel closed".to_string())
            }
            Err(_) => {
                self.pending.remove(&id);
                Err("Approval timed out".to_string())
            }
        }
    }
}

impl Default for ApprovalRouter {
    fn default() -> Self {
        Self::new()
    }
}
