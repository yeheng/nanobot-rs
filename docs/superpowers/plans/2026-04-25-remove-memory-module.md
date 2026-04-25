# Remove Memory Module — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the legacy memory abstraction layer, unifying on wiki tools and SqliteStore directly.

**Architecture:** Delete 4 memory tool files, remove the MemoryStore wrapper, remove the `pub mod memory` facade, replace the memory SKILL.md with a wiki SKILL.md. Storage types are re-exported at the `gasket_engine` top level.

**Tech Stack:** Rust, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-04-25-remove-memory-module-design.md`

---

## File Structure

### Delete
- `gasket/engine/src/tools/memorize.rs`
- `gasket/engine/src/tools/memory_search.rs`
- `gasket/engine/src/tools/memory_refresh.rs`
- `gasket/engine/src/tools/memory_decay.rs`
- `gasket/engine/src/session/store.rs`
- `workspace/skills/memory/SKILL.md` (directory renamed)

### Create
- `workspace/skills/wiki/SKILL.md`

### Modify
- `gasket/engine/src/lib.rs` — add top-level storage re-exports, remove `pub mod memory` facade, remove `MemorySearchTool` from tools re-export
- `gasket/engine/src/tools/mod.rs` — remove memory tool module declarations and re-exports
- `gasket/engine/src/tools/provider.rs` — remove memory tool imports and registrations, switch `crate::memory::*` imports
- `gasket/engine/src/tools/builder.rs` — switch `crate::memory::SqliteStore` import
- `gasket/engine/src/session/mod.rs` — remove `mod store` / `pub use store::MemoryStore`, replace `MemoryStore` with `SqliteStore` in `new()` and `with_memory_store()`
- `gasket/engine/src/session/builder.rs` — replace `MemoryStore` with `SqliteStore`, remove `.sqlite_store()` calls
- `gasket/cli/src/commands/agent.rs` — switch `gasket_engine::memory::MemoryStore` → `gasket_engine::SqliteStore`
- `gasket/cli/src/commands/gateway.rs` — switch all `gasket_engine::memory::*` imports
- `gasket/cli/src/commands/tool.rs` — switch `gasket_engine::session::MemoryStore` → `gasket_engine::SqliteStore`

---

### Task 1: Add top-level storage re-exports in lib.rs

**Files:**
- Modify: `gasket/engine/src/lib.rs:194-200`

This step is purely additive — both old (`crate::memory::*`) and new (`crate::*`) paths work.

- [ ] **Step 1: Add top-level storage re-exports**

In `gasket/engine/src/lib.rs`, add after the existing `pub use gasket_storage::{...}` block (around line 56-59):

```rust
// ── Storage (top-level re-exports, replacing pub mod memory facade) ─
pub use gasket_storage::{
    CronStore, EventStore, KvStore, MaintenanceStore, SessionStore, SqliteStore, StoreError,
};
```

Note: The existing `HistoryConfig`, `HistoryQuery`, etc. re-exports from `gasket_storage` at line 56-59 remain unchanged. The new block adds the storage types.

- [ ] **Step 2: Verify compilation**

Run: `cargo build --package gasket-engine`
Expected: SUCCESS (additive change only)

- [ ] **Step 3: Commit**

```bash
git add gasket/engine/src/lib.rs
git commit -m "refactor(engine): add top-level storage re-exports"
```

---

### Task 2: Remove memory tool files and clean up registration

**Files:**
- Delete: `gasket/engine/src/tools/memorize.rs`
- Delete: `gasket/engine/src/tools/memory_search.rs`
- Delete: `gasket/engine/src/tools/memory_refresh.rs`
- Delete: `gasket/engine/src/tools/memory_decay.rs`
- Modify: `gasket/engine/src/tools/mod.rs`
- Modify: `gasket/engine/src/tools/provider.rs`
- Modify: `gasket/engine/src/lib.rs` (remove `MemorySearchTool` from tools re-export)

- [ ] **Step 1: Delete memory tool files**

```bash
rm gasket/engine/src/tools/memorize.rs
rm gasket/engine/src/tools/memory_search.rs
rm gasket/engine/src/tools/memory_refresh.rs
rm gasket/engine/src/tools/memory_decay.rs
```

- [ ] **Step 2: Update tools/mod.rs**

Remove these lines from `gasket/engine/src/tools/mod.rs`:

```rust
mod memorize;
mod memory_decay;
mod memory_refresh;
mod memory_search;
```

Remove these `pub use` lines:

```rust
pub use memorize::MemorizeTool;
pub use memory_decay::MemoryDecayTool;
pub use memory_refresh::MemoryRefreshTool;
pub use memory_search::MemorySearchTool;
```

- [ ] **Step 3: Update tools/provider.rs imports**

In `gasket/engine/src/tools/provider.rs`, remove from the import block:

```rust
// Remove from the use super { ... } block:
MemorizeTool, MemoryDecayTool, MemoryRefreshTool, MemorySearchTool,
```

- [ ] **Step 4: Update tools/provider.rs WikiToolProvider registration**

In `WikiToolProvider::register_tools`, remove the `MemorizeTool` and `MemorySearchTool` registration blocks (the two `reg!()` calls for those tools).

- [ ] **Step 5: Update tools/provider.rs SystemToolProvider registration**

In `SystemToolProvider::register_tools`, remove the `MemoryDecayTool` and `MemoryRefreshTool` registration blocks (the two `reg!()` calls for those tools in the `if let Some(ref store) = self.page_store` block).

- [ ] **Step 6: Remove MemorySearchTool from lib.rs tools re-export**

In `gasket/engine/src/lib.rs`, find:

```rust
pub use tools::{
    CronTool, EditFileTool, ExecTool, ListDirTool, MemorySearchTool, MessageTool, ReadFileTool,
    SpawnParallelTool, SpawnTool, ToolRegistry, WebFetchTool, WebSearchTool, WriteFileTool,
};
```

Remove `MemorySearchTool` from the list:

```rust
pub use tools::{
    CronTool, EditFileTool, ExecTool, ListDirTool, MessageTool, ReadFileTool,
    SpawnParallelTool, SpawnTool, ToolRegistry, WebFetchTool, WebSearchTool, WriteFileTool,
};
```

- [ ] **Step 7: Verify compilation**

Run: `cargo build --package gasket-engine`
Expected: SUCCESS

- [ ] **Step 8: Commit**

```bash
git add -A gasket/engine/src/tools/ gasket/engine/src/lib.rs
git commit -m "refactor(engine): remove memory tool files and registration"
```

---

### Task 3: Switch engine internal imports to new paths

**Files:**
- Modify: `gasket/engine/src/tools/builder.rs`
- Modify: `gasket/engine/src/tools/provider.rs`

- [ ] **Step 1: Update builder.rs**

In `gasket/engine/src/tools/builder.rs:11`, change:

```rust
use crate::memory::SqliteStore;
```

to:

```rust
use crate::SqliteStore;
```

- [ ] **Step 2: Update provider.rs**

In `gasket/engine/src/tools/provider.rs:10`, change:

```rust
use crate::memory::{MaintenanceStore, SessionStore};
```

to:

```rust
use crate::{MaintenanceStore, SessionStore};
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build --package gasket-engine`
Expected: SUCCESS (both old and new paths resolve to the same types)

- [ ] **Step 4: Commit**

```bash
git add gasket/engine/src/tools/builder.rs gasket/engine/src/tools/provider.rs
git commit -m "refactor(engine): switch internal imports from crate::memory to crate::*"
```

---

### Task 4: Remove MemoryStore and pub mod memory facade

**Files:**
- Delete: `gasket/engine/src/session/store.rs`
- Modify: `gasket/engine/src/session/mod.rs`
- Modify: `gasket/engine/src/session/builder.rs`
- Modify: `gasket/engine/src/lib.rs`
- Modify: `gasket/cli/src/commands/agent.rs`
- Modify: `gasket/cli/src/commands/gateway.rs`
- Modify: `gasket/cli/src/commands/tool.rs`

This task must be done atomically — all changes in one commit — because removing MemoryStore breaks the facade, and removing the facade breaks CLI imports.

- [ ] **Step 1: Delete session/store.rs**

```bash
rm gasket/engine/src/session/store.rs
```

- [ ] **Step 2: Update session/mod.rs**

Remove:
```rust
pub use store::MemoryStore;
```

Remove `mod store;` if it's declared (it may be implicit via `pub use store::MemoryStore`).

Change `AgentSession::new()`:
```rust
// Before:
let memory_store = Arc::new(MemoryStore::new().await);
Self::with_memory_store(provider, workspace, config, tools, memory_store).await

