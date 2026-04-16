//! JSON-RPC daemon runner — long-lived script process with request multiplexing.
//!
//! Replaces the one-shot `run_jsonrpc` with a persistent process that handles
//! multiple requests over the same stdin/stdout pipe, eliminating cold-start
//! overhead on every tool invocation.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::plugin::dispatcher::{DispatcherContext, RpcDispatcher};
use crate::plugin::manifest::{Permission, PluginManifest};
use crate::plugin::rpc::{decode, encode, RpcMessage, RpcRequest, RpcResponse, MAX_MESSAGE_SIZE};
use crate::plugin::runner::simple::spawn_process;
use crate::plugin::runner::{PluginError, PluginResult};

/// Persistent JSON-RPC script process.
pub struct JsonRpcDaemon {
    pending: Arc<DashMap<i64, oneshot::Sender<RpcResponse>>>,
    write_tx: mpsc::UnboundedSender<RpcMessage>,
    next_id: AtomicI64,
    last_used_ms: AtomicI64,
    idle_timeout_ms: i64,
    stderr: Arc<tokio::sync::Mutex<String>>,
    alive: Arc<AtomicBool>,
}

impl JsonRpcDaemon {
    /// Spawn a new persistent JSON-RPC process.
    pub async fn spawn(
        manifest: &PluginManifest,
        manifest_dir: &Path,
        idle_timeout_secs: u64,
        permissions: &[Permission],
        dispatcher: &RpcDispatcher,
        ctx: &DispatcherContext,
    ) -> Result<Self, PluginError> {
        let mut child = spawn_process(manifest, manifest_dir)?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PluginError::Io("Failed to open stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PluginError::Io("Failed to open stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| PluginError::Io("Failed to open stderr".to_string()))?;

        let pending: Arc<DashMap<i64, oneshot::Sender<RpcResponse>>> = Arc::new(DashMap::new());
        let (write_tx, mut write_rx) = mpsc::unbounded_channel::<RpcMessage>();
        let alive = Arc::new(AtomicBool::new(true));

        // ── Writer task ──────────────────────────────────────────
        let mut stdin = stdin;
        tokio::spawn(async move {
            while let Some(msg) = write_rx.recv().await {
                let encoded = encode(&msg);
                if stdin.write_all(encoded.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        // ── Stderr collector ─────────────────────────────────────
        let stderr_buffer = Arc::new(tokio::sync::Mutex::new(String::new()));
        let stderr_buffer_clone = stderr_buffer.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        stderr_buffer_clone.lock().await.push_str(&line);
                        line.clear();
                    }
                    Err(_) => break,
                }
            }
        });

        // ── Reader task ──────────────────────────────────────────
        let pending_reader = pending.clone();
        let permissions = permissions.to_vec();
        let dispatcher = dispatcher.clone();
        let ctx = ctx.clone();
        let write_tx_clone = write_tx.clone();
        let alive_reader = alive.clone();
        tokio::spawn(async move {
            let mut reader_lines =
                FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_MESSAGE_SIZE));
            while let Some(result) = reader_lines.next().await {
                match result {
                    Ok(line) => {
                        let msg = decode(&line);
                        let msg = match msg {
                            Some(m) => m,
                            None => continue,
                        };
                        match msg {
                            RpcMessage::Request(request) => {
                                let response =
                                    dispatcher.dispatch(request, &permissions, &ctx).await;
                                let _ = write_tx_clone.send(RpcMessage::Response(response));
                            }
                            RpcMessage::Response(response) => {
                                if let Some(id) = response.id.as_i64() {
                                    if let Some((_, tx)) = pending_reader.remove(&id) {
                                        let _ = tx.send(response);
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            alive_reader.store(false, Ordering::Relaxed);
            // stdout closed — process is dead or dying
            let _ = child.kill().await;
        });

        Ok(Self {
            pending,
            write_tx,
            next_id: AtomicI64::new(1),
            last_used_ms: AtomicI64::new(chrono::Utc::now().timestamp_millis()),
            idle_timeout_ms: (idle_timeout_secs as i64) * 1000,
            stderr: stderr_buffer,
            alive,
        })
    }

    /// Execute a single JSON-RPC call through the persistent process.
    pub async fn call(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<PluginResult, PluginError> {
        if !self.alive.load(Ordering::Relaxed) {
            return Err(PluginError::Io(
                "JSON-RPC daemon process has exited".to_string(),
            ));
        }

        // Backward compatibility: initialize always uses id 0, matching the
        // original one-shot run_jsonrpc behaviour.
        let id = if method == "initialize" {
            0
        } else {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        };
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);

        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(id.into())),
            method: method.to_string(),
            params,
        };
        self.write_tx
            .send(RpcMessage::Request(request))
            .map_err(|_| PluginError::Io("Daemon write channel closed".to_string()))?;

        self.touch();

        let timeout_duration = Duration::from_millis(self.idle_timeout_ms.max(5000) as u64);
        let response = tokio::time::timeout(timeout_duration, rx)
            .await
            .map_err(|_| PluginError::Timeout(self.idle_timeout_ms.max(5000) as u64 / 1000))?
            .map_err(|_| PluginError::Io("Daemon response channel closed".to_string()))?;

        if let Some(error) = response.error {
            return Err(PluginError::InvalidOutput(format!(
                "JSON-RPC error: {} (code {})",
                error.message, error.code
            )));
        }

        let output = response.result.ok_or_else(|| {
            PluginError::InvalidOutput("No result received from daemon".to_string())
        })?;

        let stderr = self.stderr.lock().await.clone();

        Ok(PluginResult {
            output,
            stderr,
            duration: Duration::default(),
        })
    }

    /// Returns true if the daemon has been idle longer than its timeout.
    pub fn is_idle_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp_millis();
        let last = self.last_used_ms.load(Ordering::Relaxed);
        now - last > self.idle_timeout_ms
    }

    fn touch(&self) {
        self.last_used_ms
            .store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
    }
}
