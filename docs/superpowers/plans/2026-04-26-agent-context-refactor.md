# AgentContext Elimination — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the `AgentContext` enum by moving storage references to the components that use them directly.

**Architecture:** `ContextBuilder` and `ResponseFinalizer` hold `Arc<EventStore>`/`Arc<SessionStore>` directly. `AgentSession` holds `Arc<EventStore>`/`Arc<SessionStore>` directly (non-optional — AgentSession IS persistent, period). Wiki tool providers already hold their own refs — `WikiComponents` on `AgentSession` is unused and removed.

**Tech Stack:** Rust, tokio, sqlx, Arc-based dependency sharing

**Spec:** `docs/superpowers/specs/2026-04-26-agent-context-refactor-design.md`

**Critical Rule — Commit Strategy:**
Each commit MUST compile independently. No exceptions. Tasks 1-4 produce intermediate broken commits in the original plan — that breaks `git bisect` and is unacceptable. There are two valid approaches:
1. **Single atomic commit**: Make all file changes in one commit (Tasks 1-6), then docs/CLAUDE.md in separate commits.
2. **Per-file independent commits**: Restructure so each commit only touches files that compile cleanly together.

Either approach is fine. This plan uses **approach 1** — Tasks 1-6 are checkpointed as build-and-fix cycles within a local branch, then committed as a single atomic change.

---

## File Structure

| File | Responsibility |
|------|---------------|
| `gasket/engine/src/session/context.rs` | **DELETE** — remove AgentContext/PersistentContext |
| `gasket/engine/src/session/history/builder.rs` | ContextBuilder — replace `context` with direct store refs, use `SessionStore::load_summary_with_checkpoint()` |
| `gasket/engine/src/session/finalizer.rs` | ResponseFinalizer — replace `AgentContext` param with `Arc<EventStore>` field |
| `gasket/engine/src/session/mod.rs` | AgentSession — remove `context` field, add non-optional `event_store`/`session_store`, remove WikiComponents |
| `gasket/engine/src/session/builder.rs` | SessionBuilder — rewire store refs to consumers, replace `build_wiki_components()` with `is_wiki_available()` |
| `gasket/storage/src/session.rs` | **ADD** `load_summary_with_checkpoint()` method to `SessionStore` |
| `gasket/engine/src/lib.rs` | Remove `AgentContext`/`PersistentContext`/`WikiHealth` re-exports |

---

### Task 1: Refactor ContextBuilder — replace `AgentContext` with direct store refs

**Files:**
- Modify: `gasket/engine/src/session/history/builder.rs`

**Context:** `ContextBuilder` currently holds `context: AgentContext` and calls `self.context.load_summary_with_watermark()`, `self.context.save_event()`, `self.context.get_events_after_watermark()`. Replace with direct `Arc<EventStore>` + `Arc<SessionStore>` fields.

The checkpoint-loading logic (merging latest checkpoint into summary) currently lives in `AgentContext::load_summary_with_watermark()` at `context.rs:100-136`. It needs to be inlined as a private helper.

- [ ] **Step 1: Replace struct fields**

In `ContextBuilder` struct (line 54-60), replace `context: AgentContext` with `event_store` + `session_store`:

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

- [ ] **Step 2: Update `new()` constructor**

Replace the `context` parameter with `event_store` + `session_store`:

```rust
pub fn new(
    event_store: Arc<EventStore>,
    session_store: Arc<SessionStore>,
    system_prompt: String,
    skills_context: Option<String>,
    hooks: Arc<HookRegistry>,
    history_config: HistoryConfig,
) -> Self {
    Self {
        event_store,
        session_store,
        system_prompt,
        skills_context,
        hooks,
        history_config,
    }
}
```

- [ ] **Step 3: Add `load_summary_with_checkpoint()` to SessionStore (gasket-storage crate)**

This is session reconstruction logic (merge summary + checkpoint), not builder logic. It belongs on `SessionStore`.

In `gasket/storage/src/session.rs`, add:

```rust
/// Load summary with watermark and merge latest checkpoint.
///
/// Returns `(merged_summary, watermark)`.
/// If no summary exists, returns `("", 0)`.
pub async fn load_summary_with_checkpoint(
    &self,
    session_key: &SessionKey,
) -> Result<(String, i64), StoreError> {
    let (mut summary, watermark) = match self.load_summary(session_key).await {
        Ok(Some((content, watermark))) => (content, watermark),
        Ok(None) => (String::new(), 0),
        Err(e) => return Err(e),
    };

    let key_str = session_key.to_string();
    if let Ok(Some((ck_summary, _ck_seq))) =
        self.load_checkpoint(&key_str, i64::MAX).await
    {
        if !ck_summary.is_empty() {
            if !summary.is_empty() {
                summary.push_str("\n\n[Working Memory]\n");
            }
            summary.push_str(&ck_summary);
        }
    }

    Ok((summary, watermark))
}
```

- [ ] **Step 4: Rewrite `build()` method to use direct store calls**

Replace the three `self.context.*` calls in `build()`:

Step 2 (line 127-131) — load summary:
```rust
let (existing_summary, watermark) =
    self.session_store.load_summary_with_checkpoint(session_key).await
    .map_err(|e| AgentError::SessionError(
        format!("Failed to load summary for {}: {}", session_key, e)
    ))?;
```

Step 3 (line 133-143) — save user event:
```rust
let user_event = SessionEvent {
    id: uuid::Uuid::now_v7(),
    session_key: session_key_str.clone(),
    event_type: gasket_types::EventType::UserMessage,
    content: content.clone(),
    metadata: gasket_types::EventMetadata::default(),
    created_at: chrono::Utc::now(),
    sequence: 0,
};
self.event_store
    .append_event(&user_event)
    .await
    .map_err(|e| AgentError::SessionError(format!("Failed to persist user event: {}", e)))?;
```

Step 4 (line 146-149) — load history:
```rust
let history_events = if watermark == 0 {
    self.event_store.get_session_history(session_key).await
} else {
    self.event_store.get_events_after_sequence(session_key, watermark).await
}.map_err(|e| AgentError::SessionError(format!(
    "Failed to load history for '{}': {}", session_key, e
)))?;
```

- [ ] **Step 5: Update imports**

Remove: `use crate::session::context::AgentContext;`
Add: `use gasket_storage::EventStore;` (if not already imported)

- [ ] **Step 6: Verify build compiles**

Run: `cargo build --package gasket-engine`
Expected: Compile errors in `session/mod.rs`, `session/builder.rs`, `session/finalizer.rs` (expected — we fix those next). ContextBuilder itself should compile cleanly.

- [ ] **Step 7: Local checkpoint (squashed into final atomic commit)**

```bash
# Local checkpoint — will be squashed. Do not push.
git add gasket/engine/src/session/history/builder.rs
git commit -m "checkpoint: ContextBuilder store refs"
```

---

### Task 2: Refactor ResponseFinalizer — replace `AgentContext` with `Arc<EventStore>`

**Files:**
- Modify: `gasket/engine/src/session/finalizer.rs`

**Context:** `ResponseFinalizer` currently receives `&AgentContext` in `finalize()` and uses it only for `save_event()`. Replace with an owned `Arc<EventStore>` field.

- [ ] **Step 1: Update struct fields**

```rust
pub struct ResponseFinalizer {
    hooks: Arc<HookRegistry>,
    event_store: Arc<EventStore>,
    compactor: Option<Arc<ContextCompactor>>,
    pricing: Option<ModelPricing>,
    max_tokens: u32,
}
```

- [ ] **Step 2: Update `new()` constructor**

```rust
pub fn new(
    hooks: Arc<HookRegistry>,
    event_store: Arc<EventStore>,
    compactor: Option<Arc<ContextCompactor>>,
    pricing: Option<ModelPricing>,
    max_tokens: u32,
) -> Self {
    Self {
        hooks,
        event_store,
        compactor,
        pricing,
        max_tokens,
    }
}
```

- [ ] **Step 3: Simplify `finalize()` signature**

Remove `context: &AgentContext` parameter:

```rust
pub(crate) async fn finalize(
    &self,
    result: ExecutionResult,
    ctx: &FinalizeContext,
    model: &str,
) -> AgentResponse {
    let vault_values = &ctx.local_vault_values;

    save_assistant_event(&self.event_store, &result, ctx, vault_values).await;
    trigger_compaction(self.compactor.as_ref(), ctx, vault_values);
    execute_after_response_hooks(&self.hooks, &result, ctx).await;

    let cost = calculate_cost(&result.token_usage, self.pricing.as_ref());
    log_token_stats(&result.token_usage, cost, self.max_tokens);

    AgentResponse {
        content: result.content,
        reasoning_content: result.reasoning_content,
        tools_used: result.tools_used,
        model: Some(model.to_string()),
        token_usage: result.token_usage,
        cost,
    }
}
```