// After:
let sqlite_store = Arc::new(SqliteStore::new().await.expect("Failed to open SqliteStore"));
Self::with_sqlite_store(provider, workspace, config, tools, sqlite_store).await
```

Change `AgentSession::with_memory_store()` — rename to `with_sqlite_store()` and update signature:
```rust
// Before:
pub async fn with_memory_store(
    provider: Arc<dyn gasket_providers::LlmProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    tools: Arc<ToolRegistry>,
    memory_store: Arc<MemoryStore>,
) -> Result<Self, AgentError> {
    builder::build_session(provider, workspace, config, tools, memory_store).await
}

// After:
pub async fn with_sqlite_store(
    provider: Arc<dyn gasket_providers::LlmProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    tools: Arc<ToolRegistry>,
    sqlite_store: Arc<SqliteStore>,
) -> Result<Self, AgentError> {
    builder::build_session(provider, workspace, config, tools, sqlite_store).await
}
```

Add import if not present: `use gasket_storage::SqliteStore;`

Also update the `session` module re-exports. Remove:
```rust
pub use store::MemoryStore;
```

From the top-level `pub use session::...` in `session/mod.rs`, update the `lib.rs` re-export. The `MemoryStore` reference in `lib.rs` line 38:
```rust
pub use session::{
    AgentConfig, AgentContext, AgentResponse, ContextCompactor, MemoryStore, PersistentContext,
    WikiHealth,
};
```
Remove `MemoryStore` from this line.

- [ ] **Step 3: Update session/builder.rs**

Change the struct field:
```rust
// Before:
memory_store: Arc<crate::session::MemoryStore>,

