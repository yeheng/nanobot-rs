# Subagent 非阻塞交互 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 spawn/spawn_parallel 工具改为非阻塞模式，前端展示多列网格实时 subagent 执行，全部完成后折叠并综合回复。

**Architecture:** 后端将 spawn 拆成"启动→等待→综合"三阶段，通过 SynthesisCallback trait 注入综合能力。Aggregator 后台 task 等待所有 subagent 完成后通过 provider 发起 LLM 调用，直接通过 outbound_tx 发送 ChatEvent 到 WebSocket。前端新增 SubagentGridPanel 多列网格组件，通过 subagentPhase 状态机驱动展示/折叠/摘要切换。

**Tech Stack:** Rust (tokio, async-trait, serde), Vue 3 (Composition API, reactive refs), TypeScript

**Spec:** `docs/superpowers/specs/2026-04-29-subagent-nonblocking-design.md`

---

## File Structure

### 后端 - 新增文件

| 文件 | 职责 |
|------|------|
| `gasket/engine/src/tools/spawn_common.rs` | 共享逻辑：Aggregator task、event forwarding、非阻塞/阻塞模式分支 |

### 后端 - 修改文件

| 文件 | 职责 |
|------|------|
| `gasket/types/src/events/stream.rs` | 新增 `SubagentAllStarted`, `SubagentSynthesizing` ChatEvent variants + 构造函数 |
| `gasket/types/src/tool.rs` | 新增 `SynthesisCallback` trait + `ToolContext.synthesis_callback` 字段 + builder 方法 |
| `gasket/engine/src/tools/mod.rs` | 新增 `pub mod spawn_common` + re-export |
| `gasket/engine/src/tools/spawn.rs` | 重构为使用 spawn_common，支持非阻塞模式 |
| `gasket/engine/src/tools/spawn_parallel.rs` | 重构为使用 spawn_common，支持非阻塞模式 |
| `gasket/engine/src/kernel/steppable_executor.rs` | 构建 ToolContext 时注入 SynthesisCallback |

### 前端 - 新增文件

| 文件 | 职责 |
|------|------|
| `web/src/components/SubagentGridPanel.vue` | 多列网格展示组件 |

### 前端 - 修改文件

| 文件 | 职责 |
|------|------|
| `web/src/types/index.ts` | 新增 `subagent_all_started`, `subagent_synthesizing` 类型 |
| `web/src/composables/useChatSession.ts` | 新增 `subagentPhase` 状态 + 事件处理 + sendMessage 守卫调整 |
| `web/src/components/MessageBubble.vue` | 集成 SubagentGridPanel（通过 props） |
| `web/src/components/ChatArea.vue` | 移除 SubagentPanel 引用 |

### 前端 - 删除文件

| 文件 | 原因 |
|------|------|
| `web/src/components/SubagentPanel.vue` | 被 SubagentGridPanel + SubagentThoughtsPanel 替代 |

---

## Task 1: 新增 ChatEvent variants

**Files:**
- Modify: `gasket/types/src/events/stream.rs:278` (在 `SubagentError` 之后、`ApprovalRequest` 之前插入)
- Test: `gasket/types/src/events/stream.rs` (新增 tests)

- [ ] **Step 1: 在 ChatEvent enum 中添加两个新 variant**

在 `stream.rs` 的 `ChatEvent` enum 中，`SubagentError` (line ~275) 之后、`ApprovalRequest` (line ~278) 之前插入：

```rust
    /// All subagents have been spawned, main agent turn ends
    SubagentAllStarted {
        count: u32,
    },

    /// All subagents completed, main agent begins synthesis
    SubagentSynthesizing {},
```

- [ ] **Step 2: 添加构造函数**

在 `impl ChatEvent` 中（`subagent_error` 构造函数之后，`approval_request` 之前）添加：

```rust
    pub fn subagent_all_started(count: u32) -> Self {
        Self::SubagentAllStarted { count }
    }

    pub fn subagent_synthesizing() -> Self {
        Self::SubagentSynthesizing {}
    }
```

- [ ] **Step 3: 添加序列化测试**

在 `stream.rs` 底部 `mod tests` 中添加：

```rust
    #[test]
    fn test_subagent_all_started_serialization() {
        let msg = ChatEvent::subagent_all_started(3);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"subagent_all_started""#));
        assert!(json.contains(r#""count":3"#));
    }

    #[test]
    fn test_subagent_synthesizing_serialization() {
        let msg = ChatEvent::subagent_synthesizing();
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"subagent_synthesizing""#));
    }
```

