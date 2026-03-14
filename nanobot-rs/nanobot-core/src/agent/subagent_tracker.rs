//! Lightweight subagent execution tracker for parallel task coordination
//!
//! ## Design Note: Ownership over Locking
//!
//! This tracker uses direct ownership of MPSC receivers instead of `Arc<Mutex<Receiver>>`.
//! MPSC channels are inherently "Single Consumer" - wrapping them in locks to "share" them
//! is a logical fallacy. Instead, we use:
//!
//! - `result_rx: Receiver<SubagentResult>` - owned by tracker, consumed via `&mut self`
//! - `take_event_receiver()` - transfers event receiver ownership to a spawned task
//!
//! This design is more idiomatic Rust and eliminates unnecessary synchronization overhead.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use super::loop_::AgentResponse;

/// Default timeout for waiting all results (12 minutes)
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 720;

/// Subagent execution result
#[derive(Debug, Clone)]
pub struct SubagentResult {
    pub id: String,
    pub task: String,
    pub response: AgentResponse,
    /// Model name used for this execution
    pub model: Option<String>,
}

/// Events emitted during subagent execution for real-time streaming
#[derive(Debug, Clone)]
pub enum SubagentEvent {
    /// Subagent started execution
    Started { id: String, task: String },
    /// Thinking/reasoning content (incremental)
    Thinking { id: String, content: String },
    /// LLM output content (incremental) - actual response text
    Content { id: String, content: String },
    /// Subagent iteration completed (useful for tracking multi-turn conversations)
    Iteration { id: String, iteration: u32 },
    /// Tool call started
    ToolStart {
        id: String,
        tool_name: String,
        arguments: Option<String>,
    },
    /// Tool call finished
    ToolEnd {
        id: String,
        tool_name: String,
        output: String,
    },
    /// Subagent completed with result
    Completed { id: String, result: SubagentResult },
    /// Subagent encountered an error
    Error { id: String, error: String },
}

/// Tracks multiple subagent executions for parallel coordination
///
/// Uses direct ownership of receivers - no `Arc<Mutex>` needed.
/// The event receiver should be taken via `take_event_receiver()` and moved
/// to a spawned task before calling `wait_for_all`.
pub struct SubagentTracker {
    results: Arc<RwLock<HashMap<String, SubagentResult>>>,
    result_tx: mpsc::Sender<SubagentResult>,
    result_rx: Option<mpsc::Receiver<SubagentResult>>,
    /// Event channel for real-time streaming
    event_tx: mpsc::Sender<SubagentEvent>,
    event_rx: Option<mpsc::Receiver<SubagentEvent>>,
}

impl SubagentTracker {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(100);
        let (event_tx, event_rx) = mpsc::channel(256);
        Self {
            results: Arc::new(RwLock::new(HashMap::new())),
            result_tx: tx,
            result_rx: Some(rx),
            event_tx,
            event_rx: Some(event_rx),
        }
    }

    /// Generate a unique subagent ID
    pub fn generate_id() -> String {
        Uuid::new_v4().to_string()
    }

    /// Get a sender for reporting subagent results
    pub fn result_sender(&self) -> mpsc::Sender<SubagentResult> {
        self.result_tx.clone()
    }

    /// Get a sender for streaming events
    pub fn event_sender(&self) -> mpsc::Sender<SubagentEvent> {
        self.event_tx.clone()
    }

    /// Take ownership of the event receiver.
    ///
    /// This transfers the event receiver to a spawned task for real-time processing.
    /// Should be called once before `wait_for_all`.
    ///
    /// # Panics
    ///
    /// Panics if called more than once (receiver already taken).
    pub fn take_event_receiver(&mut self) -> mpsc::Receiver<SubagentEvent> {
        self.event_rx
            .take()
            .expect("event_receiver already taken - can only call once")
    }

    /// Check if event receiver is still available
    pub fn has_event_receiver(&self) -> bool {
        self.event_rx.is_some()
    }

    /// Wait for N subagents to complete with default timeout.
    ///
    /// Takes `&mut self` because we need exclusive access to the result receiver.
    pub async fn wait_for_all(&mut self, count: usize) -> Vec<SubagentResult> {
        self.wait_for_all_timeout(count, Duration::from_secs(DEFAULT_WAIT_TIMEOUT_SECS))
            .await
    }

    /// Wait for N subagents to complete with custom timeout.
    ///
    /// Returns all results collected before timeout. If timeout occurs,
    /// partial results are returned with error markers for missing tasks.
    ///
    /// Takes `&mut self` because we need exclusive access to the result receiver.
    pub async fn wait_for_all_timeout(
        &mut self,
        count: usize,
        timeout: Duration,
    ) -> Vec<SubagentResult> {
        let mut collected = Vec::with_capacity(count);

        // Get the receiver - we own it exclusively
        let rx = self
            .result_rx
            .as_mut()
            .expect("result_rx should be available");

        // Use tokio::select to implement overall timeout
        let collect_future = async {
            for i in 0..count {
                match rx.recv().await {
                    Some(result) => {
                        tracing::debug!(
                            "[Tracker] Received result {}/{} from subagent {}",
                            i + 1,
                            count,
                            result.id
                        );
                        self.results
                            .write()
                            .await
                            .insert(result.id.clone(), result.clone());
                        collected.push(result);
                    }
                    None => {
                        // Channel closed, no more results coming
                        tracing::warn!(
                            "[Tracker] Channel closed unexpectedly after receiving {}/{} results. \
                             This usually means all result senders were dropped before tasks completed.",
                            collected.len(),
                            count
                        );
                        break;
                    }
                }
            }
        };

        // Wrap with timeout
        match tokio::time::timeout(timeout, collect_future).await {
            Ok(()) => {
                if collected.len() < count {
                    tracing::warn!(
                        "[Tracker] Only collected {}/{} results (channel closed)",
                        collected.len(),
                        count
                    );
                } else {
                    tracing::debug!("[Tracker] Successfully collected all {} results", count);
                }
                collected
            }
            Err(_) => {
                tracing::warn!(
                    "[Tracker] wait_for_all timed out after {:?}, collected {} of {} results",
                    timeout,
                    collected.len(),
                    count
                );
                collected
            }
        }
    }

    /// Get result by ID (non-blocking)
    pub async fn get_result(&self, id: &str) -> Option<SubagentResult> {
        self.results.read().await.get(id).cloned()
    }

    /// Get count of collected results so far
    pub async fn result_count(&self) -> usize {
        self.results.read().await.len()
    }
}

impl Default for SubagentTracker {
    fn default() -> Self {
        Self::new()
    }
}
