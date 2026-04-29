# Subagent 非阻塞交互重设计

**日期**: 2026-04-29
**状态**: Approved

## 1. 概述

将 `spawn` / `spawn_parallel` 工具从阻塞式改为非阻塞等待模式。主 agent 调用 spawn 后立即结束本轮，前端展示多列网格实时呈现各 subagent 的 thinking/tool calls/content，全部完成后自动折叠为可展开摘要，主 agent 再发起综合回复。

### 设计决策记录

| 决策项 | 选择 | 理由 |
|--------|------|------|
| 阻塞模式 | 非阻塞等待 | 前端感知立即完成，后端仍等结果 |
| 面板布局 | 聊天区内嵌多列网格 | 每列一个 subagent，实时滚动 |
| 折叠行为 | 自动折叠为可展开摘要 | 折叠后紧凑行，点击可展开查看详情 |
| 汇合过渡 | 显式提示 | 显示"正在综合结果..." |
| Provider 访问 | SynthesisCallback trait | 保持 ToolContext 简洁 |
| 用户中断 | 允许输入，丢弃旧结果 | 不阻塞用户 |

## 2. 后端事件协议

### 2.1 新增 ChatEvent variants

```rust
/// 所有 subagent 已启动，主 agent 本轮结束
SubagentAllStarted {
    count: u32,
}

/// 所有 subagent 完成，主 agent 开始综合分析
SubagentSynthesizing {}
```

同时更新前端类型：
- `web/src/types/index.ts` 的 `SubagentWsMessage` 联合类型新增：
  ```ts
  | { type: 'subagent_all_started'; count: number }
  | { type: 'subagent_synthesizing' }
  ```

### 2.2 完整事件流时序

```
主 agent 调用 spawn_parallel
  ├─ ChatEvent::ToolStart { name: "spawn_parallel" }
  ├─ [execute() 同步发送] ChatEvent::SubagentAllStarted { count: N }
  ├─ [execute() 同步发送] ChatEvent::SubagentStarted { id, task, index } × N
  ├─ ChatEvent::ToolEnd { name: "spawn_parallel", output: "已启动 N 个并行任务" }
  ├─ [kernel loop] ChatEvent::Done                               ← 主 agent 本轮结束
  │
  │  ── 前端进入网格展示模式（Done 时 activeSubagents.size > 0） ──
  │
  ├─ [Aggregator 后台 task] ChatEvent::SubagentThinking ...      ← 实时流式
  ├─ [Aggregator 后台 task] ChatEvent::SubagentToolStart ...
  ├─ [Aggregator 后台 task] ChatEvent::SubagentToolEnd ...
  ├─ [Aggregator 后台 task] ChatEvent::SubagentContent ...
  ├─ [Aggregator 后台 task] ChatEvent::SubagentCompleted ... × N
  │
  ├─ [Aggregator 后台 task] ChatEvent::SubagentSynthesizing {}   ← 前端折叠 + 过渡提示
  ├─ [Aggregator 直接通过 outbound_tx] ChatEvent::Thinking ...    ← 主 agent 综合思考
  ├─ [Aggregator 直接通过 outbound_tx] ChatEvent::Content ...     ← 主 agent 流式输出
  ├─ [Aggregator 直接通过 outbound_tx] ChatEvent::Done            ← 真正结束
  ```

### 2.3 事件排序保证

**关键约束**：`SubagentAllStarted` 和所有 `SubagentStarted` 事件必须在 `execute()` 返回前**同步发送**到 `outbound_tx`。这保证它们先于 kernel loop 发出的 `Done` 到达前端。

实现方式：`spawn_with_stream()` 返回 `(subagent_id, event_rx, result_rx)` 后，`execute()` 立即通过 `outbound_tx` 发送 startup 事件，然后再 spawn event-forwarding task。不依赖 tokio task 的调度顺序。

## 3. 后端执行模型

### 3.1 改动文件

| 文件 | 变更 |
|------|------|
| `gasket/types/src/events/stream.rs` | 新增 `SubagentAllStarted`, `SubagentSynthesizing` |
| `gasket/types/src/tool.rs` | 新增 `SynthesisCallback` trait 和 `ToolContext` 字段 |
| `gasket/engine/src/tools/spawn.rs` | 重构为非阻塞模式 |
| `gasket/engine/src/tools/spawn_parallel.rs` | 重构为非阻塞模式 |
| `gasket/engine/src/tools/spawn_common.rs` | 新增共享逻辑（Aggregator、event forwarding） |
| `gasket/engine/src/kernel/steppable_executor.rs` | 构建 ToolContext 时注入 SynthesisCallback |

