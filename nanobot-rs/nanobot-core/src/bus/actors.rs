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
use tokio::time::{interval, timeout, Duration};

use crate::agent::AgentLoop;
use crate::bus::events::{InboundMessage, OutboundMessage, SessionKey};
use crate::config::ChannelsConfig;

// ── Outbound Actor ──────────────────────────────────────────

/// Outbound Actor: dedicated to cross-network HTTP/WebSocket sends.
///
/// Even if Telegram's API blocks for 30 seconds, this never blocks
/// the core AgentLoop or upstream session actors.
pub async fn run_outbound_actor(
    mut rx: mpsc::Receiver<OutboundMessage>,
    config: Arc<ChannelsConfig>,
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
            }
            continue;
        }

        let cfg = config.clone();
        // Fire-and-forget: each send runs in its own task,
        // eliminating Head-of-Line Blocking across messages.
        tokio::spawn(async move {
            if let Err(e) = crate::channels::send_outbound(&cfg, msg).await {
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
pub async fn run_session_actor(
    session_key: SessionKey,
    mut rx: mpsc::Receiver<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    agent: Arc<AgentLoop>,
) {
    let session_key_str = session_key.to_string();
    tracing::debug!("Session Actor [{}] spawned", session_key_str);
    let idle_timeout = Duration::from_secs(3600); // 1 hour idle → self-destruct

    loop {
        match timeout(idle_timeout, rx.recv()).await {
            Ok(Some(msg)) => {
                // Serial processing: no locks needed, only one message at a time.
                match agent.process_direct(&msg.content, &session_key).await {
                    Ok(response) => {
                        // Forward to Outbound Actor — returns immediately, no network wait.
                        let outbound_msg = OutboundMessage {
                            channel: msg.channel,
                            chat_id: msg.chat_id,
                            content: response.content,
                            metadata: None,
                            trace_id: msg.trace_id,
                        };
                        if let Err(e) = outbound_tx.send(outbound_msg).await {
                            tracing::error!(
                                "Session [{}] failed to send to outbound: {}",
                                session_key_str,
                                e
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!("Agent error in session [{}]: {}", session_key_str, e);
                    }
                }
            }
            Ok(None) => {
                // Channel closed (gateway shutting down)
                tracing::debug!("Session [{}] channel closed", session_key_str);
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
/// **GC mechanism**: Every 10 minutes, scans the routing table and removes
/// closed channels (zombie entries from sessions that self-destructed but
/// were never re-contacted by their users).
pub async fn run_router_actor(
    mut inbound_rx: mpsc::Receiver<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    agent: Arc<AgentLoop>,
) {
    tracing::info!("Router Actor started");
    // Plain HashMap — only this task touches it. No locks, no DashMap.
    let mut sessions: HashMap<String, mpsc::Sender<InboundMessage>> = HashMap::new();
    // GC interval: clean up zombie channels every 10 minutes
    let mut cleanup_interval = interval(Duration::from_secs(600));

    loop {
        tokio::select! {
            // Primary path: receive and route inbound messages
            Some(msg) = inbound_rx.recv() => {
                let key = msg.session_key().to_string();

                let mut needs_respawn = true;
                if let Some(tx) = sessions.get(&key) {
                    // Try to send. If Err, the session actor has already self-destructed.
                    if tx.send(msg.clone()).await.is_ok() {
                        needs_respawn = false;
                    } else {
                        tracing::debug!("Session [{}] channel dead, respawning...", key);
                    }
                }

                if needs_respawn {
                    let (tx, rx) = mpsc::channel(32);
                    let ob_tx = outbound_tx.clone();
                    let agent_clone = agent.clone();
                    let session_key = SessionKey::from(key.clone());

                    // Spawn a new session actor
                    tokio::spawn(run_session_actor(session_key, rx, ob_tx, agent_clone));

                    // Send to the freshly created channel (guaranteed to succeed)
                    if let Err(e) = tx.send(msg).await {
                        tracing::error!("Failed to send to fresh session [{}]: {}", key, e);
                    }
                    sessions.insert(key, tx);
                }
            }
            // GC path: periodically remove dead channels
            _ = cleanup_interval.tick() => {
                let before = sessions.len();
                // tx.is_closed() is O(1) — cheap check for dead channels
                sessions.retain(|_, tx| !tx.is_closed());
                let after = sessions.len();
                if before != after {
                    tracing::debug!(
                        "Router GC: removed {} zombie session channels ({} → {})",
                        before - after,
                        before,
                        after
                    );
                }
            }
            // Shutdown path: inbound channel closed
            else => break,
        }
    }
    tracing::info!("Router Actor shutting down");
}