- [ ] **Step 4: Update `save_assistant_event()` to take `&EventStore` directly**

```rust
async fn save_assistant_event(
    event_store: &EventStore,
    result: &ExecutionResult,
    ctx: &FinalizeContext,
    vault_values: &[String],
) {
    let history_content = redact_secrets(&result.content, vault_values);
    let assistant_event = SessionEvent {
        id: uuid::Uuid::now_v7(),
        session_key: ctx.session_key_str.to_string(),
        event_type: EventType::AssistantMessage,
        content: history_content,
        metadata: EventMetadata {
            tools_used: result.tools_used.clone(),
            ..Default::default()
        },
        created_at: chrono::Utc::now(),
        sequence: 0,
    };
    if let Err(e) = event_store.append_event(&assistant_event).await {
        warn!("Failed to persist assistant event: {}", e);
    }
}
```

- [ ] **Step 5: Update imports**

Remove: `use crate::session::context::AgentContext;`
Add: `use gasket_storage::EventStore;`

- [ ] **Step 6: Verify build compiles**

Run: `cargo build --package gasket-engine`
Expected: Compile errors in `session/mod.rs` and `session/builder.rs` (expected — fix next).

- [ ] **Step 7: Local checkpoint**

```bash
# Local checkpoint — will be squashed. Do not push.
git add gasket/engine/src/session/finalizer.rs
git commit -m "checkpoint: ResponseFinalizer store refs"
```

---

### Task 3: Refactor AgentSession — remove `context`, add `event_store`, remove WikiComponents

**Files:**
- Modify: `gasket/engine/src/session/mod.rs`

**Context:** `AgentSession` currently holds `context: AgentContext` and `wiki_components: Option<WikiComponents>`. Remove both, add `event_store: Option<Arc<EventStore>>`. Update all methods that used `self.context.*`.

- [ ] **Step 1: Update `AgentSession` struct fields (line 241-258)**

Remove `context: AgentContext` and `wiki_components: Option<WikiComponents>`. Add non-optional store fields:

```rust
pub struct AgentSession {
    runtime_ctx: RuntimeContext,
    event_store: Arc<EventStore>,
    session_store: Arc<SessionStore>,
    config: AgentConfig,
    system_prompt: String,
    hooks: Arc<HookRegistry>,
    compactor: Option<Arc<ContextCompactor>>,
    pricing: Option<ModelPricing>,
    finalizer: ResponseFinalizer,
    pending_done: tokio_util::task::TaskTracker,
}
```

**Design note:** Stores are non-optional. `AgentSession` IS a persistent session — making stores `Option` just recreates the `Stateless` pattern under a different name. Tests use in-memory SQLite via `setup_test_db()`, exactly like existing tests.

- [ ] **Step 2: Remove `WikiComponents` struct, `WikiHealth` enum, and wiki accessor methods**

Delete (lines 216-347):
- `struct WikiComponents` (lines 217-222)
- `enum WikiHealth` (lines 224-233)
- Methods: `page_store()`, `page_index()`, `wiki_log()`, `wiki_health()` (lines 326-347)

Remove imports no longer needed:
- `use crate::wiki::{PageIndex, PageStore, WikiLog};` — remove `WikiLog` if only used by WikiComponents
- Check if `PageStore`/`PageIndex` still needed elsewhere in the file

- [ ] **Step 3: Rewrite `clear_session()` (line 349-357)**

```rust
pub async fn clear_session(&self, session_key: &SessionKey) {
    match self.event_store.clear_session(session_key).await {
        Ok(_) => info!("Session '{}' cleared", session_key),
        Err(e) => warn!("Failed to clear session '{}': {}", session_key, e),
    }
}
```

- [ ] **Step 4: Update `PipelineContext` (line 261-269)**

Remove `context: AgentContext`:

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

- [ ] **Step 5: Update `preprocess()` — checkpoint callback construction (lines 557-565)**