- [ ] **Step 4: 运行测试**

Run: `cargo test --package gasket-types test_subagent_all_started test_subagent_synthesizing`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add gasket/types/src/events/stream.rs
git commit -m "feat(types): add SubagentAllStarted and SubagentSynthesizing ChatEvent variants"
```

---

## Task 2: 新增 SynthesisCallback trait + 扩展 ToolContext

**Files:**
- Modify: `gasket/types/src/tool.rs:122-190` (ToolContext struct + impl)
- Test: `gasket/types/src/tool.rs` (新增 tests)

- [ ] **Step 1: 在 tool.rs 中添加 SynthesisCallback trait**

在 `ToolContext` struct 定义之前（约 line 120）插入：

```rust
/// Callback for synthesizing subagent results into a final response.
///
/// The concrete implementation holds provider, outbound_tx, session_key etc.
/// Returned Future is 'static so it can be safely moved into a tokio::spawn task.
pub trait SynthesisCallback: Send + Sync {
    fn synthesize(
        &self,
        results: Vec<SubagentResult>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>;
}
```

- [ ] **Step 2: 在 ToolContext struct 中添加字段**

在 `ws_summary_limit` 字段之后（约 line 135）添加：

```rust
    /// Callback for triggering synthesis after all subagents complete.
    /// When None (CLI/Telegram/non-WebSocket mode), spawn tools use blocking mode.
    #[serde(skip)]
    pub synthesis_callback: Option<std::sync::Arc<dyn SynthesisCallback>>,
```

- [ ] **Step 3: 更新 ToolContext::default()**

在 `default()` 实现中添加：

```rust
            synthesis_callback: None,
```

- [ ] **Step 4: 添加 builder 方法**

在 `impl ToolContext` 的 `ws_summary_limit` 方法之后添加：

```rust
    pub fn synthesis_callback(mut self, cb: std::sync::Arc<dyn SynthesisCallback>) -> Self {
        self.synthesis_callback = Some(cb);
        self
    }
```

- [ ] **Step 5: 更新 Debug impl**

在 Debug impl 的 `.field()` 链中添加：

```rust
            .field("synthesis_callback", &self.synthesis_callback.is_some())
```

- [ ] **Step 6: 运行编译检查**

Run: `cargo build --package gasket-types`
Expected: 编译通过

- [ ] **Step 7: Commit**

```bash
git add gasket/types/src/tool.rs
git commit -m "feat(types): add SynthesisCallback trait and ToolContext.synthesis_callback field"
```

---

## Task 3: 新增 spawn_common.rs 共享模块

**Files:**
- Create: `gasket/engine/src/tools/spawn_common.rs`
- Modify: `gasket/engine/src/tools/mod.rs` (添加 `pub mod spawn_common`)

- [ ] **Step 1: 创建 spawn_common.rs**

```rust
//! Shared logic for spawn and spawn_parallel tools:
//! - Event forwarding from subagent event_rx → WebSocket outbound_tx
//! - Aggregator background task (wait for results → synthesize)
//! - Blocking vs non-blocking mode branching

use std::sync::Arc;

use gasket_types::{
    events::{ChatEvent, OutboundMessage},
    SubagentResult, SynthesisCallback,
};
use gasket_types::events::SessionKey;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Forward subagent StreamEvents to WebSocket as ChatEvents.
/// Spawned as a background task for each subagent.
pub fn spawn_event_forwarder(
    subagent_id: String,
    mut event_rx: mpsc::Receiver<gasket_types::StreamEvent>,
    session_key: SessionKey,
    outbound_tx: mpsc::Sender<OutboundMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use gasket_types::StreamEventKind;

        while let Some(event) = event_rx.recv().await {
            let chat_event = match &event.kind {
                StreamEventKind::Thinking { content } => {
                    Some(ChatEvent::subagent_thinking(
                        &subagent_id,
                        content.as_ref(),
                    ))
                }
                StreamEventKind::ToolStart { name, arguments } => {
                    Some(ChatEvent::subagent_tool_start(
                        &subagent_id,
                        name.as_ref(),
                        arguments.as_ref().map(|s| s.to_string()),
                    ))
                }
                StreamEventKind::ToolEnd { name, output } => {
                    Some(ChatEvent::subagent_tool_end(
                        &subagent_id,
                        name.as_ref(),
                        output.as_ref().map(|s| s.to_string()),
                    ))
                }
                StreamEventKind::Content { content } => Some(ChatEvent::subagent_content(
                    &subagent_id,
                    content.as_ref(),
                )),
                _ => None,
            };

            if let Some(chat_event) = chat_event {
                let msg = OutboundMessage::with_ws_message(
                    session_key.channel.clone(),
                    session_key.chat_id.clone(),
                    chat_event,
                );
                let _ = outbound_tx.send(msg).await;
            }
        }
    })
}

