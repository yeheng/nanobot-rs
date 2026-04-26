# AgentContext Refactor: Eliminate God Class

**Date:** 2026-04-26
**Status:** Draft
**Scope:** `gasket/engine/src/session/`, `gasket/engine/src/hooks/`

## Problem

`AgentContext` is an enum (`Persistent` | `Stateless`) that couples data ownership with business logic:
- Holds `EventStore` + `SessionStore` references in `PersistentContext`
- Implements methods like `save_event()`, `load_summary_with_watermark()`, `get_events_after_watermark()`
- Forces `ContextBuilder` and `ResponseFinalizer` to depend on it for storage I/O
- `Stateless` variant exists only in tests — the abstraction is wrong
- Violates SRP: a "context" should carry state, not perform I/O

## Decision

Eliminate `AgentContext` entirely. Storage I/O moves into self-contained `PipelineHook` implementations that own their `Arc<EventStore>` / `Arc<SessionStore>` dependencies. `AgentSession` no longer holds any context struct.

## Design

### 1. Delete AgentContext

Remove from `session/context.rs`:
- `enum AgentContext` (`Persistent` | `Stateless`)
- `struct PersistentContext`
- All methods: `save_event`, `load_summary_with_watermark`, `get_events_after_watermark`, `load_session`, `clear_session`, `is_persistent`, `persistent()`
- The entire `context.rs` file

Remove re-exports from `session/mod.rs` and `lib.rs`.

### 2. New Hooks

#### 2.1 `HistoryLoadHook` (`hooks/history.rs`, new file)

Runs at `HookPoint::AfterHistory`. Replaces steps 2-4 in current `ContextBuilder::build()`:

- Load summary + watermark from `SessionStore`
- Save user event to `EventStore`
- Load events after watermark, trim by token budget
- Inject history into `ctx.messages`

Owns: `Arc<EventStore>`, `Arc<SessionStore>`, `HistoryConfig`

```rust
pub struct HistoryLoadHook {
    event_store: Arc<EventStore>,
    session_store: Arc<SessionStore>,
    config: HistoryConfig,
}

impl PipelineHook for HistoryLoadHook {
    fn name(&self) -> &str { "history_loader" }
    fn point(&self) -> HookPoint { HookPoint::AfterHistory }
}
```

#### 2.2 `PersistHook` (`hooks/persist.rs`, new file)

Runs at `HookPoint::AfterResponse` (parallel strategy). Replaces `save_assistant_event()` in `ResponseFinalizer`:

- Construct `SessionEvent` from response content
- Save to `EventStore`

Owns: `Arc<EventStore>`

```rust
pub struct PersistHook {
    event_store: Arc<EventStore>,
}

impl PipelineHook for PersistHook {
    fn name(&self) -> &str { "persist" }
    fn point(&self) -> HookPoint { HookPoint::AfterResponse }
}
```

### 3. Checkpoint Mechanism

`SessionCheckpointCallback` remains a kernel callback (not a `PipelineHook`) — it runs inside the kernel execution loop, not at a pipeline stage.

Changes:
- `AgentSession` gains `event_store: Option<Arc<EventStore>>` field, used solely for constructing the callback
- `preprocess()` constructs `SessionCheckpointCallback` from `self.event_store` directly — no `AgentContext` destructure

```rust
if let (Some(ref compactor), Some(ref store)) = (&self.compactor, &self.event_store) {
    runtime_ctx.checkpoint_callback = Arc::new(SessionCheckpointCallback {
        session_key: fctx.session_key.clone(),
        compactor: compactor.clone(),
        event_store: store.clone(),
    });
}
```

### 4. ContextBuilder Simplification

`session/history/builder.rs`:

- Remove `context: AgentContext` field
- `build()` no longer calls `self.context.*` methods
- Retains: BeforeRequest hook execution, system prompt assembly, AfterHistory + BeforeLLM hook dispatch
- HistoryLoadHook handles all storage I/O during AfterHistory

```rust
pub struct ContextBuilder {
    system_prompt: String,
    skills_context: Option<String>,
    hooks: Arc<HookRegistry>,
    history_config: HistoryConfig,
}
```

### 5. ResponseFinalizer Simplification

`session/finalizer.rs`:

- Remove `AgentContext` parameter from `finalize()`
- Remove `save_assistant_event()` function
- Remove `trigger_compaction()` — compaction trigger moves to an AfterResponse hook or stays as direct call
- `finalize()` becomes: execute AfterResponse hooks (including PersistHook), calculate cost, build response

### 6. PipelineContext Simplification

Remove `context: AgentContext` field from `PipelineContext`.

```rust
struct PipelineContext {
    runtime_ctx: RuntimeContext,
    messages: Vec<ChatMessage>,
    fctx: FinalizeContext,
    model: String,
    pricing: Option<ModelPricing>,
    finalizer: ResponseFinalizer,
}
```

### 7. AgentSession Changes

- Remove `context: AgentContext` field
- Add `event_store: Option<Arc<EventStore>>` for checkpoint callback
- Remove `wiki_components: Option<WikiComponents>` — tools hold their own references
- Remove `WikiComponents` struct and `WikiHealth` enum
- Remove methods: `page_store()`, `page_index()`, `wiki_log()`, `wiki_health()`
- Remove `clear_session()` (move to `AgentSession` directly using `self.event_store`)

### 8. SessionBuilder Changes

`session/builder.rs`:

- Construct `HistoryLoadHook` with `Arc<EventStore>` + `Arc<SessionStore>`
- Construct `PersistHook` with `Arc<EventStore>`
- Add both to `HookBuilder` alongside existing `ExternalShellHook` + `VaultHook`
- Wiki tools capture `Arc<PageStore>`/`Arc<PageIndex>`/`Arc<WikiLog>` at registration time
- `build_wiki_components()` returns components only for tool registration, not stored on session

### 9. File Changes Summary

| File | Action |
|------|--------|
| `session/context.rs` | Delete entire file |
| `session/mod.rs` | Remove context re-exports, remove WikiComponents/WikiHealth, update struct fields |
| `session/builder.rs` | Construct hooks with store refs, remove WikiComponents from AgentSession |
| `session/history/builder.rs` | Remove `context` field, simplify `build()` |
| `session/finalizer.rs` | Remove `AgentContext` param, remove save/compact logic |
| `hooks/history.rs` | New — HistoryLoadHook |
| `hooks/persist.rs` | New — PersistHook |
| `hooks/mod.rs` | Add new module re-exports |
| `lib.rs` | Remove AgentContext/PersistentContext re-exports |

### 10. Compaction Trigger

`trigger_compaction()` currently lives in `ResponseFinalizer`. After refactoring, it moves to an AfterResponse hook or stays as a direct method on `AgentSession` using `self.compactor`. The compactor already owns `EventStore`/`SessionStore` references internally, so no additional wiring needed.

Prefer: keep `trigger_compaction()` as a direct call inside `postprocess()`, since compaction is a core infrastructure concern (not a pluggable behavior) and the compactor is already on `AgentSession`.

## Acceptance Criteria

1. `AgentContext` enum and `PersistentContext` struct no longer exist
2. No file imports `AgentContext` or `PersistentContext`
3. `AgentSession` has no `context` field
4. Unit tests for `AgentSession` can run without a database (by not registering HistoryLoadHook/PersistHook)
5. All existing integration tests pass
6. `HistoryLoadHook` and `PersistHook` have dedicated unit tests
