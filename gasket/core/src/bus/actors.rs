//! Actor-based message pipeline for the gateway.
//!
//! Three actors form a clean pipeline with zero locks:
//!
//! ```text
//! Inbound → [Router Actor] → per-session channel → [Session Actor] → [Outbound Actor] → HTTP
//! ```
//!
//! - **Router Actor**: Dispatches inbound messages to per-session channels.
//!   Owns the session routing table (plain `HashMap`, single-threaded).
//!   Respawns session actors on dead channels.
//!
//! - **Session Actor**: Processes messages serially for a single session_key.
//!   Uses the shared `Arc<AgentLoop>` — no per-session duplication.
//!   Self-destructs after idle timeout (default: 1 hour).
//!
//! - **Outbound Actor**: Receives outbound messages and fires concurrent HTTP sends.
//!   Never blocks upstream — each send is a fire-and-forget `tokio::spawn`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

use crate::agent::{AgentLoop, SubagentManager};
use crate::bus::events::{InboundMessage, OutboundMessage, SessionKey};
use crate::channels::OutboundSenderRegistry;

// ── Outbound Actor ──────────────────────────────────────────

/// Outbound Actor: dedicated to cross-network HTTP/WebSocket sends.
///
/// Even if Telegram's API blocks for 30 seconds, this never blocks
/// the core AgentLoop or upstream session actors.
///
/// Uses `OutboundSenderRegistry` for extensible routing, supporting
/// both built-in channels and custom channels registered at runtime.
pub async fn run_outbound_actor(
    mut rx: mpsc::Receiver<OutboundMessage>,
    registry: Arc<OutboundSenderRegistry>,
    #[cfg(feature = "webhook")] websocket_manager: Option<
        Arc<crate::channels::websocket::WebSocketManager>,
    >,
) {
    tracing::info!("Outbound Actor started");
    while let Some(msg) = rx.recv().await {
        #[cfg(feature = "webhook")]
        if let crate::bus::events::ChannelType::WebSocket = msg.channel {
            if let Some(ref manager) = websocket_manager {
                manager.send(msg).await;
            } else {
                tracing::warn!(
                    "Outbound Actor: websocket_manager is None, cannot send WebSocket message"
                );
            }
            continue;
        }

        let reg = registry.clone();
        // Fire-and-forget: each send runs in its own task,
        // eliminating Head-of-Line Blocking across messages.
        tokio::spawn(async move {
            if let Err(e) = reg.send(msg).await {
                tracing::error!("Outbound delivery failed: {}", e);
            }
        });
    }
    tracing::info!("Outbound Actor shutting down");
}

// ── Session Actor ───────────────────────────────────────────

/// Session Actor: serial execution for a single session_key.
///
/// **Key design decision**: shares `Arc<AgentLoop>` instead of owning
/// a dedicated instance. AgentLoop is stateless per-session — all
/// per-session data lives in SQLite, keyed by `session_key`.
/// This avoids duplicating SQLite connections, MemoryStore, and
/// SummarizationService across sessions.
///
/// Self-destructs after `idle_timeout` of inactivity, freeing memory.
///
/// ## WebSocket Streaming
///
/// For WebSocket channels, this actor sends real-time streaming events:
/// - `thinking`: LLM reasoning content
/// - `tool_start`: Tool call initiated
/// - `tool_end`: Tool execution completed
/// - `content`: Streaming text content
/// - `done`: Stream completed
pub async fn run_session_actor(
    session_key: SessionKey,
    mut rx: mpsc::Receiver<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    agent: Arc<AgentLoop>,
    subagent_manager: Option<Arc<SubagentManager>>,
    idle_timeout: Duration,
) {
    let session_key_str = session_key.to_string();
    tracing::info!("Session Actor [{}] spawned", session_key_str);

    loop {
        let msg = match timeout(idle_timeout, rx.recv()).await {
            Ok(Some(msg)) => msg,
            Ok(None) => {
                tracing::info!("Session [{}] channel closed", session_key_str);
                break;
            }
            Err(_) => {
                tracing::info!("Session [{}] idle timeout, GC-ing actor", session_key_str);
                break;
            }
        };

        // Use RAII guard for session key management
        // The guard automatically clears the session key when dropped,
        // even if processing panics
        let _guard = subagent_manager
            .as_ref()
            .map(|m| m.session_key_guard(session_key.clone()));

        // Process message and handle result immediately (avoid holding non-Send across await)
        match process_session_message(
            msg,
            &session_key,
            &agent,
            &outbound_tx,
            subagent_manager.as_ref().map(|m| m.as_ref()),
        )
        .await
        {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("Session [{}] error: {}", session_key_str, e);
            }
        }
        // _guard is automatically dropped here, clearing the session key
    }
}

async fn process_session_message(
    msg: InboundMessage,
    session_key: &SessionKey,
    agent: &Arc<AgentLoop>,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    subagent_manager: Option<&SubagentManager>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Route based on channel capability (streaming support)
    if msg.channel.supports_streaming() {
        process_streaming_message(msg, session_key, agent, outbound_tx, subagent_manager).await
    } else {
        process_regular_message(msg, session_key, agent, outbound_tx, subagent_manager).await
    }
}