/// Send a ChatEvent to the WebSocket via outbound_tx.
pub async fn send_ws_event(
    session_key: &SessionKey,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    event: ChatEvent,
) {
    let msg = OutboundMessage::with_ws_message(
        session_key.channel.clone(),
        session_key.chat_id.clone(),
        event,
    );
    let _ = outbound_tx.send(msg).await;
}

/// Result of spawning a single subagent task.
pub struct SpawnedTask {
    pub subagent_id: String,
    pub task_desc: String,
    pub index: u32,
    pub event_forward_handle: tokio::task::JoinHandle<()>,
    pub result_rx: tokio::sync::oneshot::Receiver<gasket_types::SubagentResult>,
}

/// Send startup events synchronously for all spawned tasks.
/// Must be called before returning from execute() to guarantee ordering.
pub async fn send_startup_events(
    session_key: &SessionKey,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    count: usize,
    tasks: &[(String, String, u32)], // (id, task, index)
) {
    send_ws_event(
        session_key,
        outbound_tx,
        ChatEvent::subagent_all_started(count as u32),
    )
    .await;

    for (id, task, index) in tasks {
        send_ws_event(
            session_key,
            outbound_tx,
            ChatEvent::subagent_started(id, task, *index),
        )
        .await;
    }
}

/// Send completion event for a single subagent.
pub async fn send_completion_event(
    session_key: &SessionKey,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    subagent_id: &str,
    index: u32,
    summary: &str,
    tool_count: u32,
) {
    send_ws_event(
        session_key,
        outbound_tx,
        ChatEvent::subagent_completed(subagent_id, index, summary, tool_count),
    )
    .await;
}

/// Spawn the Aggregator background task.
///
/// Waits for all subagent results, then invokes synthesis_callback to produce
/// the final aggregated response. Events are sent directly as ChatEvent
/// through outbound_tx, bypassing the kernel StreamEvent pipeline.
pub fn spawn_aggregator(
    result_receivers: Vec<tokio::sync::oneshot::Receiver<SubagentResult>>,
    synthesis_callback: Arc<dyn SynthesisCallback>,
    cancellation_token: tokio_util::sync::CancellationToken,
    session_key: SessionKey,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    ws_summary_limit: usize,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Wait for all results with a total timeout of 15 minutes
        let timeout = std::time::Duration::from_secs(900);
        let results = tokio::select! {
            result = tokio::time::timeout(timeout, collect_all_results(result_receivers, &session_key, &outbound_tx, ws_summary_limit)) => {
                match result {
                    Ok(results) => results,
                    Err(_) => {
                        warn!("[Aggregator] Timed out waiting for subagent results");
                        send_ws_event(&session_key, &outbound_tx, ChatEvent::error("Subagent aggregation timed out")).await;
                        send_ws_event(&session_key, &outbound_tx, ChatEvent::done()).await;
                        return;
                    }
                }
            }
            _ = cancellation_token.cancelled() => {
                info!("[Aggregator] Cancelled");
                return;
            }
        };

        // Send synthesizing transition event
        send_ws_event(&session_key, &outbound_tx, ChatEvent::subagent_synthesizing()).await;

        // Invoke synthesis
        match synthesis_callback.synthesize(results).await {
            Ok(()) => {}
            Err(e) => {
                warn!("[Aggregator] Synthesis failed: {}", e);
                send_ws_event(&session_key, &outbound_tx, ChatEvent::error(&format!("Synthesis failed: {}", e))).await;
                send_ws_event(&session_key, &outbound_tx, ChatEvent::done()).await;
            }
        }
    })
}