Replace the `AgentContext::Persistent` destructure:

```rust
// Before:
if let (Some(ref compactor), AgentContext::Persistent(ref persistent_ctx)) =
    (&self.compactor, &context)
{
    runtime_ctx.checkpoint_callback = Arc::new(SessionCheckpointCallback {
        session_key: fctx.session_key.clone(),
        compactor: compactor.clone(),
        event_store: persistent_ctx.event_store.clone(),
    });
}

// After:
if let Some(ref compactor) = &self.compactor {
    runtime_ctx.checkpoint_callback = Arc::new(SessionCheckpointCallback {
        session_key: fctx.session_key.clone(),
        compactor: compactor.clone(),
        event_store: self.event_store.clone(),
    });
}
```

- [ ] **Step 6: Update `preprocess()` — remove `context` from `PipelineContext` construction**

In both the `Aborted` branch and the normal branch, remove the `context: self.context.clone()` line from `PipelineContext` construction.

- [ ] **Step 7: Update `postprocess()` — remove `context` parameter**

```rust
async fn postprocess(result: ExecutionResult, ctx: &PipelineContext) -> AgentResponse {
    ctx.finalizer.finalize(result, &ctx.fctx, &ctx.model).await
}
```

- [ ] **Step 8: Update `prepare_pipeline()` — pass stores to ContextBuilder**