/// Process message with real-time streaming (for channels that support it).
///
/// Streaming channels receive incremental LLM output and forward events
/// to the client in real-time (thinking, content, tool events).
///
/// ## Backpressure
///
/// This function now uses `process_direct_streaming_with_channel` which returns
/// an `mpsc::Receiver<StreamEvent>`. We can await each send, providing proper
/// backpressure — no more `try_send` dropping messages.
async fn process_streaming_message(
    msg: InboundMessage,
    session_key: &SessionKey,
    agent: &Arc<AgentLoop>,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    _subagent_manager: Option<&SubagentManager>,
) -> Result<(), Box<dyn std::error::Error>> {
    use tracing::{debug, info};

    let channel = msg.channel.clone();
    let chat_id = msg.chat_id.clone();
    let session_key_str = session_key.to_string();

    info!(
        "[Streaming] Processing message for session: {}",
        session_key_str
    );

    // Use the new channel-based API for proper backpressure
    let (mut event_rx, result_handle) = agent
        .process_direct_streaming_with_channel(&msg.content, session_key)
        .await?;

    // Consume events with proper awaiting
    let mut event_count = 0usize;
    while let Some(event) = event_rx.recv().await {
        event_count += 1;
        if event_count == 1 {
            debug!(
                "[Streaming] First event received for session: {}",
                session_key_str
            );
        }
        if let Some(ws_msg) = stream_event_to_ws_message(event) {
            let outbound_msg =
                OutboundMessage::with_ws_message(channel.clone(), chat_id.clone(), ws_msg);
            // Use .await instead of try_send - proper backpressure!
            outbound_tx.send(outbound_msg).await?;
        }
    }

    // Wait for the final result
    let _response = result_handle.await??;

    info!(
        "[Streaming] Streaming completed for session: {}, total events: {}",
        session_key_str, event_count
    );

    Ok(())
}

/// Convert a StreamEvent to a WebSocketMessage.
///
/// Returns None for events that should not be forwarded (e.g., TokenStats).
fn stream_event_to_ws_message(
    event: crate::agent::stream::StreamEvent,
) -> Option<crate::bus::events::WebSocketMessage> {
    use crate::agent::stream::StreamEvent;
    use crate::bus::events::WebSocketMessage;

    match event {
        StreamEvent::Content(content) => Some(WebSocketMessage::content(content)),
        StreamEvent::Reasoning(content) => Some(WebSocketMessage::thinking(content)),
        StreamEvent::ToolStart { name, arguments } => {
            Some(WebSocketMessage::tool_start(name, arguments))
        }
        StreamEvent::ToolEnd { name, output } => {
            Some(WebSocketMessage::tool_end(name, Some(output)))
        }
        StreamEvent::Done => Some(WebSocketMessage::done()),
        StreamEvent::TokenStats { .. } => None,
    }
}

async fn process_regular_message(
    msg: InboundMessage,
    session_key: &SessionKey,
    agent: &Arc<AgentLoop>,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    _subagent_manager: Option<&SubagentManager>,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = agent.process_direct(&msg.content, session_key).await?;

    let outbound_msg = OutboundMessage {
        channel: msg.channel,
        chat_id: msg.chat_id,
        content: response.content,
        metadata: None,
        trace_id: msg.trace_id,
        ws_message: None,
    };

    outbound_tx.send(outbound_msg).await?;
    Ok(())
}

// ── Router Actor ────────────────────────────────────────────

/// Router Actor: dispatches inbound messages to per-session actors.
///
/// Owns the session routing table as a plain `HashMap` — no locks needed
/// because only this single task mutates it.
///
/// On dead channels (session actor timed out and dropped its receiver),
/// automatically respawns a fresh session actor.
///
/// **GC mechanism**: Passive cleanup on send failure. When `tx.send()` fails,
/// the dead entry gets replaced with a fresh session actor. No polling needed —
/// if a session never receives another message, its HashMap entry is harmless.
pub async fn run_router_actor(
    inbound_rx: mpsc::Receiver<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    agent: Arc<AgentLoop>,
    subagent_manager: Option<Arc<SubagentManager>>,
) {
    run_router_actor_with_timeout(
        inbound_rx,
        outbound_tx,
        agent,
        subagent_manager,
        Duration::from_secs(crate::agent::loop_::DEFAULT_SESSION_IDLE_TIMEOUT_SECS),
    )
    .await
}

/// Router actor with configurable session idle timeout.
pub async fn run_router_actor_with_timeout(
    mut inbound_rx: mpsc::Receiver<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    agent: Arc<AgentLoop>,
    subagent_manager: Option<Arc<SubagentManager>>,
    idle_timeout: Duration,
) {
    tracing::info!("Router Actor started");
    let mut sessions: HashMap<SessionKey, mpsc::Sender<InboundMessage>> = HashMap::new();

    while let Some(msg) = inbound_rx.recv().await {
        let key = msg.session_key().clone();

        let mut needs_respawn = true;
        if let Some(tx) = sessions.get(&key) {
            if tx.send(msg.clone()).await.is_ok() {
                needs_respawn = false;
            } else {
                tracing::info!("Session [{}] channel dead, respawning...", key);
            }
        }

        if needs_respawn {
            let (tx, rx) = mpsc::channel(32);
            let ob_tx = outbound_tx.clone();
            let agent_clone = agent.clone();
            let session_key = key.clone();
            let manager_clone = subagent_manager.clone();

            tokio::spawn(run_session_actor(
                session_key,
                rx,
                ob_tx,
                agent_clone,
                manager_clone,
                idle_timeout,
            ));

            // Send to the freshly created channel (guaranteed to succeed)
            if let Err(e) = tx.send(msg).await {
                tracing::error!("Failed to send to fresh session [{}]: {}", key, e);
            }
            sessions.insert(key, tx);
        }
    }
    tracing::info!("Router Actor shutting down");
}