async fn collect_all_results(
    receivers: Vec<tokio::sync::oneshot::Receiver<SubagentResult>>,
    session_key: &SessionKey,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    ws_summary_limit: usize,
) -> Vec<SubagentResult> {
    let mut results = Vec::with_capacity(receivers.len());
    for rx in receivers {
        match rx.await {
            Ok(result) => {
                let summary = if ws_summary_limit == 0 {
                    result.response.content.clone()
                } else {
                    result.response.content.chars().take(ws_summary_limit).collect()
                };
                send_completion_event(
                    session_key,
                    outbound_tx,
                    &result.id,
                    0, // index tracked elsewhere
                    &summary,
                    result.response.tools_used.len() as u32,
                )
                .await;
                results.push(result);
            }
            Err(e) => {
                warn!("[Aggregator] Subagent result channel closed: {}", e);
            }
        }
    }
    results
}
```

- [ ] **Step 2: 在 mod.rs 中注册模块**

在 `gasket/engine/src/tools/mod.rs` 的 `mod` 声明区域添加：

```rust
pub mod spawn_common;
```

- [ ] **Step 3: 运行编译检查**

Run: `cargo build --package gasket-engine`
Expected: 编译通过（可能有未使用的 import warnings，这些是正常的）

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/tools/spawn_common.rs gasket/engine/src/tools/mod.rs
git commit -m "feat(engine): add spawn_common module with event forwarding and aggregator"
```

---

## Task 4: 重构 spawn.rs 为非阻塞模式

**Files:**
- Modify: `gasket/engine/src/tools/spawn.rs`
- Test: `gasket/engine/src/tools/spawn.rs` (修改现有 tests)

- [ ] **Step 1: 重写 spawn.rs**

将 `spawn.rs` 重写为使用 `spawn_common` 模块。核心变化：
- `execute()` 中检查 `ctx.synthesis_callback`
- 若 `Some`: 发送 startup events → 启动 Aggregator → 立即返回
- 若 `None`: 保持现有阻塞行为

```rust
//! Spawn tool for subagent execution
//!
//! Non-blocking mode: sends startup events + Done, returns immediately.
//! Aggregator background task waits for results and triggers synthesis.
//! Blocking mode (CLI/Telegram): waits for result synchronously.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, instrument};

use super::spawn_common;
use super::{format_subagent_response, Tool, ToolContext, ToolError, ToolResult};

pub struct SpawnTool;

impl SpawnTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SpawnTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct SpawnArgs {
    task: String,
    model_id: Option<String>,
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a subagent to execute a task. \
         In WebSocket mode, returns immediately with progress shown in real-time. \
         In CLI mode, blocks until completion."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Task description / prompt to execute"
                },
                "model_id": {
                    "type": "string",
                    "description": "Optional model profile ID to use for this subagent."
                }
            },
            "required": ["task"]
        })
    }

    #[instrument(name = "tool.spawn", skip_all)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let args: SpawnArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        if args.task.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "Task description cannot be empty".to_string(),
            ));
        }

        let spawner = &ctx.spawner;

        info!("[Spawn] Starting subagent for task: {}", args.task);

        let (subagent_id, event_rx, result_rx) = spawner
            .spawn_with_stream(args.task.clone(), args.model_id.clone())
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to spawn subagent: {}", e)))?;

        if let Some(ref callback) = ctx.synthesis_callback {
            // === Non-blocking mode (WebSocket) ===

            // Start event forwarding
            let forward_handle = spawn_common::spawn_event_forwarder(
                subagent_id.clone(),
                event_rx,
                ctx.session_key.clone(),
                ctx.outbound_tx.clone(),
            );

            // Send startup events synchronously (before kernel sends Done)
            spawn_common::send_startup_events(
                &ctx.session_key,
                &ctx.outbound_tx,
                1,
                &[(subagent_id.clone(), args.task.clone(), 0)],
            )
            .await;

            // Launch aggregator
            spawn_common::spawn_aggregator(
                vec![result_rx],
                callback.clone(),
                tokio_util::sync::CancellationToken::new(),
                ctx.session_key.clone(),
                ctx.outbound_tx.clone(),
                ctx.ws_summary_limit,
            );

            let _ = forward_handle.await;

            Ok("Subagent task dispatched. Results will stream in real-time.".to_string())
        } else {
            // === Blocking mode (CLI/Telegram) ===

            // Send started event
            let session_key = ctx.session_key.clone();
            let outbound_tx = ctx.outbound_tx.clone();
            let _ = outbound_tx
                .send(gasket_types::events::OutboundMessage::with_ws_message(
                    session_key.channel.clone(),
                    session_key.chat_id.clone(),
                    gasket_types::events::ChatEvent::subagent_started(
                        subagent_id.clone(),
                        args.task.clone(),
                        0,
                    ),
                ))
                .await;

            // Forward events
            let fwd_id = subagent_id.clone();
            let fwd_sk = session_key.clone();
            let fwd_tx = outbound_tx.clone();
            let forward_handle = tokio::spawn(async move {
                use gasket_types::StreamEventKind;
                while let Some(event) = event_rx.recv().await {
                    let chat_event = match &event.kind {
                        StreamEventKind::Thinking { content } => {
                            Some(gasket_types::events::ChatEvent::subagent_thinking(&fwd_id, content.as_ref()))
                        }
                        StreamEventKind::ToolStart { name, arguments } => {
                            Some(gasket_types::events::ChatEvent::subagent_tool_start(
                                &fwd_id, name.as_ref(), arguments.as_ref().map(|s| s.to_string()),
                            ))
                        }
                        StreamEventKind::ToolEnd { name, output } => {
                            Some(gasket_types::events::ChatEvent::subagent_tool_end(
                                &fwd_id, name.as_ref(), output.as_ref().map(|s| s.to_string()),
                            ))
                        }
                        StreamEventKind::Content { content } => {
                            Some(gasket_types::events::ChatEvent::subagent_content(&fwd_id, content.as_ref()))
                        }
                        _ => None,
                    };
                    if let Some(ce) = chat_event {
                        let msg = gasket_types::events::OutboundMessage::with_ws_message(
                            fwd_sk.channel.clone(), fwd_sk.chat_id.clone(), ce,
                        );
                        let _ = fwd_tx.send(msg).await;
                    }
                }
            });

            let result = result_rx.await.map_err(|e| {
                ToolError::ExecutionError(format!("Subagent result channel closed: {}", e))
            })?;

            // Send completed event
            let summary = if ctx.ws_summary_limit == 0 {
                result.response.content.clone()
            } else {
                result.response.content.chars().take(ctx.ws_summary_limit).collect::<String>()
            };
            let _ = ctx
                .outbound_tx
                .send(gasket_types::events::OutboundMessage::with_ws_message(
                    ctx.session_key.channel.clone(),
                    ctx.session_key.chat_id.clone(),
                    gasket_types::events::ChatEvent::subagent_completed(
                        subagent_id, 0, summary, result.response.tools_used.len() as u32,
                    ),
                ))
                .await;

            let _ = forward_handle.await;
            Ok(format_subagent_response(&result))
        }
    }
}
```