// After:
sqlite_store: Arc<gasket_storage::SqliteStore>,
```

Change `SessionBuilder::new()` signature and body:
```rust
// Before:
pub fn new(
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    tools: Arc<crate::tools::ToolRegistry>,
    memory_store: Arc<crate::session::MemoryStore>,
) -> Self {
    Self { provider, workspace, config, tools, memory_store }
}

// After:
pub fn new(
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    tools: Arc<crate::tools::ToolRegistry>,
    sqlite_store: Arc<gasket_storage::SqliteStore>,
) -> Self {
    Self { provider, workspace, config, tools, sqlite_store }
}
```

In `build()`, change `self.memory_store` references:
```rust
// Before:
let pool = self.memory_store.sqlite_store().pool();

// After:
let pool = self.sqlite_store.pool();
```

```rust
// Before:
build_wiki_components(self.memory_store.sqlite_store(), &self.config).await;

// After:
build_wiki_components(&self.sqlite_store, &self.config).await;
```

Change `build_session()` function:
```rust
// Before:
pub async fn build_session(
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    tools: Arc<crate::tools::ToolRegistry>,
    memory_store: Arc<crate::session::MemoryStore>,
) -> Result<AgentSession, AgentError> {
    SessionBuilder::new(provider, workspace, config, tools, memory_store)
        .build()
        .await
}

