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
//!   Uses a MessageHandler trait for processing — decoupled from AgentLoop.
//!   Self-destructs after idle timeout (default: 1 hour).
//!
//! - **Outbound Actor**: Receives outbound messages and fires concurrent HTTP sends.
//!   Never blocks upstream — each send is a fire-and-forget `tokio::spawn`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

use gasket_types::events::{InboundMessage, OutboundMessage, SessionKey, WebSocketMessage};

/// Message handler trait for processing session messages.
///
/// This trait allows decoupling the session actor from the concrete AgentLoop,
/// enabling dependency injection and easier testing.
#[async_trait::async_trait]
pub trait MessageHandler: Send + Sync {
    /// Process a message and return the response content.
    async fn handle_message(
        &self,
        session_key: &SessionKey,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    /// Process a message with streaming support.
    ///
    /// Returns a receiver for stream events and a handle for the final result.
    async fn handle_streaming_message(
        &self,
        message: &str,
        session_key: &SessionKey,
    ) -> Result<
        (
            mpsc::Receiver<StreamEvent>,
            tokio::sync::oneshot::Receiver<Result<OutboundMessage, Box<dyn std::error::Error + Send + Sync>>>,
        ),
        Box<dyn std::error::Error + Send + Sync>,
    >;
}

/// Stream events for real-time output.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Content text
    Content(String),
    /// Reasoning/thinking content
    Reasoning(String),
    /// Tool call started
    ToolStart { name: String, arguments: String },
    /// Tool call completed
    ToolEnd { name: String, output: String },
    /// Stream completed
    Done,
    /// Token usage statistics
    TokenStats { prompt: usize, completion: usize, total: usize },
}

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
    registry: Arc<dyn OutboundSender + Send + Sync>,
) {
    tracing::info!("Outbound Actor started");
    while let Some(msg) = rx.recv().await {
        let reg = registry.clone();
        // Fire-and-forget: each send runs in its own task,
        // eliminating Head-of-Line Blocking across messages.
        tokio::spawn(async move {
            if let Err(e) = reg.send_outbound(msg).await {
                tracing::error!("Outbound delivery failed: {}", e);
            }
        });
    }
    tracing::info!("Outbound Actor shutting down");
}

/// Trait for outbound message sending.
///
/// This allows decoupling the outbound actor from the concrete OutboundSenderRegistry.
#[async_trait::async_trait]
pub trait OutboundSender {
    async fn send_outbound(&self, msg: OutboundMessage) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

// ── Session Actor ───────────────────────────────────────────

/// Session Actor: serial execution for a single session_key.
///
/// **Key design decision**: Uses MessageHandler trait instead of owning
/// a concrete AgentLoop. This enables dependency injection and testing.
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
pub async fn run_session_actor<H>(
    session_key: SessionKey,
    mut rx: mpsc::Receiver<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    handler: Arc<H>,
    idle_timeout: Duration,
) where
    H: MessageHandler + 'static,
{
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

        // Process message and handle result immediately (avoid holding non-Send across await)
        match process_session_message(msg, &session_key, &handler, &outbound_tx).await {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("Session [{}] error: {}", session_key_str, e);
            }
        }
    }
}

async fn process_session_message<H>(
    msg: InboundMessage,
    session_key: &SessionKey,
    handler: &Arc<H>,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    H: MessageHandler + 'static,
{
    // Route based on channel capability (streaming support)
    if msg.channel.supports_streaming() {
        process_streaming_message(msg, session_key, handler, outbound_tx).await
    } else {
        process_regular_message(msg, session_key, handler, outbound_tx).await
    }
}

/// Process message with real-time streaming (for channels that support it).
///
/// Streaming channels receive incremental LLM output and forward events
/// to the client in real-time (thinking, content, tool events).
///
/// ## Backpressure
///
/// This function uses `handle_streaming_message` which returns
/// an `mpsc::Receiver<StreamEvent>`. We can await each send, providing proper
/// backpressure — no more `try_send` dropping messages.
async fn process_streaming_message<H>(
    msg: InboundMessage,
    session_key: &SessionKey,
    handler: &Arc<H>,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    H: MessageHandler + 'static,
{
    use tracing::{debug, info};

    let channel = msg.channel.clone();
    let chat_id = msg.chat_id.clone();
    let session_key_str = session_key.to_string();

    info!(
        "[Streaming] Processing message for session: {}",
        session_key_str
    );

    // Use the channel-based API for proper backpressure
    let (mut event_rx, result_handle) = handler.handle_streaming_message(&msg.content, session_key).await?;

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
fn stream_event_to_ws_message(event: StreamEvent) -> Option<WebSocketMessage> {
    match event {
        StreamEvent::Content(content) => Some(WebSocketMessage::content(content)),
        StreamEvent::Reasoning(content) => Some(WebSocketMessage::thinking(content)),
        StreamEvent::ToolStart { name, arguments } => {
            Some(WebSocketMessage::tool_start(name, Some(arguments)))
        }
        StreamEvent::ToolEnd { name, output } => {
            Some(WebSocketMessage::tool_end(name, Some(output)))
        }
        StreamEvent::Done => Some(WebSocketMessage::done()),
        StreamEvent::TokenStats { .. } => None,
    }
}

async fn process_regular_message<H>(
    msg: InboundMessage,
    session_key: &SessionKey,
    handler: &Arc<H>,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    H: MessageHandler + 'static,
{
    let content = handler.handle_message(session_key, &msg.content).await?;

    let outbound_msg = OutboundMessage {
        channel: msg.channel,
        chat_id: msg.chat_id,
        content,
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
pub async fn run_router_actor<H>(
    inbound_rx: mpsc::Receiver<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    handler: Arc<H>,
) where
    H: MessageHandler + 'static,
{
    run_router_actor_with_timeout(
        inbound_rx,
        outbound_tx,
        handler,
        Duration::from_secs(3600), // Default 1 hour idle timeout
    )
    .await
}

/// Router actor with configurable session idle timeout.
pub async fn run_router_actor_with_timeout<H>(
    mut inbound_rx: mpsc::Receiver<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    handler: Arc<H>,
    idle_timeout: Duration,
) where
    H: MessageHandler + 'static,
{
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
            let handler_clone = handler.clone();
            let session_key = key.clone();

            tokio::spawn(run_session_actor(
                session_key,
                rx,
                ob_tx,
                handler_clone,
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