- [ ] **Step 2: 运行现有测试确保通过**

Run: `cargo test --package gasket-engine -- spawn`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add gasket/engine/src/tools/spawn.rs
git commit -m "feat(engine): refactor spawn tool to support non-blocking mode via synthesis_callback"
```

---

## Task 5: 重构 spawn_parallel.rs 为非阻塞模式

**Files:**
- Modify: `gasket/engine/src/tools/spawn_parallel.rs`
- Test: `gasket/engine/src/tools/spawn_parallel.rs` (保留现有 tests)

- [ ] **Step 1: 重写 spawn_parallel.rs**

核心变化与 spawn.rs 相同：检查 `ctx.synthesis_callback`，分非阻塞/阻塞两条路径。
非阻塞路径使用 `spawn_common::send_startup_events` 和 `spawn_common::spawn_aggregator`。

在 `execute()` 方法中：

1. 解析 tasks，spawn 所有 subagent（与现有逻辑相同）
2. 检查 `ctx.synthesis_callback`:
   - `Some(callback)`: 
     - 对每个 subagent 启动 `spawn_common::spawn_event_forwarder`
     - 调用 `spawn_common::send_startup_events` 同步发送启动事件
     - 调用 `spawn_common::spawn_aggregator` 启动后台聚合
     - 返回 `Ok("已启动 N 个并行任务，执行中...")`
   - `None`: 保持现有阻塞逻辑（collect results → aggregate → return）

阻塞模式代码保持与当前完全一致，不需要修改。

- [ ] **Step 2: 运行现有测试确保通过**

Run: `cargo test --package gasket-engine -- spawn_parallel`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add gasket/engine/src/tools/spawn_parallel.rs
git commit -m "feat(engine): refactor spawn_parallel to support non-blocking mode"
```

---

## Task 6: 实现 SynthesisCallback + 注入到 ToolContext

**Files:**
- Create: `gasket/engine/src/kernel/synthesis.rs` (SynthesisCallback 实现)
- Modify: `gasket/engine/src/kernel/mod.rs` (添加 `pub mod synthesis`)
- Modify: `gasket/engine/src/kernel/steppable_executor.rs:160-169` (注入 callback)

