## Context

nanobot 的 Gateway 模式使用 `Arc<AgentLoop>` 共享一个 agent 实例给所有并发请求。当前没有 per-session 的串行化机制，同一用户的两条消息可能同时进入 `process_direct`，导致 `PersistenceHook` 中的 `active_sessions` HashMap 出现 race condition。

`AgentHook` trait 设计上使用 `&self`（不可变引用），暗示 Hook 应该是无状态的，但 `PersistenceHook` 通过 `Arc<Mutex<HashMap>>` 引入了共享可变状态，违反了这一设计意图。

## Goals / Non-Goals

**Goals:**
- 确保同一 session 的请求严格串行执行
- 消除 Hook 中的共享可变状态（`active_sessions`）
- 为每个请求赋予唯一 `request_id`，贯穿整个管线
- 修复 metadata 从 `on_response` 到最终 `on_session_save` 的传播断裂
- 明确文档化 Stateless Hook 合约

**Non-Goals:**
- 不改变跨 session 的并发模型（不同 session 仍然完全并发）
- 不引入请求取消（CancellationToken）机制（留给未来提案）
- 不改变 Subagent 的隔离模型（已经是独立 AgentLoop）
- 不引入分布式锁或 Redis（SQLite WAL 已足够）

## Decisions

### Decision 1: Per-Session Semaphore Map

**方案**: 在 Gateway inbound handler 中维护 `DashMap<String, Arc<Semaphore>>` 或 `Mutex<HashMap<String, Arc<Semaphore>>>`，每个 session_key 对应一个 `Semaphore(1)`。

**Why**: 比全局 Mutex 更细粒度 — 不同 session 完全不阻塞。Semaphore(1) 等价于互斥锁但与 tokio async 更兼容。

**Alternatives considered:**
- 全局 `tokio::sync::Mutex`: 太粗粒度，所有 session 串行化
- mpsc channel per session: 复杂度高，需要管理 channel 生命周期
- Actor model (per-session actor): 过度设计，引入大量样板代码

### Decision 2: 移除 `active_sessions` In-Memory Cache

**方案**: `PersistenceHook` 不再维护 `active_sessions: Arc<Mutex<HashMap<String, Session>>>`。改为：
- `on_session_load`: 直接从 `SessionManager` 获取会话，不缓存
- `on_session_save`: 直接通过 `SessionManager::append_message()` 写入 SQLite

这与 `SessionManager` 的设计意图一致（其文档明确写道 "No in-memory cache: SQLite is the single source of truth"）。

**Why**: Per-session 串行化保证同一 session 不会并发访问，因此不再需要 in-memory cache 来桥接 load/save 之间的状态。

### Decision 3: `request_id` in Context Structs

**方案**: 在 `process_direct_with_callback` 入口生成 `Uuid::new_v4().to_string()`，将其放入所有 `*Context.metadata` 中的 `"_request_id"` key。同时为每个 Context struct 添加一个 `request_id: String` 顶级字段以便直接访问。

**Why**: 使用 metadata 的 convention key 而非 struct field 会导致 Hook 依赖字符串常量；顶级字段更明确且编译器可检查。

### Decision 4: Metadata 传播修复

**方案**: 在 `process_direct_with_callback` 中，将 `run_agent_loop` 返回的 `hook_metadata` 传入 `ResponseContext.metadata`，再将 `resp_ctx.metadata` 传入最终的 `SessionSaveContext.metadata`。

当前代码在 line 286 创建 `ResponseContext` 时使用 `metadata: HashMap::new()` 而非继承前序 metadata — 这是 bug。

## Risks / Trade-offs

- **Semaphore Map 内存增长**: session key 越多，Semaphore 越多。缓解：定期清理不活跃的 session key（TTL 机制）或使用 weak reference。初期可接受 — session 数量级通常在百级别。
- **PersistenceHook 移除缓存后的性能**: 每次 `on_session_load` 都要查 SQLite。缓解：SQLite WAL 模式下读操作非常快（微秒级）；Per-session 串行化保证同一 session 不会并发读写。
- **Breaking Change**: 外部代码若依赖 `PersistenceHook::active_sessions`（极不可能，因为该字段是私有的）。

## Migration Plan

1. 添加 `request_id` 到所有 Context struct — 纯 additive，不影响现有 Hook
2. 修复 metadata 传播 — bug fix，无 breaking change
3. 在 Gateway 中添加 per-session semaphore — 对 Hook 透明
4. 重构 `PersistenceHook` 移除 `active_sessions` — 内部实现变更，API 不变
5. 更新 `AgentHook` trait 文档 — 纯文档

所有步骤可以在一个 PR 中完成，也可以拆分为 2 个 PR（additive changes 先合，breaking change 后合）。

## Open Questions

- 是否需要为 Semaphore Map 添加 TTL 清理机制？（建议：初期不需要，观察内存后再决定）
- `request_id` 是否需要暴露给 LLM Provider（作为 request header）？（建议：暂不需要，但 metadata 机制支持未来扩展）
