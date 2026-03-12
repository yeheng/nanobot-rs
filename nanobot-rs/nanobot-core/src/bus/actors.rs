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

use crate::agent::AgentLoop;
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
) {
    let session_key_str = session_key.to_string();
    tracing::info!("Session Actor [{}] spawned", session_key_str);
    let idle_timeout = Duration::from_secs(3600); // 1 hour idle → self-destruct

    loop {
        match timeout(idle_timeout, rx.recv()).await {
            Ok(Some(msg)) => {
                let channel = msg.channel.clone();
                let chat_id = msg.chat_id.clone();
                let trace_id = msg.trace_id.clone();

                // Check if this is a WebSocket channel for streaming
                #[cfg(feature = "webhook")]
                let is_websocket = matches!(channel, crate::bus::events::ChannelType::WebSocket);
                #[cfg(not(feature = "webhook"))]
                let is_websocket = false;

                tracing::info!(
                    "Session Actor [{}] processing message: channel={:?}, chat_id={}, is_websocket={}",
                    session_key_str, channel, chat_id, is_websocket
                );

                // Process message with streaming for WebSocket
                #[cfg(feature = "webhook")]
                let result = if is_websocket {
                    let ob_tx = outbound_tx.clone();
                    let ch = channel.clone();
                    let cid = chat_id.clone();

                    agent
                        .process_direct_streaming(&msg.content, &session_key, move |event| {
                            let ws_msg = match event {
                                crate::agent::stream::StreamEvent::Content(content) => {
                                    crate::bus::events::WebSocketMessage::content(content)
                                }
                                crate::agent::stream::StreamEvent::Reasoning(content) => {
                                    crate::bus::events::WebSocketMessage::thinking(content)
                                }
                                crate::agent::stream::StreamEvent::ToolStart {
                                    name,
                                    arguments,
                                } => crate::bus::events::WebSocketMessage::tool_start(
                                    name, arguments,
                                ),
                                crate::agent::stream::StreamEvent::ToolEnd { name, output } => {
                                    crate::bus::events::WebSocketMessage::tool_end(
                                        name,
                                        Some(output),
                                    )
                                }
                                crate::agent::stream::StreamEvent::TokenStats { .. } => {
                                    // Skip token stats for now
                                    return;
                                }
                                crate::agent::stream::StreamEvent::Done => {
                                    crate::bus::events::WebSocketMessage::done()
                                }
                            };

                            // Send streaming event to outbound
                            let outbound_msg =
                                OutboundMessage::with_ws_message(ch.clone(), cid.clone(), ws_msg);
                            // Use try_send to avoid blocking the stream
                            let _ = ob_tx.try_send(outbound_msg);
                        })
                        .await
                } else {
                    agent.process_direct(&msg.content, &session_key).await
                };

                #[cfg(not(feature = "webhook"))]
                let result = agent.process_direct(&msg.content, &session_key).await;

                match result {
                    Ok(response) => {
                        // For WebSocket, streaming events already sent via callback
                        // Only send final response for non-WebSocket channels
                        if !is_websocket {
                            let outbound_msg = OutboundMessage {
                                channel,
                                chat_id,
                                content: response.content,
                                metadata: None,
                                trace_id,
                                ws_message: None,
                            };
                            if let Err(e) = outbound_tx.send(outbound_msg).await {
                                tracing::error!(
                                    "Session [{}] failed to send to outbound: {}",
                                    session_key_str,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Agent error in session [{}]: {}", session_key_str, e);
                    }
                }
            }
            Ok(None) => {
                // Channel closed (gateway shutting down)
                tracing::info!("Session [{}] channel closed", session_key_str);
                break;
            }
            Err(_) => {
                // Idle timeout — GC this actor to prevent memory leaks.
                tracing::info!("Session [{}] idle timeout, GC-ing actor", session_key_str);
                break;
            }
        }
    }
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
    mut inbound_rx: mpsc::Receiver<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    agent: Arc<AgentLoop>,
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

            tokio::spawn(run_session_actor(session_key, rx, ob_tx, agent_clone));

            // Send to the freshly created channel (guaranteed to succeed)
            if let Err(e) = tx.send(msg).await {
                tracing::error!("Failed to send to fresh session [{}]: {}", key, e);
            }
            sessions.insert(key, tx);
        }
    }
    tracing::info!("Router Actor shutting down");
}