- [ ] **Step 1: 创建 synthesis.rs**

```rust
//! SynthesisCallback implementation for subagent result aggregation.

use std::sync::Arc;

use gasket_providers::{ChatMessage, LlmProvider};
use gasket_types::{
    events::{ChatEvent, OutboundMessage, SessionKey},
    SubagentResult, SynthesisCallback,
};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Concrete SynthesisCallback that holds provider + outbound channel.
pub struct WebSocketSynthesizer {
    provider: Arc<dyn LlmProvider>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    session_key: SessionKey,
}

impl WebSocketSynthesizer {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        outbound_tx: mpsc::Sender<OutboundMessage>,
        session_key: SessionKey,
    ) -> Self {
        Self {
            provider,
            outbound_tx,
            session_key,
        }
    }

    async fn send_event(&self, event: ChatEvent) {
        let msg = OutboundMessage::with_ws_message(
            self.session_key.channel.clone(),
            self.session_key.chat_id.clone(),
            event,
        );
        let _ = self.outbound_tx.send(msg).await;
    }
}

impl SynthesisCallback for WebSocketSynthesizer {
    fn synthesize(
        &self,
        results: Vec<SubagentResult>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>> {
        let provider = self.provider.clone();
        let outbound_tx = self.outbound_tx.clone();
        let session_key = self.session_key.clone();

        Box::pin(async move {
            info!(
                "[Synthesizer] Synthesizing {} subagent results",
                results.len()
            );

            // Build synthesis prompt
            let mut prompt = format!(
                "以下是 {} 个并行任务的结果，请综合分析并给出最终回复：\n\n",
                results.len()
            );
            for (idx, result) in results.iter().enumerate() {
                prompt.push_str(&format!("## Task {}\n", idx + 1));
                prompt.push_str(&format!("**任务**: {}\n", result.task));
                if result.response.content.starts_with("Error:") {
                    prompt.push_str(&format!("**结果**: [错误] {}\n\n", result.response.content));
                } else {
                    prompt.push_str(&format!("**结果**: {}\n\n", result.response.content));
                }
            }
            prompt.push_str("请基于以上结果，给出综合性的最终回复。");

            // Call LLM
            let messages = vec![ChatMessage::user(&prompt)];
            let response = provider
                .chat(messages, None)
                .await
                .map_err(|e| anyhow::anyhow!("Synthesis LLM call failed: {}", e))?;

            // Stream the response as a single content event
            let sk = session_key;
            let tx = outbound_tx;

            // Send thinking if reasoning content exists
            if let Some(ref reasoning) = response.reasoning_content {
                let msg = OutboundMessage::with_ws_message(
                    sk.channel.clone(),
                    sk.chat_id.clone(),
                    ChatEvent::thinking(reasoning),
                );
                let _ = tx.send(msg).await;
            }

            // Send content
            let msg = OutboundMessage::with_ws_message(
                sk.channel.clone(),
                sk.chat_id.clone(),
                ChatEvent::content(&response.content),
            );
            let _ = tx.send(msg).await;

            // Send done
            let msg = OutboundMessage::with_ws_message(
                sk.channel,
                sk.chat_id,
                ChatEvent::done(),
            );
            let _ = tx.send(msg).await;

            Ok(())
        })
    }
}
```

- [ ] **Step 2: 在 kernel/mod.rs 中注册**

添加 `pub mod synthesis;`

- [ ] **Step 3: 在 steppable_executor.rs 中注入 callback**

在构建 `ToolContext` 的位置（约 line 160-169），在 `session_key` 设置之后添加：

```rust
        if let Some(ref session_key) = self.ctx.session_key {
            ctx = ctx.session_key(session_key.clone());
            // Inject SynthesisCallback for WebSocket sessions
            if matches!(session_key.channel, gasket_types::events::ChannelType::Websocket) {
                let callback = std::sync::Arc::new(
                    crate::kernel::synthesis::WebSocketSynthesizer::new(
                        self.ctx.provider.clone(),
                        ctx.outbound_tx.clone(),
                        session_key.clone(),
                    ),
                );
                ctx = ctx.synthesis_callback(callback);
            }
        }
```

注意：需要确认 `ChannelType::Websocket` variant 是否存在。如果不存在，需要用其他方式判断（如检查 `session_key.channel` 的值）。

- [ ] **Step 4: 运行编译检查**

Run: `cargo build --package gasket-engine`
Expected: 编译通过

- [ ] **Step 5: Commit**

