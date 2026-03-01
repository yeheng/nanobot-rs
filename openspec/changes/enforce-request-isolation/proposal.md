# Change: Enforce Request Context Isolation and Stateless Hooks

## Why

当前 Gateway 模式下，每条入站消息会 `tokio::spawn` 一个独立任务，但**同一 session 的多条消息可能并发执行**，导致：

1. **`PersistenceHook.active_sessions`** 是一个全局 `Arc<Mutex<HashMap>>` — 同 session 的两条并发请求会互相覆盖会话快照，丢失消息。
2. **无 `request_id`** — Hook 在并发场景下无法区分"自己属于哪个请求"。
3. **`metadata` 传播中断** — `on_response` 阶段的 metadata 未流入后续 `on_session_save`，导致跨阶段通信断裂。
4. **Hook 无状态合约未强制** — `AgentHook` trait 虽然使用 `&self`，但 `PersistenceHook` 通过内部可变性引入了共享可变状态，违反了无状态设计意图。

本提案确立三条原则：

- **请求上下文绝对隔离**：同一 session 的请求串行执行，跨 session 完全并发。
- **单线执行**：一个请求从 `on_request` 到 `on_response` 的整个管线内顺序执行，无内部并发。
- **Hook 必须无状态（Stateless）**：Hook 不得持有跨请求的可变状态；所有上下文通过 Context 结构体传入。

## What Changes

- 引入 **per-session 串行化**：Gateway inbound handler 使用 `session_key → Semaphore(1)` 确保同 session 请求排队执行
- 移除 `PersistenceHook.active_sessions` 缓存，改为每次请求通过 SQLite 直接读写
- 为所有 `*Context` 结构体添加 `request_id: String` 字段（UUID v4）
- 修复 `on_response` → `on_session_save` 的 metadata 传播断裂
- 在 `AgentHook` trait 文档中明确 **Stateless Contract**
- **BREAKING**: `PersistenceHook` 不再暴露 `active_sessions`；第三方 Hook 若依赖此字段需迁移

## Impact

- Affected specs: (无现有 spec，全部为 ADDED)
- Affected code:
  - `nanobot-core/src/hooks/mod.rs` — Context 结构体 + trait 文档
  - `nanobot-core/src/hooks/persistence.rs` — 移除 `active_sessions`
  - `nanobot-core/src/agent/loop_.rs` — 生成 `request_id` 并传播 metadata
  - `nanobot-cli/src/commands/gateway.rs` — per-session 串行化