### 3.2 SynthesisCallback trait

```rust
/// types/src/tool.rs
///
/// Callback for synthesizing subagent results into a final response.
/// The concrete implementation holds a provider, outbound_tx, and session_key.
///
/// Returned Future is 'static (no borrow from &self) so it can be
/// safely moved into a tokio::spawn task.
pub trait SynthesisCallback: Send + Sync {
    fn synthesize(
        &self,
        results: Vec<SubagentResult>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
}
```

**重要**：返回的 Future 是 `'static`，不借用 `&self`。具体实现将 `outbound_tx`、`session_key`、`provider` 等 clone 到 struct 中。这允许 Aggregator task 被安全地 `tokio::spawn`。

### 3.3 ToolContext 扩展

```rust
pub struct ToolContext {
    // ...existing fields...
    /// Callback for triggering synthesis after all subagents complete.
    /// When None (CLI/Telegram/non-WebSocket mode), spawn tools use blocking mode.
    pub synthesis_callback: Option<Arc<dyn SynthesisCallback>>,
}
```

**非 WebSocket 回退**：当 `synthesis_callback` 为 `None` 时，spawn 工具检测到无回调，退回当前的阻塞等待模式。`SteppableExecutor` 仅在 WebSocket channel 模式下注入 callback。

### 3.4 spawn_parallel 核心流程变更

```
execute():
  1. 解析任务列表
  2. 调用 spawn_with_stream() 获取所有 (id, event_rx, result_rx)
  3. [同步] 通过 outbound_tx 发送 SubagentAllStarted { count }
  4. [同步] 通过 outbound_tx 发送所有 SubagentStarted { id, task, index }
  5. 启动 event forwarding tasks（与现有逻辑相同）
  6. 检查 synthesis_callback:
     - 若 Some(callback): 启动 Aggregator 后台 task，立即返回 Ok("已启动...")
     - 若 None: 执行现有阻塞等待逻辑（保持 CLI/Telegram 兼容）
  7. 返回
```

### 3.5 Aggregator 后台 task

Aggregator 是一个 `tokio::spawn` 的 task，通过 `CancellationToken` 支持取消。

```
Aggregator(callback, result_receivers, cancellation_token):
  1. tokio::select! 等待所有 result_rx 完成，带 15 分钟总超时
     - 超时或取消：将未完成的 subagent 标记为 error
  2. 通过 outbound_tx 发送 SubagentSynthesizing {}
  3. 调用 callback.synthesize(results)
     - callback 内部：
       a. 构建综合 prompt，注入 subagent 结果
       b. 通过 provider.chat_stream() 发起 LLM 调用
       c. 流式将 ChatEvent::Thinking/Content/Done 直接发送到 outbound_tx
          （绕过 kernel 的 StreamEvent 管道）
  4. 若 synthesize 失败：发送 ChatEvent::Error + ChatEvent::Done
```

**事件路由说明**：Aggregator 发送的事件是 `ChatEvent`（不是 `StreamEvent`），直接通过 `outbound_tx` → `OutboundMessage::with_ws_message()` 发送到 WebSocket。这完全绕过 kernel 的 StreamEvent→ChatEvent 转换管道，因为 Aggregator 在 kernel loop 之外运行。

### 3.6 非 WebSocket 通道回退

`SteppableExecutor` 构建 `ToolContext` 时，检查 `session_key` 的 channel type：
- WebSocket channel: 注入 `SynthesisCallback`
- 其他（CLI/Telegram/Discord 等）: `synthesis_callback = None`

spawn 工具在 `execute()` 中检查 `ctx.synthesis_callback`：
- `Some` → 非阻塞模式（两阶段）
- `None` → 阻塞模式（现有行为不变）

### 3.7 Aggregator 取消机制

用户发送新消息时（前端进入新 session turn），通过 `CancellationToken` 取消 Aggregator：
- 前端发送新消息后清理 `activeSubagents`
- 后端收到新消息时（bus/router 层），取消与当前 session 关联的 Aggregator token
- 已发起的 LLM 调用被中断，避免无意义的 token 消耗

## 4. 前端组件架构

### 4.1 新增组件：SubagentGridPanel.vue

多列网格展示组件，根据 phase 切换显示模式。

**Props**:

```ts
interface Props {
  subagents: SubagentState[]
  phase: 'running' | 'synthesizing' | 'completed'
}
```

**布局**:

- `running` phase: 多列网格，每列一个 subagent，各列独立滚动
  - Header: Task 编号 + 任务描述 + 状态徽标
  - Thinking: 实时推理内容
  - Tool Calls: 可折叠工具调用列表（名称 + 参数 + 输出）
  - Content: 实时输出内容
- `synthesizing` phase: 触发折叠动画，显示"正在综合结果..."过渡提示
- `completed` phase: 不渲染（由 SubagentThoughtsPanel 接替）

### 4.2 组件层级

```
ChatArea.vue
├── MessageBubble.vue
│   ├── MessageThoughtsPanel.vue   (主 agent thinking)
│   ├── SubagentGridPanel.vue      (running/synthesizing: 网格展示)  ← 新增
│   └── SubagentThoughtsPanel.vue  (completed: 折叠摘要)
└── SubagentPanel.vue              (移除)
```

SubagentGridPanel 从 MessageBubble 中通过 props 渲染，跟随消息流位置。

### 4.3 useChatSession.ts 状态扩展

```ts
const subagentPhase = ref<'idle' | 'running' | 'synthesizing' | 'completed'>('idle')
```

必须在 `return reactive({...})` 中导出 `subagentPhase`，供 `MessageBubble.vue` 传递给 `SubagentGridPanel`。

事件处理变更:

```ts
case 'subagent_all_started':
  subagentPhase.value = 'running'
  break

case 'done':
  // 如果有活跃 subagent，不结束接收状态
  if (activeSubagents.value.size > 0) break
  isThinking.value = false
  isReceiving.value = false
  fetchContext()
  break

case 'subagent_synthesizing':
  subagentPhase.value = 'synthesizing'
  // 短暂延迟后折叠网格
  setTimeout(() => { subagentPhase.value = 'completed' }, 300)
  break
```

**sendMessage 输入守卫调整**：

```ts
// 当前逻辑
if (isReceiving.value) return false

// 改为：subagent 运行期间允许发送
if (isReceiving.value && subagentPhase.value !== 'running') return false
```

用户在网格模式下发送新消息时：
1. 清理 `activeSubagents`
2. 重置 `subagentPhase` 为 `'idle'`
3. 创建新的 bot message 开始新对话轮

### 4.4 移除组件

`SubagentPanel.vue` 将被 `SubagentGridPanel` + `SubagentThoughtsPanel` 的组合替代。

## 5. 异常处理

### 5.1 Subagent 执行失败

单 subagent 出错不影响其他。网格中该列显示 error 状态，Aggregator 综合时标注 `[Error]`。

### 5.2 Aggregator LLM 调用失败

发送 `ChatEvent::Error` + `ChatEvent::Done`，前端正常结束。用户可手动追问。

### 5.3 Aggregator 总超时

Aggregator 使用 `tokio::time::timeout(15min, join_all(result_receivers))`。超时后将未完成的 subagent 标记为 error，继续综合已完成的 subagent 结果。已完成的 subagent 各自有 10 分钟超时（`SUBAGENT_TIMEOUT_SECS`）。

### 5.4 用户在网格模式中发送新消息

允许输入。新消息发送后：
- 前端清理 subagent 状态，重置 `subagentPhase` 为 `'idle'`
- 后端通过 `CancellationToken` 取消 Aggregator（见 3.7）
- Aggregator 已产生的 WebSocket 事件被前端忽略

### 5.5 WebSocket 断连

subagent 详细过程已持久化在 `message.subagents[]`（chatStore），重连后从 store 恢复显示。网格面板只负责实时展示。

### 5.6 spawn（单个）工具

复用相同模式：`SubagentAllStarted { count: 1 }` + `Done` + 后台等待 → 综合。共享逻辑抽取到 `spawn_common` 模块。

## 6. 实现优先级

1. `types`: 新增 ChatEvent variants + SynthesisCallback trait + ToolContext 字段
2. `engine/tools`: 抽取 `spawn_common` 模块（共享 event forwarding + Aggregator 逻辑）
3. `engine/tools`: 重构 spawn + spawn_parallel 使用 spawn_common
4. `engine/kernel`: 注入 SynthesisCallback 到 ToolContext
5. `engine`: 实现 SynthesisCallback（持有 provider + outbound_tx，执行综合 LLM 调用）
6. `web/types`: 更新 SubagentWsMessage 联合类型
7. `web`: 新增 SubagentGridPanel.vue 组件
8. `web`: useChatSession.ts 状态机扩展 + sendMessage 守卫调整
9. `web`: MessageBubble.vue 集成 GridPanel（props 传入 phase + subagents）
10. `web`: 移除 SubagentPanel.vue