// After:
pub async fn build_session(
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    tools: Arc<crate::tools::ToolRegistry>,
    sqlite_store: Arc<gasket_storage::SqliteStore>,
) -> Result<AgentSession, AgentError> {
    SessionBuilder::new(provider, workspace, config, tools, sqlite_store)
        .build()
        .await
}
```

- [ ] **Step 4: Update lib.rs — remove facade and clean up**

Remove the entire `pub mod memory` block:
```rust
// Core storage types (memory module name kept for backward compatibility)
pub mod memory {
    pub use crate::session::MemoryStore;
    pub use gasket_storage::{
        CronStore, EventStore, KvStore, MaintenanceStore, SessionStore, SqliteStore, StoreError,
    };
}
```

Remove `MemoryStore` from the session re-export:
```rust
// Before:
pub use session::{
    AgentConfig, AgentContext, AgentResponse, ContextCompactor, MemoryStore, PersistentContext,
    WikiHealth,
};

// After:
pub use session::{
    AgentConfig, AgentContext, AgentResponse, ContextCompactor, PersistentContext,
    WikiHealth,
};
```

The top-level storage re-exports added in Task 1 now serve as the replacement.

- [ ] **Step 5: Update cli/commands/agent.rs**

```rust
// Before (line 14):
use gasket_engine::memory::MemoryStore;

// After:
use gasket_engine::SqliteStore;
```

```rust
// Before (line 71):
let memory_store = Arc::new(MemoryStore::new().await);
let pool = memory_store.sqlite_store().pool();

// After:
let sqlite_store = Arc::new(SqliteStore::new().await.expect("Failed to open SqliteStore"));
let pool = sqlite_store.pool();
```

```rust
// Before (line 123):
sqlite_store: Some(memory_store.sqlite_store().clone()),

// After:
sqlite_store: Some(sqlite_store.as_ref().clone()),
```

Note: `SqliteStore` implements `Clone` via its `SqlitePool` internally. Use `sqlite_store.as_ref().clone()` or `(*sqlite_store).clone()`.

```rust
// Before (line 169):
let agent = AgentSession::with_memory_store(
    provider_info.provider, workspace, agent_config, tools, memory_store,
)

// After:
let agent = AgentSession::with_sqlite_store(
    provider_info.provider, workspace, agent_config, tools, sqlite_store,
)
```

- [ ] **Step 6: Update cli/commands/gateway.rs**

```rust
// Before (lines 14-15):
use gasket_engine::memory::EventStore;
use gasket_engine::memory::MemoryStore;

// After:
use gasket_engine::EventStore;
use gasket_engine::SqliteStore;
```

Also fix the `gasket_engine::memory::SqliteStore` reference (around line 56):
```rust
// Before:
let sqlite_store = Arc::new(
    gasket_engine::memory::SqliteStore::new()
        .await
        .expect("Failed to open SQLite store for cron persistence"),
);

// After:
let sqlite_store = Arc::new(
    gasket_engine::SqliteStore::new()
        .await
        .expect("Failed to open SQLite store for cron persistence"),
);
```

And the `gasket_engine::memory::SessionStore` reference (around line 293):
```rust
// Before:
let ctx_session_store = Arc::new(gasket_engine::memory::SessionStore::new(ctx_pool));

// After:
let ctx_session_store = Arc::new(gasket_engine::SessionStore::new(ctx_pool));
```

Update all `memory_store` variables and `MemoryStore` types to `SqliteStore`:
- `let memory_store = Arc::new(MemoryStore::new().await);` → `let sqlite_store = Arc::new(SqliteStore::new().await.expect("Failed to open SqliteStore"));`
- All `memory_store.sqlite_store()` calls → direct `sqlite_store.` method calls
- All `&Arc<MemoryStore>` parameters → `&Arc<SqliteStore>`
- All `AgentSession::with_memory_store(...)` → `AgentSession::with_sqlite_store(...)`

- [ ] **Step 7: Update cli/commands/tool.rs**

```rust
// Before (line 23):
let memory_store = gasket_engine::session::MemoryStore::new().await;
let sqlite_store = memory_store.sqlite_store().clone();

