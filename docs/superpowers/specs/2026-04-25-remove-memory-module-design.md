# Remove Memory Module — Design Spec

**Date:** 2026-04-25
**Status:** Approved
**Scope:** engine, cli, workspace/skills

## Background

The "memory" module in Gasket is a legacy naming layer over the wiki system. All memory tools (`memorize`, `memory_search`, `memory_refresh`, `memory_decay`) internally call wiki `PageStore`/`PageIndex`. The `MemoryStore` struct is a thin wrapper over `SqliteStore`. The `pub mod memory` facade in `lib.rs` re-exports `gasket_storage` types.

This creates unnecessary indirection and confusing naming for no functional benefit.

## Goal

Remove the entire memory abstraction layer. Unify on wiki tools for knowledge management and `SqliteStore` directly for database access. Re-export storage types at the `gasket_engine` top level to keep CLI decoupled from `gasket_storage`.

## Decisions

1. **No migration of mapping logic** — `memorize` tool's `scenario → path prefix` mapping is not transferred to `wiki_write`. Agents will use wiki tools directly.
2. **Storage types re-exported at engine top level** — `SqliteStore`, `SessionStore`, etc. become `gasket_engine::SqliteStore` instead of `gasket_engine::memory::SqliteStore`.

## Changes

### 1. Delete files

| File | Reason |
|---|---|
| `engine/src/tools/memorize.rs` | Superseded by `wiki_write` |
| `engine/src/tools/memory_search.rs` | Superseded by `wiki_search` |
| `engine/src/tools/memory_refresh.rs` | Superseded by `wiki_refresh` |
| `engine/src/tools/memory_decay.rs` | Superseded by `wiki_decay` |
| `workspace/skills/memory/SKILL.md` | No longer applicable |

### 2. Remove `MemoryStore`

Delete `engine/src/session/store.rs` entirely.

Update all call sites to use `SqliteStore` directly:

| File | Change |
|---|---|
| `engine/src/session/mod.rs` | Remove `pub use store::MemoryStore`; replace internal `MemoryStore::new()` with `SqliteStore::new()` |
| `engine/src/session/builder.rs` | `Arc<MemoryStore>` → `Arc<SqliteStore>`; remove `.sqlite_store()` calls |
| `cli/src/commands/agent.rs` | `MemoryStore::new()` → `SqliteStore::new()` |
| `cli/src/commands/gateway.rs` | Same |
| `cli/src/commands/tool.rs` | Same |

`MemoryStore` provides two methods that need direct equivalents:
- `.sqlite_store()` → removed (use the `SqliteStore` directly)
- `.pool()` → `SqliteStore::pool()` (already exists)

### 3. Remove `pub mod memory` facade

In `engine/src/lib.rs`:

**Remove:**
```rust
pub use session::MemoryStore;

pub mod memory {
    pub use crate::session::MemoryStore;
    pub use gasket_storage::{
        CronStore, EventStore, KvStore, MaintenanceStore, SessionStore, SqliteStore, StoreError,
    };
}
```

**Add to top-level re-exports:**
```rust
pub use gasket_storage::{
    CronStore, EventStore, KvStore, MaintenanceStore, SessionStore, SqliteStore, StoreError,
};
```

Update import paths in downstream code:
- `engine/src/tools/builder.rs`: `use crate::memory::SqliteStore` → `use crate::SqliteStore`
- `engine/src/tools/provider.rs`: `use crate::memory::{MaintenanceStore, SessionStore}` → `use crate::{MaintenanceStore, SessionStore}`
- `cli/src/commands/agent.rs`: `use gasket_engine::memory::MemoryStore` → `use gasket_engine::SqliteStore`
- `cli/src/commands/gateway.rs`: same
- `cli/src/commands/tool.rs`: `use gasket_engine::session::MemoryStore` → `use gasket_engine::SqliteStore`

### 4. Clean up tool registration

In `engine/src/tools/mod.rs`:
- Remove `mod memorize;`, `mod memory_decay;`, `mod memory_refresh;`, `mod memory_search;`
- Remove corresponding `pub use` lines

In `engine/src/tools/provider.rs`:
- Remove `MemorizeTool`, `MemoryDecayTool`, `MemoryRefreshTool`, `MemorySearchTool` from imports
- `WikiToolProvider::register_tools`: remove `MemorizeTool` and `MemorySearchTool` registration
- `SystemToolProvider::register_tools`: remove `MemoryDecayTool` and `MemoryRefreshTool` registration

In `engine/src/lib.rs` top-level tools re-export:
- Remove `MemorySearchTool` from `pub use tools::{...}`

### 5. Update `session/mod.rs` exports

Remove `pub use store::MemoryStore;` and the `mod store;` declaration (or remove the entire `store.rs` file).

### 6. Remove wiki backward-compat comment in `lib.rs`

Remove the comment `// Core storage types (memory module name kept for backward compatibility)` — no longer applicable.

## Unchanged

- All wiki tools (`wiki_write`, `wiki_search`, `wiki_refresh`, `wiki_decay`, `wiki_read`, `search_sops`)
- `PageStore`, `PageIndex`, `WikiPage` and all wiki types
- `gasket_storage` crate internals
- Wiki CLI commands

## Verification

1. `cargo build --workspace` — compiles without errors
2. `cargo test --workspace` — all tests pass
3. `grep -r "memory" engine/src/tools/` — only references to in-memory SQLite, not the memory module
4. `grep -r "MemoryStore" gasket/` — zero hits
5. `grep -r "crate::memory" gasket/` — zero hits