(The `session_store` field is already declared in Step 1's struct definition. Now wire it through `prepare_pipeline()`:)

```rust
async fn prepare_pipeline(
    &self,
    content: &str,
    session_key: &SessionKey,
) -> Result<history::builder::BuildOutcome, AgentError> {
    use history::builder::ContextBuilder;

    let history_config = gasket_storage::HistoryConfig {
        max_events: self.config.memory_window,
        ..Default::default()
    };

    let builder = ContextBuilder::new(
        self.event_store.clone(),
        self.session_store.clone(),
        self.system_prompt.clone(),
        None,
        self.hooks.clone(),
        history_config,
    );

    builder.build(content, session_key).await
}
```

- [ ] **Step 9: Update imports**

Remove: `use crate::session::context::AgentContext;`
Remove: `pub use context::{AgentContext, PersistentContext};` (line 16)
Add: `use gasket_storage::EventStore;` if not already imported

Remove: `use crate::wiki::{PageIndex, PageStore, WikiLog};` — only needed if other code in this file uses them. Check and keep only what's needed.

- [ ] **Step 10: Verify build**

Run: `cargo build --package gasket-engine`
Expected: Compile errors only in `session/builder.rs` (fix next).

- [ ] **Step 11: Local checkpoint**

```bash
# Local checkpoint — will be squashed. Do not push.
git add gasket/engine/src/session/mod.rs
git commit -m "checkpoint: AgentSession store refs + WikiComponents removal"
```

---

### Task 4: Update SessionBuilder — rewire store refs

**Files:**
- Modify: `gasket/engine/src/session/builder.rs`

**Context:** `SessionBuilder::build()` currently creates `AgentContext::persistent(event_store, session_store)` and passes it to `AgentSession`. Now it passes store refs directly to each consumer.

- [ ] **Step 1: Remove `AgentContext` construction**

Delete lines 92-93:
```rust
// DELETE:
let context = AgentContext::persistent(event_store.clone(), session_store.clone());
```

Remove import: `use crate::session::context::AgentContext;`

- [ ] **Step 2: Update `ResponseFinalizer::new()` call**

```rust
let finalizer = ResponseFinalizer::new(
    hooks.clone(),
    event_store.clone(),
    compactor.clone(),
    None, // pricing set later via with_pricing()
    self.config.max_tokens,
);
```

- [ ] **Step 3: Update `AgentSession` construction**

```rust
Ok(AgentSession {
    runtime_ctx,
    event_store,
    session_store,
    config: self.config,
    system_prompt,
    hooks,
    compactor,
    pricing: None,
    finalizer,
    pending_done,
})
```

Remove `wiki_components` field and `context` field.

- [ ] **Step 4: Simplify wiki availability check**

Replace `build_wiki_components()` (which constructs full WikiComponents only to check if wiki works) with a lightweight probe:

```rust
/// Check if wiki is configured and minimally functional.
///
/// Returns true if wiki config is enabled and the base path exists.
/// Does NOT initialize PageStore/PageIndex/WikiLog — that happens
/// during tool registration in `tools/builder.rs`.
fn is_wiki_available(config: &AgentConfig) -> bool {
    config.wiki.as_ref().map_or(false, |cfg| {
        cfg.enabled && std::path::Path::new(&cfg.base_path).exists()
    })
}
```

Usage in `build()`:

```rust
if is_wiki_available(&self.config) {
    system_prompt.push_str("\n\n");
    system_prompt.push_str(WIKI_PREPARATION_PROMPT);
}
```

**Note:** Verified — `WikiToolProvider` gets `page_store`/`page_index` from `ToolRegistryConfig` in `tools/builder.rs`, not from `AgentSession.wiki_components`. The wiki accessor methods on `AgentSession` (`page_store()`, `page_index()`, `wiki_log()`, `wiki_health()`) have zero external callers — confirmed by grep. So `wiki_components` can be removed from `AgentSession` entirely, and `build_wiki_components()` is replaced by `is_wiki_available()` for prompt decisions only.

- [ ] **Step 5: Verify build**

Run: `cargo build --package gasket-engine`
Expected: Compile errors in `lib.rs` (fix next).

- [ ] **Step 6: Local checkpoint**

```bash
# Local checkpoint — will be squashed. Do not push.
git add gasket/engine/src/session/builder.rs
git commit -m "checkpoint: SessionBuilder store refs"
```

---

### Task 5: Update lib.rs — remove re-exports

**Files:**
- Modify: `gasket/engine/src/lib.rs`

**Context:** Remove `AgentContext`, `PersistentContext`, `WikiHealth` from public re-exports.

- [ ] **Step 1: Update re-export line (line 36-38)**

```rust
// Before:
pub use session::{
    AgentConfig, AgentContext, AgentResponse, ContextCompactor, PersistentContext, WikiHealth,
};

// After:
pub use session::{AgentConfig, AgentResponse, ContextCompactor};
```

- [ ] **Step 2: Update module doc comment (line 14)**

```rust
// Before:
//! - **Enum-based dispatch**: `AgentContext` enum instead of trait objects

// After:
//! - **Direct store refs**: Components hold `Arc<EventStore>` directly
```

- [ ] **Step 3: Verify build**

Run: `cargo build --package gasket-engine`
Expected: Clean compile (or errors only from external callers).

- [ ] **Step 4: Check for external consumers**

Run: `grep -r "AgentContext\|PersistentContext\|WikiHealth" --include="*.rs" gasket/cli/ gasket/channels/ gasket/broker/`
Expected: No matches. If there are matches, update those files too.

- [ ] **Step 5: Local checkpoint**

```bash
# Local checkpoint — will be squashed. Do not push.
git add gasket/engine/src/lib.rs
git commit -m "checkpoint: lib.rs re-exports cleanup"
```

---

### Task 6: Delete context.rs

**Files:**
- Delete: `gasket/engine/src/session/context.rs`

**Context:** Remove the file and the module declaration.

- [ ] **Step 1: Remove module declaration**

In `session/mod.rs`, delete:
```rust
pub mod context;
```

- [ ] **Step 2: Delete the file**

```bash
rm gasket/engine/src/session/context.rs
```

- [ ] **Step 3: Verify build**

Run: `cargo build --package gasket-engine`
Expected: Clean compile.

- [ ] **Step 4: Atomic commit (squash all Tasks 1-6 into one)**

```bash
# Squash all Tasks 1-6 checkpoints into a single atomic commit:
git reset --soft HEAD~5  # count of checkpoint commits above
git add -A
git commit -m "refactor: eliminate AgentContext — components hold store refs directly

Remove AgentContext enum and PersistentContext struct. Components that
previously depended on AgentContext for storage I/O now hold Arc<EventStore>
or Arc<SessionStore> directly (non-optional — AgentSession IS persistent):

- ContextBuilder: holds Arc<EventStore> + Arc<SessionStore>
- ResponseFinalizer: holds Arc<EventStore>
- AgentSession: holds Arc<EventStore> + Arc<SessionStore> (non-optional)
- SessionStore: gains load_summary_with_checkpoint() method

Also remove WikiComponents/WikiHealth from AgentSession (unused — wiki
tools get refs from ToolRegistryConfig), replace build_wiki_components()
with lightweight is_wiki_available() probe, and drop corresponding
re-exports from lib.rs."
```

---

### Task 7: Verify full workspace build + tests

**Files:** None (verification only, plus test migration/creation)

**Test Migration from `context.rs`:**

| Original test | Action |
|------|--------|
| `test_stateless_context_is_not_persistent` | Delete — `Stateless` variant removed |
| `test_stateless_load_session` | Delete — `Stateless` variant removed |
| `test_stateless_save_event` | Delete — `Stateless` variant removed |
| `test_stateless_context_clear_session` | Delete — `Stateless` variant removed |
| `test_persistent_context_creation` | Migrate to `history/builder.rs` — verifies ContextBuilder store setup with in-memory SQLite |
| `test_persistent_context_save_event` | Migrate to `finalizer.rs` — verifies ResponseFinalizer saves assistant event correctly |

**New tests to add:**

| File | Test | What it covers |
|------|------|----------------|
| `history/builder.rs` | `test_builder_creates_chat_request` | `ContextBuilder::build()` with in-memory SQLite produces a valid `ChatRequest` |
| `finalizer.rs` | `test_finalizer_persists_event` | `finalize()` persists assistant event to `EventStore` |
| `session/mod.rs` | `test_clear_session_works` | `clear_session()` clears events for given session key |

- [ ] **Step 1: Delete Stateless tests — 4 tests removed from `context.rs`**

These tests tested the `Stateless` variant which no longer exists. No replacement needed.

- [ ] **Step 2: Migrate `test_persistent_context_creation` to `history/builder.rs`**

The test verifies `ContextBuilder` can use an in-memory SQLite-backed store. Create `setup_test_db()` helper (already in `context.rs` — move it) and verify `builder.build()` succeeds.

- [ ] **Step 3: Migrate `test_persistent_context_save_event` to `finalizer.rs`**

The test verifies `ResponseFinalizer::finalize()` persists an assistant event. Use `setup_test_db()` and verify the event appears in the store after `finalize()` completes.

- [ ] **Step 4: Add `clear_session` test to `session/mod.rs`**

```rust
#[tokio::test]
async fn test_clear_session_works() {
    let pool = setup_test_db().await;
    let event_store = Arc::new(EventStore::new(pool.clone()));
    let session_store = Arc::new(SessionStore::new(pool.clone()));
    let session = /* build AgentSession with event_store, session_store */;
    // Save an event first
    let event = SessionEvent { session_key: "test:session".into(), ... };
    event_store.append_event(&event).await.unwrap();
    // Clear session
    session.clear_session(&SessionKey::parse("test:session").unwrap()).await;
    // Verify events for this session are gone
    let events = event_store.get_session_history(&SessionKey::parse("test:session").unwrap()).await.unwrap();
    assert!(events.is_empty());
}
```

- [ ] **Step 5: Full workspace build**

Run: `cargo build --workspace`
Expected: Clean compile.

- [ ] **Step 6: Run engine tests**

Run: `cargo test --package gasket-engine`
Expected: All tests pass, including migrated and new tests.

- [ ] **Step 7: Run full workspace tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 8: Verify no remaining AgentContext references in code**

Run: `grep -r "AgentContext" --include="*.rs" gasket/`
Expected: No matches outside spec/docs files. (Already verified: zero matches in cli/, channels/, broker/)

---

### Task 8: Update documentation references

**Files:**
- Various files in `docs/`

**Context:** Multiple doc files reference `AgentContext`. Update them to reflect the new architecture.

- [ ] **Step 1: Find all doc references**

Run: `grep -rn "AgentContext" docs/`
List all matches.

- [ ] **Step 2: Update each doc file**

For each match found:
- Remove or replace `AgentContext` references
- Update diagrams showing the enum-based dispatch
- Update to reflect direct store ref pattern

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs: update AgentContext references after refactoring"
```

---

### Task 9: Update CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` (project root)

**Context:** CLAUDE.md references the wiki system and may reference AgentContext.

- [ ] **Step 1: Search for AgentContext references in CLAUDE.md**

Run: `grep -n "AgentContext\|context.rs" CLAUDE.md`

- [ ] **Step 2: Update references if found**

Update architecture description to reflect direct store refs pattern.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md after AgentContext removal"
```