// After:
let sqlite_store = gasket_engine::SqliteStore::new()
    .await
    .expect("Failed to open SqliteStore");
```

- [ ] **Step 8: Verify compilation**

Run: `cargo build --workspace`
Expected: SUCCESS

- [ ] **Step 9: Run tests**

Run: `cargo test --workspace`
Expected: ALL PASS

- [ ] **Step 10: Commit**

```bash
git add -A gasket/engine/src/session/ gasket/engine/src/lib.rs gasket/cli/src/commands/
git commit -m "refactor: remove MemoryStore and pub mod memory facade"
```

---

### Task 5: Replace memory SKILL.md with wiki SKILL.md

**Files:**
- Rename: `workspace/skills/memory/` → `workspace/skills/wiki/`
- Create: `workspace/skills/wiki/SKILL.md`

- [ ] **Step 1: Remove old skill directory**

```bash
rm -rf workspace/skills/memory/
mkdir -p workspace/skills/wiki/
```

- [ ] **Step 2: Create wiki SKILL.md**

Write `workspace/skills/wiki/SKILL.md`:

```yaml
---
name: wiki
description: Operational guide for gasket's wiki knowledge system
always: false
---

# Wiki Skill

Operational guide for reading, writing, and managing wiki knowledge pages.

## Path Conventions

| Content Type | Path Pattern | page_type |
|---|---|---|
| General knowledge | `topics/<slug>` | `topic` |
| People, projects, teams | `entities/<slug>` | `entity` |
| External references, URLs | `sources/<slug>` | `source` |
| Step-by-step procedures | `sop/<slug>` | `sop` |

## Common Operations

### Write a Page

```
wiki_write(
    path: "topics/rust-async-patterns",
    title: "Rust Async Patterns",
    content: "## Overview\n...",
    page_type: "topic",
    tags: ["rust", "async"]
)
```

### Search Pages

```
wiki_search(query: "database design", limit: 10)
```

### Read a Page

```
wiki_read(path: "topics/rust-async-patterns")
```

### Refresh Index

```
wiki_refresh(action: "sync")     # Sync changed files only
wiki_refresh(action: "reindex")  # Full rebuild
wiki_refresh(action: "stats")    # Show statistics
```

## Best Practices

1. **Search before write** — use `wiki_search` first to avoid duplicates.
2. **One concept per page** — easier retrieval and lifecycle management.
3. **Use descriptive paths** — `topics/event-sourcing-design` over `topics/note-1`.
4. **At least 2 tags** — tags improve search quality.
5. **Use entities for people/projects** — `entities/people/alice`, `entities/projects/gasket`.

## Manual Operations

```bash
# Browse wiki files
ls ~/.gasket/wiki/topics/
cat ~/.gasket/wiki/topics/rust-async-patterns.md

# Search via CLI
gasket wiki search <query>

# Rebuild index after manual edits
gasket wiki reindex
```
```

- [ ] **Step 3: Commit**

```bash
git add -A workspace/skills/
git commit -m "refactor(skills): replace memory SKILL with wiki SKILL"
```

---

### Task 6: Final verification

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: SUCCESS

- [ ] **Step 2: Full workspace test**

Run: `cargo test --workspace`
Expected: ALL PASS

- [ ] **Step 3: Verify no memory references remain**

Run: `grep -rn "MemoryStore" gasket/`
Expected: zero hits

Run: `grep -rn "crate::memory" gasket/`
Expected: zero hits

Run: `grep -rn "gasket_engine::memory" gasket/`
Expected: zero hits

Run: `ls workspace/skills/wiki/SKILL.md`
Expected: file exists

Run: `ls workspace/skills/memory/ 2>&1`
Expected: "No such file or directory"
