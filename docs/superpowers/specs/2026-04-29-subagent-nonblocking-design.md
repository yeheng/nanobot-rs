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

### 2.2 完整事件流时序

```
主 agent 调用 spawn_parallel
  ├─ ChatEvent::ToolStart { name: "spawn_parallel" }
  ├─ ChatEvent::SubagentAllStarted { count: N }
  ├─ ChatEvent::SubagentStarted { id, task, index } × N
  ├─ ChatEvent::ToolEnd { name: "spawn_parallel", output: "已启动 N 个并行任务" }
  ├─ ChatEvent::Done                                          ← 主 agent 本轮结束
  │
  │  ── 前端进入网格展示模式 ──
  │
  ├─ ChatEvent::SubagentThinking { id, content } × ...        ← 实时流式
  ├─ ChatEvent::SubagentToolStart { id, name, args } × ...
  ├─ ChatEvent::SubagentToolEnd { id, name, output } × ...
  ├─ ChatEvent::SubagentContent { id, content } × ...
  ├─ ChatEvent::SubagentCompleted { id, index, summary } × N
  │
  ├─ ChatEvent::SubagentSynthesizing {}                       ← 前端折叠 + 过渡提示
  ├─ ChatEvent::Thinking { content: "..." }                   ← 主 agent 综合思考
  ├─ ChatEvent::Content { content: "最终回复..." }            ← 主 agent 流式输出
  ├─ ChatEvent::Done                                          ← 真正结束
```

## 3. 后端执行模型

### 3.1 改动文件

| 文件 | 变更 |
|------|------|
| `gasket/types/src/events/stream.rs` | 新增 `SubagentAllStarted`, `SubagentSynthesizing` |
| `gasket/types/src/tool.rs` | 新增 `SynthesisCallback` trait 和 `ToolContext` 字段 |
| `gasket/engine/src/tools/spawn.rs` | 重构为非阻塞模式 |
| `gasket/engine/src/tools/spawn_parallel.rs` | 重构为非阻塞模式 |
| `gasket/engine/src/kernel/steppable_executor.rs` | 构建 ToolContext 时注入 SynthesisCallback |

### 3.2 SynthesisCallback trait

```rust
/// types/src/tool.rs
pub trait SynthesisCallback: Send + Sync {
    fn synthesize(
        &self,
        results: Vec<SubagentResult>,
        session_key: SessionKey,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}
```

### 3.3 ToolContext 扩展

```rust
pub struct ToolContext {
    // ...existing fields...
    /// Callback for triggering synthesis after all subagents complete.
    pub synthesis_callback: Option<Arc<dyn SynthesisCallback>>,
}
```

### 3.4 spawn_parallel 核心流程变更

```
execute():
  1. 解析任务列表
  2. spawn 所有 subagent，获取 (id, event_rx, result_rx) 列表
  3. 启动 event forwarding tasks（与现有逻辑相同）
  4. 发送 SubagentAllStarted { count } 事件
  5. 发送 Done 事件
  6. 启动 Aggregator 后台 task（等待结果 → 综合）
  7. 立即返回 Ok("已启动 N 个并行任务，执行中...")
```

### 3.5 Aggregator 后台 task

```
Aggregator:
  1. 等待所有 result_rx 完成 (join_all)
  2. 发送 SubagentSynthesizing {} 事件
  3. 构建综合 prompt，注入 subagent 结果
  4. 通过 provider.chat_stream() 发起 LLM 调用
  5. 流式转发 Thinking/Content/Done 事件到 WebSocket
```

### 3.6 非 WebSocket 通道回退

SynthesisCallback 实现检查 session_key 是否有活跃的 WebSocket 连接。若非 WebSocket 模式（CLI/Telegram），spawn 工具退回阻塞模式，保持现有行为不变。

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

事件处理变更:

- `subagent_all_started`: 设置 `subagentPhase = 'running'`
- `done`: 若 `activeSubagents.size > 0`，不结束接收状态
- `subagent_synthesizing`: 设置 `subagentPhase = 'synthesizing'`，延迟后切换为 `'completed'`
- `subagent_completed`: 现有逻辑不变，更新 store 中的 subagent 状态

### 4.4 移除组件

`SubagentPanel.vue` 将被 `SubagentGridPanel` + `SubagentThoughtsPanel` 的组合替代。

## 5. 异常处理

### 5.1 Subagent 执行失败

单 subagent 出错不影响其他。网格中该列显示 error 状态，Aggregator 综合时标注 `[Error]`。

### 5.2 Aggregator LLM 调用失败

发送 `ChatEvent::Error` + `ChatEvent::Done`，前端正常结束。用户可手动追问。

### 5.3 用户在网格模式中发送新消息

允许输入。新消息发送后前端清理 subagent 状态，Aggregator 的后续结果被前端忽略。

### 5.4 WebSocket 断连

subagent 详细过程已持久化在 `message.subagents[]`（chatStore），重连后从 store 恢复显示。网格面板只负责实时展示。

### 5.5 spawn（单个）工具

复用相同模式：`SubagentAllStarted { count: 1 }` + `Done` + 后台等待 → 综合。共享逻辑抽取到 `spawn_common` 模块。

## 6. 实现优先级

1. `types`: 新增 ChatEvent variants + SynthesisCallback trait
2. `engine/tools`: 重构 spawn + spawn_parallel + 新增 spawn_common
3. `engine/kernel`: 注入 SynthesisCallback 到 ToolContext
4. `engine`: 实现 SynthesisCallback（持有 provider，执行综合 LLM 调用）
5. `web`: SubagentGridPanel.vue 组件
6. `web`: useChatSession.ts 状态机扩展
7. `web`: MessageBubble.vue 集成 GridPanel
8. `web`: 移除 SubagentPanel.vue