```bash
git add gasket/engine/src/kernel/synthesis.rs gasket/engine/src/kernel/mod.rs gasket/engine/src/kernel/steppable_executor.rs
git commit -m "feat(engine): implement WebSocketSynthesizer and inject SynthesisCallback into ToolContext"
```

---

## Task 7: 前端类型更新

**Files:**
- Modify: `web/src/types/index.ts:50-64`

- [ ] **Step 1: 更新 SubagentWsMessage 联合类型**

在 `SubagentWsMessage` 类型中添加两个新成员：

```ts
export type SubagentWsMessage =
  | { type: 'subagent_all_started'; count: number }
  | { type: 'subagent_synthesizing' }
  | { type: 'subagent_started'; id: string; task: string; index: number }
  | { type: 'subagent_thinking'; id: string; content: string }
  | { type: 'subagent_content'; id: string; content: string }
  | { type: 'subagent_tool_start'; id: string; name: string; arguments?: string }
  | { type: 'subagent_tool_end'; id: string; tool_id?: string; name: string; output?: string }
  | { type: 'subagent_completed'; id: string; index: number; summary: string; tool_count: number }
  | { type: 'subagent_error'; id: string; index: number; error: string };
```

- [ ] **Step 2: Commit**

```bash
git add web/src/types/index.ts
git commit -m "feat(web): add subagent_all_started and subagent_synthesizing to SubagentWsMessage type"
```

---

## Task 8: 前端 useChatSession.ts 状态扩展

**Files:**
- Modify: `web/src/composables/useChatSession.ts`

- [ ] **Step 1: 添加 subagentPhase ref**

在 `const activeSubagents` 声明之后添加：

```ts
const subagentPhase = ref<'idle' | 'running' | 'synthesizing' | 'completed'>('idle')
```

- [ ] **Step 2: 添加事件处理**

在 `processWebSocketMessageInner` 的 switch 中添加两个新 case（在 `subagent_started` 之前）：

```ts
      case 'subagent_all_started':
        subagentPhase.value = 'running'
        break
```

修改 `done` case：

```ts
      case 'done':
        isThinking.value = false
        // Don't end receiving if subagents are active
        if (activeSubagents.value.size > 0) break
        isReceiving.value = false
        fetchContext()
        break
```

在 `subagent_error` case 之后添加：

```ts
      case 'subagent_synthesizing':
        subagentPhase.value = 'synthesizing'
        setTimeout(() => { subagentPhase.value = 'completed' }, 300)
        break
```

- [ ] **Step 3: 修改 sendMessage 守卫**

将 line 374 的守卫改为：

```ts
    if (!text.trim() || !isConnected.value || isSending.value || (isReceiving.value && subagentPhase.value !== 'running')) return false;
```

在 `sendMessage` 中，发送新消息时清理 subagent 状态。在 `isSending.value = true` 之前添加：

```ts
    // Clear subagent state when user sends during running phase
    if (subagentPhase.value === 'running') {
      activeSubagents.value.clear()
      subagentPhase.value = 'idle'
    }
```

- [ ] **Step 4: 更新 checkAndFinalizeSubagents**

修改 `checkAndFinalizeSubagents` 方法，在全部完成时重置 phase：

```ts
  const checkAndFinalizeSubagents = () => {
    const allCompleted = [...activeSubagents.value.values()].every(s => s.status !== 'running')
    if (allCompleted && activeSubagents.value.size > 0) {
      // Don't clear yet - wait for synthesizing event
      // phase transition handled by subagent_synthesizing event
    }
  }
```

- [ ] **Step 5: 导出 subagentPhase**

在 `return reactive({...})` 的 `// Subagents` 部分添加：

```ts
    subagentPhase,
```

- [ ] **Step 6: 验证编译**

Run: `cd web && npx vue-tsc --noEmit`
Expected: 无类型错误

- [ ] **Step 7: Commit**

```bash
git add web/src/composables/useChatSession.ts
git commit -m "feat(web): add subagentPhase state and handle new subagent events in useChatSession"
```

---

## Task 9: 新增 SubagentGridPanel.vue 组件

**Files:**
- Create: `web/src/components/SubagentGridPanel.vue`

- [ ] **Step 1: 创建 SubagentGridPanel.vue**

多列网格组件，三个 phase：
- `running`: 多列网格，每列实时展示 subagent 执行
- `synthesizing`: 折叠动画 + "正在综合结果..." 提示
- `completed`: 不渲染（由 SubagentThoughtsPanel 接替）

