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

Eliminate `AgentContext` entirely. Storage references move to the components that actually need them:
- **Preprocess (history loading)**: `ContextBuilder` holds `Arc<EventStore>` + `Arc<SessionStore>` directly
- **Postprocess (event persistence)**: `ResponseFinalizer` holds `Arc<EventStore>` directly
- **Checkpoint**: `AgentSession` holds `Arc<EventStore>` directly for callback construction
- **Wiki**: Tools capture their own storage references at registration

Core infrastructure (history loading, event persistence, compaction) stays as direct method calls — not hooks — because ordering is critical and these behaviors are not pluggable.

## Design

### 1. Delete AgentContext

Remove from `session/context.rs`:
- `enum AgentContext` (`Persistent` | `Stateless`)
- `struct PersistentContext`
- All methods: `save_event`, `load_summary_with_watermark`, `get_events_after_watermark`, `load_session`, `clear_session`, `is_persistent`, `persistent()`
- The entire `context.rs` file

Remove re-exports from `session/mod.rs` and `lib.rs`.

### 2. ContextBuilder: Direct Store References

`ContextBuilder` replaces `context: AgentContext` with direct store fields. History loading stays inline in `build()` — not as a hook — because it must complete before `AfterHistory` hooks execute. Moving it to a hook would create ordering dependencies with other `AfterHistory` hooks.

```rust
pub struct ContextBuilder {
    event_store: Arc<EventStore>,
    session_store: Arc<SessionStore>,
    system_prompt: String,
    skills_context: Option<String>,
    hooks: Arc<HookRegistry>,
    history_config: HistoryConfig,
}
```

`build()` now calls `self.event_store.*` and `self.session_store.*` directly instead of `self.context.*`:
- `self.session_store.load_summary_with_checkpoint(session_key)` replaces `self.context.load_summary_with_watermark(session_key)` (logic moved to `SessionStore` as a new method)
- `self.event_store.append_event(&user_event)` replaces `self.context.save_event(user_event)`
- `self.event_store.get_events_after_sequence(...)` replaces `self.context.get_events_after_watermark(...)`

The checkpoint loading logic (merging latest checkpoint into summary) moves from `AgentContext::load_summary_with_watermark()` to `SessionStore::load_summary_with_checkpoint()` — it's session reconstruction logic, not builder logic.

### 3. ResponseFinalizer: Direct Store Reference

`ResponseFinalizer` replaces `context: &AgentContext` with `event_store: Arc<EventStore>`:

```rust
pub struct ResponseFinalizer {
    hooks: Arc<HookRegistry>,
    event_store: Arc<EventStore>,
    compactor: Option<Arc<ContextCompactor>>,
    pricing: Option<ModelPricing>,
    max_tokens: u32,
}
```

`finalize()` signature changes:

```rust
// Before:
pub(crate) async fn finalize(&self, result: ExecutionResult, ctx: &FinalizeContext, context: &AgentContext, model: &str) -> AgentResponse

// After:
pub(crate) async fn finalize(&self, result: ExecutionResult, ctx: &FinalizeContext, model: &str) -> AgentResponse
```

`save_assistant_event()` and `trigger_compaction()` remain as direct calls inside `finalize()` — event persistence and compaction are core infrastructure, not pluggable behaviors. They use `self.event_store` and `self.compactor` directly.

### 4. Checkpoint Mechanism

`SessionCheckpointCallback` remains a kernel callback (not a `PipelineHook`) — it runs inside the kernel execution loop, not at a pipeline stage.

Changes:
- `AgentSession` gains `event_store: Arc<EventStore>` field (non-optional — AgentSession IS persistent)
- `preprocess()` constructs `SessionCheckpointCallback` from `self.event_store` directly — no `AgentContext` destructure

```rust
if let Some(ref compactor) = &self.compactor {
    runtime_ctx.checkpoint_callback = Arc::new(SessionCheckpointCallback {
        session_key: fctx.session_key.clone(),
        compactor: compactor.clone(),
        event_store: self.event_store.clone(),
    });
}
```

### 5. PipelineContext Simplification

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

`postprocess()` signature no longer needs `&PipelineContext.context`:

```rust
async fn postprocess(result: ExecutionResult, ctx: &PipelineContext) -> AgentResponse {
    ctx.finalizer.finalize(result, &ctx.fctx, &ctx.model).await
}
```

### 6. AgentSession Changes

- Remove `context: AgentContext` field
- Add `event_store: Arc<EventStore>` for checkpoint callback (non-optional)
- Add `session_store: Arc<SessionStore>` for ContextBuilder construction (non-optional)
- Remove `wiki_components: Option<WikiComponents>` — tools hold their own references
- Remove `WikiComponents` struct and `WikiHealth` enum
- Remove methods: `page_store()`, `page_index()`, `wiki_log()`, `wiki_health()`
- `clear_session()` rewrites to use `self.event_store` directly:

```rust
pub async fn clear_session(&self, session_key: &SessionKey) {
    match self.event_store.clear_session(session_key).await {
        Ok(_) => info!("Session '{}' cleared", session_key),
        Err(e) => warn!("Failed to clear session '{}': {}", session_key, e),
    }
}
```

