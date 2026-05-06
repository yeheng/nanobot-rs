//! Pending-ask coordination types.
//!
//! `ask_user` lets a tool block until the next inbound message on the same
//! session arrives. The trait declared here is the contract between the engine
//! (which owns the registry) and the tool (which awaits the answer). The
//! concrete implementation lives in `gasket-engine`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::events::{ChannelType, InboundMessage, MediaAttachment, SessionKey};

/// Reply to a pending `ask_user`. Returned to the awaiting tool as the
/// `tool_result` payload (after JSON serialization).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskAnswer {
    pub content: String,
    pub sender_id: String,
    pub channel: ChannelType,
    pub timestamp: DateTime<Utc>,
    pub media: Option<Vec<MediaAttachment>>,
}

impl AskAnswer {
    /// Build an `AskAnswer` from a fully-populated inbound message.
    pub fn from_inbound(msg: InboundMessage) -> Self {
        Self {
            content: msg.content,
            sender_id: msg.sender_id,
            channel: msg.channel,
            timestamp: msg.timestamp,
            media: msg.media,
        }
    }

    /// Build an `AskAnswer` synthetically when only `(content, session_key)`
    /// is available (e.g., legacy entry points).
    pub fn synthesize(content: String, key: &SessionKey) -> Self {
        Self {
            content,
            sender_id: key.chat_id.clone(),
            channel: key.channel.clone(),
            timestamp: Utc::now(),
            media: None,
        }
    }
}

/// Per-slot registration data returned to the caller of `register`.
pub struct AskRegistration {
    pub ask_id: uuid::Uuid,
    pub answer_rx: oneshot::Receiver<AskAnswer>,
}

#[derive(Debug, thiserror::Error)]
pub enum AskError {
    #[error("session {0} already has a pending ask")]
    AlreadyPending(SessionKey),
    #[error("ask timed out after {0:?}")]
    Timeout(Duration),
    #[error("ask cancelled: session shutting down")]
    Cancelled,
}

/// Engine-side pending-ask registry, keyed by `SessionKey`.
///
/// **Invariants:**
/// - At most one slot per `SessionKey` is occupied at a time.
/// - Every successful `register` is paired with exactly one of `try_fulfill`
///   (matched by inbound) or `cancel` (timeout / abort).
/// - If the receiver is dropped without `cancel`, `try_fulfill` MUST evict the
///   stale slot (via `Sender::is_closed()` check) so that subsequent
///   `register` calls succeed.
pub trait PendingAskRegistry: Send + Sync {
    /// Reserve the slot for `key` and return a receiver for the answer.
    /// Returns `Err(AlreadyPending)` if the slot is already in use.
    fn register(
        &self,
        key: SessionKey,
        prompt: String,
        deadline: Instant,
    ) -> Result<AskRegistration, AskError>;

    /// Remove a registration (used by tool when timeout fires or future is
    /// cancelled). No-op if `ask_id` does not match the slot's current id.
    fn cancel(&self, key: &SessionKey, ask_id: uuid::Uuid);

    /// Try to deliver `msg` to a pending ask on `key`. On miss returns
    /// `Err(msg)` so the caller can route the message to the normal pipeline.
    /// On stale slot (receiver dropped), evicts the slot and reports miss.
    fn try_fulfill(&self, key: &SessionKey, msg: InboundMessage) -> Result<(), InboundMessage>;
}

/// Convenience: dyn-trait alias used by `ToolContext`.
pub type DynPendingAskRegistry = Arc<dyn PendingAskRegistry>;