组件接收 `subagents: SubagentState[]` 和 `phase` props。
每列包含：Header（Task 编号 + 描述 + 状态）+ Thinking + Tool Calls（可折叠）+ Content。
使用 Tailwind CSS grid 布局，`grid-cols-N` 根据 subagent 数量动态设置（最多 4 列，超过 4 则换行）。

每列的 content 区域使用 `max-h-64 overflow-y-auto` 保持独立滚动。
Tool call 复用现有 `Collapsible` 组件模式（参考 `SubagentThoughtsPanel.vue` 的实现）。

`synthesizing` phase 时：
1. 网格区域渐隐（`opacity-0` transition）
2. 显示居中提示 "正在综合结果..." + Loader2 动画图标

- [ ] **Step 2: 验证编译**

Run: `cd web && npx vue-tsc --noEmit`
Expected: 无类型错误

- [ ] **Step 3: Commit**

```bash
git add web/src/components/SubagentGridPanel.vue
git commit -m "feat(web): add SubagentGridPanel component for multi-column subagent display"
```

---

## Task 10: 集成 GridPanel 到 MessageBubble + 移除 SubagentPanel

**Files:**
- Modify: `web/src/components/MessageBubble.vue`
- Modify: `web/src/components/ChatArea.vue`
- Delete: `web/src/components/SubagentPanel.vue`

- [ ] **Step 1: 修改 MessageBubble.vue**

在 `SubagentThoughtsPanel` 之前添加 `SubagentGridPanel`：

```vue
import SubagentGridPanel from './SubagentGridPanel.vue'
```

添加 `subagentPhase` prop：

```ts
const props = defineProps<{
  message: Message
  isLastBotMessage: boolean
  isThinking: boolean
  isReceiving: boolean
  subagentPhase: 'idle' | 'running' | 'synthesizing' | 'completed'
}>()
```

在 `SubagentThoughtsPanel` 之前插入：

```vue
        <SubagentGridPanel
          v-if="message.subagents && message.subagents.length > 0 && ['running', 'synthesizing'].includes(subagentPhase)"
          :subagents="message.subagents"
          :phase="subagentPhase as 'running' | 'synthesizing'"
        />
```

调整 `SubagentThoughtsPanel` 显示条件——仅在 `completed` 或 `idle` 且有 subagents 时显示：

```vue
        <SubagentThoughtsPanel
          v-if="message.subagents && message.subagents.length > 0 && !['running', 'synthesizing'].includes(subagentPhase)"
          :subagents="message.subagents"
        />
```

- [ ] **Step 2: 修改 ChatArea.vue**

移除 `SubagentPanel` import 和使用。

删除：
```ts
import SubagentPanel from './SubagentPanel.vue'
```

删除模板中的：
```vue
          <SubagentPanel
            v-if="session.hasActiveSubagents"
            :subagents="session.activeSubagents"
            class="max-w-full md:max-w-4xl lg:max-w-5xl xl:max-w-6xl mx-auto w-full mt-2"
          />
```

传递 `subagentPhase` 到 `MessageBubble`：

```vue
            <MessageBubble
              :message="msg"
              :is-last-bot-message="msg.role === 'bot' && idx === messages.length - 1"
              :is-thinking="session.isThinking"
              :is-receiving="session.isReceiving"
              :subagent-phase="session.subagentPhase"
              @retry="() => retryMessage(msg.id, msg.content)"
            />
```

- [ ] **Step 3: 删除 SubagentPanel.vue**

```bash
rm web/src/components/SubagentPanel.vue
```

- [ ] **Step 4: 验证编译**

Run: `cd web && npx vue-tsc --noEmit`
Expected: 无类型错误

- [ ] **Step 5: Commit**

```bash
git add -A web/src/components/
git commit -m "feat(web): integrate SubagentGridPanel into MessageBubble, remove SubagentPanel"
```

---

## Task 11: 端到端验证

**Files:** 无修改

- [ ] **Step 1: 后端编译测试**

Run: `cargo build --release --workspace`
Expected: 编译通过

- [ ] **Step 2: 后端单元测试**

Run: `cargo test --workspace`
Expected: 全部通过

- [ ] **Step 3: 前端编译测试**

Run: `cd web && npm run build`
Expected: 构建成功

- [ ] **Step 4: 最终 Commit（如有格式修复）**

```bash
git add -A
git commit -m "chore: fix formatting after subagent non-blocking implementation"
```