### 7. SessionBuilder Changes

`session/builder.rs`:

- Construct `EventStore` / `SessionStore` as before
- Pass `event_store` / `session_store` to `ContextBuilder` directly
- Pass `event_store` to `ResponseFinalizer`
- Pass `event_store` to `AgentSession` (for checkpoint + clear_session)
- Wiki tools capture `Arc<PageStore>`/`Arc<PageIndex>`/`Arc<WikiLog>` at registration time
- `build_wiki_components()` replaced by `is_wiki_available()` — lightweight config+path probe, no full component construction

```rust
let event_store = Arc::new(EventStore::new(pool));
let session_store = Arc::new(SessionStore::new(pool));

// ContextBuilder gets direct store refs
let context_builder = ContextBuilder::new(
    event_store.clone(),
    session_store.clone(),
    system_prompt,
    skills_context,
    hooks.clone(),
    history_config,
);

// ResponseFinalizer gets direct event_store ref
let finalizer = ResponseFinalizer::new(
    hooks.clone(),
    event_store.clone(),
    compactor.clone(),
    pricing,
    config.max_tokens,
);

Ok(AgentSession {
    event_store,
    session_store,
    compactor,
    hooks,
    finalizer,
    ...
})
```

### 8. Test Migration

Existing tests in `session/context.rs`:

| Test | Action |
|------|--------|
| `test_stateless_context_is_not_persistent` | Delete — `Stateless` variant removed |
| `test_stateless_load_session` | Delete — `Stateless` variant removed |
| `test_stateless_save_event` | Delete — `Stateless` variant removed |
| `test_stateless_context_clear_session` | Delete — `Stateless` variant removed |
| `test_persistent_context_creation` | Migrate to `history/builder.rs` tests — verifies ContextBuilder store setup |
| `test_persistent_context_save_event` | Migrate to `finalizer.rs` tests — verifies ResponseFinalizer event persistence |

New tests needed:
- `history/builder.rs`: Test `ContextBuilder::build()` with in-memory SQLite (integration test)
- `finalizer.rs`: Test `ResponseFinalizer::finalize()` saves assistant event correctly
- `session/mod.rs`: Test `clear_session()` works correctly (stores always present)
- `gasket/storage/src/session.rs`: Test `load_summary_with_checkpoint()` (unit test for new SessionStore method)

### 9. File Changes Summary

| File | Action |
|------|--------|
| `session/context.rs` | Delete entire file |
| `session/mod.rs` | Remove context re-exports, remove WikiComponents/WikiHealth, add non-optional `event_store`/`session_store` fields, rewrite `clear_session()` |
| `session/builder.rs` | Pass store refs to ContextBuilder and ResponseFinalizer, replace `build_wiki_components()` with `is_wiki_available()` |
| `session/history/builder.rs` | Replace `context: AgentContext` with `event_store` + `session_store` fields, use `SessionStore::load_summary_with_checkpoint()` |
| `session/finalizer.rs` | Replace `AgentContext` param with `Arc<EventStore>` field, keep save/compact as direct calls |
| `storage/src/session.rs` | Add `load_summary_with_checkpoint()` method to `SessionStore` |
| `hooks/mod.rs` | No changes (no new hooks) |
| `lib.rs` | Remove `AgentContext`/`PersistentContext`/`WikiHealth` re-exports |

### 10. Documentation Updates

The following doc files reference `AgentContext` and need updating:

| File | Change |
|------|--------|
| `docs/architecture.md` | Update session layer diagram |
| `docs/architecture-en.md` | Update session layer diagram |
| `docs/modules.md` | Remove AgentContext from module descriptions |
| `docs/modules-en.md` | Remove AgentContext from module descriptions |
| `docs/session-en.md` | Update session management docs |
| `docs/data-structures.md` | Remove AgentContext from data structure diagrams |
| `docs/data-structures-en.md` | Remove AgentContext from data structure diagrams |
| `docs/technical-design.md` | Update technical design references |
| `docs/core-modules-design.md` | Update core module design |

## Acceptance Criteria

1. `AgentContext` enum and `PersistentContext` struct no longer exist
2. No file imports `AgentContext` or `PersistentContext`
3. `AgentSession` has no `context` field
4. `ContextBuilder` holds `Arc<EventStore>` + `Arc<SessionStore>` directly
5. `ResponseFinalizer` holds `Arc<EventStore>` directly
6. `AgentSession` holds non-optional `Arc<EventStore>` + `Arc<SessionStore>` — no `Option` wrappers
7. `clear_session()` on `AgentSession` works via `self.event_store` (no `if let Some` guard)
8. `SessionStore` has `load_summary_with_checkpoint()` method for summary+checkpoint merging
9. `build_wiki_components()` replaced by lightweight `is_wiki_available()` config probe
10. All existing integration tests pass
11. Migrated tests (from `context.rs`) pass in their new locations
12. All tests use in-memory SQLite (no `Option`-based "no database" pattern)
