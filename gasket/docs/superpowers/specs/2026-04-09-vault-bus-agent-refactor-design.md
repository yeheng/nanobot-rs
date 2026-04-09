# Vault/Bus Migration + Agent Module Refactoring

**Date:** 2026-04-09  
**Status:** Design  
**Approach:** Big Bang Refactor (Approach 2)

## Motivation

**Vault & Bus Migration:**
- Simplify workspace structure (fewer crates to manage)
- Improve performance by reducing cross-crate boundaries
- Both crates are small (~1835 and ~484 lines) and already partially integrated into engine

**Agent Module Refactoring:**
- Reduce file count from 20 files to 16 files total (10 implementation files + 6 mod.rs files)
- Clarify boundaries and responsibilities between modules
- Improve execution flow and reduce complexity
- Better separate concerns (memory, execution, streaming, subagents)

## Current State

### Workspace Structure
```
gasket/
├── types/
├── vault/          # 1835 lines - encryption, scanning, storage
├── storage/
├── bus/            # 484 lines - three-actor message pipeline
├── engine/         # Main orchestration crate
│   ├── src/vault/  # VaultInjector (re-exports gasket_vault)
│   └── src/bus/    # Facade (re-exports gasket_bus)
├── cli/
├── providers/
├── channels/
├── sandbox/
└── tantivy/
```

### Agent Module (20 files, ~8000 lines)
```
engine/src/agent/
├── mod.rs (44 lines)
├── loop_.rs (1007 lines) - Main agent loop
├── executor.rs (224 lines) - ToolExecutor
├── executor_core.rs (651 lines) - AgentExecutor
├── request.rs (151 lines) - RequestHandler
├── memory.rs (50 lines) - MemoryStore wrapper
├── memory_manager.rs (1438 lines) - Three-phase loading
├── memory_provider.rs (49 lines) - Memory provider
├── compactor.rs (382 lines) - Context compression
├── context.rs (684 lines) - AgentContext enum
├── context_builder.rs (328 lines) - Context building
├── history_coordinator.rs (185 lines) - History coordination
├── indexing.rs (455 lines) - IndexingService
├── indexing_queue.rs (150 lines) - IndexingQueue
├── stream.rs (424 lines) - StreamEvent
├── stream_buffer.rs (113 lines) - BufferedEvents
├── subagent.rs (1090 lines) - SubagentManager
├── subagent_tracker.rs (278 lines) - SubagentTracker
├── skill_loader.rs (107 lines) - Skill loading
└── prompt.rs (153 lines) - Prompt assembly
```

## Target Architecture

### Part 1: Vault Migration

**New Structure:**
```
engine/src/vault/
├── mod.rs              # Public API + re-exports
├── injector.rs         # VaultInjector (ChatMessage integration)
└── core/               # Core vault functionality (from vault crate)
    ├── crypto.rs       # Encryption/decryption
    ├── error.rs        # VaultError
    ├── redaction.rs    # Secret redaction
    ├── scanner.rs      # Placeholder scanning
    └── store.rs        # VaultStore
```

**Dependencies to add to `engine/Cargo.toml`:**
```toml
chacha20poly1305 = "0.10"
argon2 = { version = "0.5", features = ["std"] }
rand = "0.8"
zeroize = { version = "1.7", features = ["zeroize_derive"] }
```

**Import changes:**
- `use gasket_vault::` → `use crate::vault::core::`
- External: `use gasket_engine::vault::`

### Part 2: Bus Migration

**New Structure:**
```
engine/src/bus/
├── mod.rs              # Public API + re-exports
├── actors.rs           # Router, Session, Outbound actors
└── queue.rs            # MessageBus
```

**Import changes:**
- `use gasket_bus::` → `use crate::bus::`
- External: `use gasket_engine::bus::`

### Part 3: Agent Module Refactoring

**New Structure (20 files → 16 files total):**
- 10 implementation files (core logic)
- 6 mod.rs files (module organization)

**File breakdown:**
```
engine/src/agent/
├── mod.rs                  # Public API + re-exports
├── core/
│   ├── mod.rs             # Core types
│   ├── loop_.rs           # AgentLoop (1007 lines)
│   ├── config.rs          # AgentConfig (extracted from loop_.rs)
│   └── context.rs         # AgentContext enum (684 lines)
├── execution/
│   ├── mod.rs             # Execution subsystem
│   ├── executor.rs        # MERGED: executor + executor_core + request (~1000 lines)
│   └── prompt.rs          # Prompt assembly (153 lines)
├── memory/
│   ├── mod.rs             # Memory subsystem
│   ├── manager.rs         # MemoryManager (1438 lines)
│   ├── store.rs           # MERGED: memory + memory_provider (~100 lines)
│   └── compactor.rs       # ContextCompactor (382 lines)
├── history/
│   ├── mod.rs             # History subsystem
│   ├── coordinator.rs     # HistoryCoordinator (185 lines)
│   ├── indexing.rs        # MERGED: indexing + indexing_queue (~600 lines)
│   └── builder.rs         # RENAMED: context_builder.rs (328 lines)
├── streaming/
│   ├── mod.rs             # Streaming subsystem
│   └── stream.rs          # MERGED: stream + stream_buffer (~540 lines)
└── subagents/
    ├── mod.rs             # Subagent subsystem
    ├── manager.rs         # SubagentManager + Builder (1090 lines)
    ├── tracker.rs         # SubagentTracker (278 lines)
    └── runner.rs          # EXTRACTED: run_subagent + ModelResolver (~50 lines)
```

## Module Boundaries & Responsibilities

### `agent/core/` - Core types and main loop
- **Responsibility:** Agent configuration, context management, main execution loop
- **Public API:** `AgentLoop`, `AgentConfig`, `AgentContext`, `PersistentContext`
- **Dependencies:** All other agent submodules
- **Key insight:** The main orchestration layer that coordinates all subsystems

### `agent/execution/` - Request execution
- **Responsibility:** Execute LLM requests, handle tool calls, assemble prompts
- **Public API:** `AgentExecutor`, `ToolExecutor`, `ExecutionResult`, `ExecutorOptions`
- **Dependencies:** `tools`, `providers`, `hooks`
- **Consolidation:** Merges executor.rs + executor_core.rs + request.rs
  - `RequestHandler` is just a helper for `AgentExecutor`
  - `ToolExecutor` is used by `AgentExecutor`
  - All three deal with executing LLM requests and tool calls

### `agent/memory/` - Memory management
- **Responsibility:** Three-phase memory loading, context compression, storage
- **Public API:** `MemoryManager`, `MemoryStore`, `ContextCompactor`
- **Dependencies:** `storage`, `search`
- **Consolidation:** Merges memory.rs + memory_provider.rs
  - Both are tiny wrappers (~50 lines each)
  - No reason to separate

### `agent/history/` - History & indexing
- **Responsibility:** History retrieval, semantic indexing, context building
- **Public API:** `HistoryCoordinator`, `IndexingService`, `ContextBuilder`
- **Dependencies:** `storage`, `memory`
- **Consolidation:** Merges indexing.rs + indexing_queue.rs
  - `IndexingQueue` is an internal implementation detail of `IndexingService`
  - Keep them together for cohesion

### `agent/streaming/` - Streaming output
- **Responsibility:** Stream events, buffering, SSE formatting
- **Public API:** `StreamEvent`, `BufferedEvents`
- **Dependencies:** None (pure data structures)
- **Consolidation:** Merges stream.rs + stream_buffer.rs
  - `BufferedEvents` is only used by streaming logic
  - No reason to separate

### `agent/subagents/` - Subagent orchestration
- **Responsibility:** Spawn and track subagent tasks, model resolution
- **Public API:** `SubagentManager`, `SubagentTracker`, `ModelResolver`, `run_subagent`
- **Dependencies:** `execution`, `streaming`
- **Extraction:** Splits subagent.rs into manager.rs + runner.rs
  - Extract pure function `run_subagent` and `ModelResolver` trait
  - Keep complex `SubagentManager` in its own file

## File Consolidation Details

### Consolidation 1: Execution Layer
**Target:** `execution/executor.rs` (merged from 3 files)

**Sources:**
- `executor.rs` (224 lines) → `ToolExecutor` struct
- `executor_core.rs` (651 lines) → `AgentExecutor` + `ExecutionResult` + `ExecutorOptions`
- `request.rs` (151 lines) → `RequestHandler` (inline into `AgentExecutor`)

**Rationale:** These three files all deal with executing LLM requests and tool calls. `RequestHandler` is just a helper for `AgentExecutor`, and `ToolExecutor` is used by `AgentExecutor`. Merging eliminates artificial boundaries.

### Consolidation 2: Streaming Layer
**Target:** `streaming/stream.rs` (merged from 2 files)

**Sources:**
- `stream.rs` (424 lines) → `StreamEvent` enum + streaming logic
- `stream_buffer.rs` (113 lines) → `BufferedEvents` (inline as helper struct)

**Rationale:** `BufferedEvents` is only used by streaming logic. No reason to separate.

### Consolidation 3: History/Indexing Layer
**Target:** `history/indexing.rs` (merged from 2 files)

**Sources:**
- `indexing.rs` (455 lines) → `IndexingService`
- `indexing_queue.rs` (150 lines) → `IndexingQueue` + `Priority` enum

**Rationale:** `IndexingQueue` is an internal implementation detail of `IndexingService`. Keep them together.

### Consolidation 4: Memory Layer
**Target:** `memory/store.rs` (merged from 2 files)

**Sources:**
- `memory.rs` (50 lines) → `MemoryStore`
- `memory_provider.rs` (49 lines) → Inline into `MemoryStore`

**Rationale:** Both are tiny wrappers. Merge them into one file.

### Consolidation 5: Subagent Layer
**Target:** `subagents/runner.rs` (extracted from subagent.rs)

**Sources:**
- `subagent.rs` (1090 lines) → Split into:
  - `manager.rs` → `SubagentManager` + `SubagentTaskBuilder` + `SessionKeyGuard`
  - `runner.rs` → `run_subagent()` function + `ModelResolver` trait

**Rationale:** Extract the pure function `run_subagent` and `ModelResolver` trait into a separate file. Keep the complex `SubagentManager` in its own file.

### Consolidation 6: Core Config Extraction
**Target:** `core/config.rs` (extracted from loop_.rs)

**Sources:**
- `loop_.rs` (1007 lines) → Extract:
  - `AgentConfig` struct and impl
  - Default constants (DEFAULT_MODEL, DEFAULT_MAX_ITERATIONS, etc.)

**Rationale:** Separate configuration from execution logic. Makes `loop_.rs` focus purely on the agent loop execution flow.

### Files to Keep As-Is
- `core/loop_.rs` (1007 lines) - Main agent loop, already well-structured
- `core/context.rs` (684 lines) - AgentContext enum, clear responsibility
- `memory/compactor.rs` (382 lines) - Summarization logic, distinct concern
- `memory/manager.rs` (1438 lines) - Three-phase loading, complex but cohesive
- `history/coordinator.rs` (185 lines) - History coordination, clear boundary
- `history/builder.rs` (328 lines, renamed from context_builder.rs) - Context building
- `subagents/tracker.rs` (278 lines) - Subagent tracking
- `execution/prompt.rs` (153 lines) - Prompt assembly

### Files to Delete
- `skill_loader.rs` (107 lines) - Move `load_skills()` and `find_builtin_skills_dir()` utility functions into `core/mod.rs`. These are simple skill discovery functions used during agent initialization, safe to inline as they have no complex dependencies.

## Import Cleanup

### Before (scattered imports)
```rust
use crate::agent::executor::ToolExecutor;
use crate::agent::executor_core::AgentExecutor;
use crate::agent::memory::MemoryStore;
use crate::agent::memory_manager::MemoryManager;
```

### After (organized by subsystem)
```rust
use crate::agent::execution::{AgentExecutor, ToolExecutor};
use crate::agent::memory::{MemoryManager, MemoryStore};
```

### Public re-exports in `agent/mod.rs`
```rust
// Core
pub use core::{AgentLoop, AgentConfig, AgentContext, PersistentContext};

// Execution
pub use execution::{AgentExecutor, ToolExecutor, ExecutionResult, ExecutorOptions};

// Memory
pub use memory::{MemoryManager, MemoryStore, ContextCompactor};

// History
pub use history::{HistoryCoordinator, IndexingService, ContextBuilder};

// Streaming
pub use streaming::{StreamEvent, BufferedEvents};

// Subagents
pub use subagents::{SubagentManager, SubagentTracker, ModelResolver, run_subagent};

// Re-export from storage for convenience
pub use gasket_storage::{
    count_tokens, process_history, HistoryConfig, HistoryQuery, 
    HistoryQueryBuilder, HistoryResult, HistoryRetriever, 
    ProcessedHistory, QueryOrder, ResultMeta, SemanticQuery, TimeRange,
};
```

## Implementation Steps

### Step 1: Vault Migration
1. Create `engine/src/vault/core/` directory
2. Move all files from `vault/src/` to `engine/src/vault/core/`
3. Update `engine/src/vault/mod.rs` to re-export from `core/`
4. Add crypto dependencies to `engine/Cargo.toml`
5. Update imports in `engine/src/`: `gasket_vault::` → `crate::vault::core::`
6. Update `engine/src/lib.rs` vault re-exports
7. Delete `vault/` directory
8. Remove `vault` from workspace members in root `Cargo.toml`
9. Run `cargo build --workspace` to verify

### Step 2: Bus Migration
1. Move `bus/src/actors.rs` to `engine/src/bus/actors.rs`
2. Move `bus/src/queue.rs` to `engine/src/bus/queue.rs`
3. Update `engine/src/bus/mod.rs` (remove facade, make it real)
4. Update imports in `engine/src/`: `gasket_bus::` → `crate::bus::`
5. Update `engine/src/lib.rs` bus re-exports
6. Delete `bus/` directory
7. Remove `bus` from workspace members in root `Cargo.toml`
8. Run `cargo build --workspace` to verify

### Step 3: Agent Refactoring - Phase 1 (Create Structure)
1. Create new directory structure:
   ```bash
   mkdir -p engine/src/agent/{core,execution,memory,history,streaming,subagents}
   ```
2. Create all `mod.rs` files with placeholder content
3. Run `cargo build` to verify structure compiles

### Step 4: Agent Refactoring - Phase 2 (Move Files)
1. Move files to new locations:
   - `loop_.rs` → `core/loop_.rs`
   - `context.rs` → `core/context.rs`
   - `executor.rs` → `execution/executor.rs` (will merge later)
   - `executor_core.rs` → `execution/executor_core.rs` (will merge later)
   - `request.rs` → `execution/request.rs` (will merge later)
   - `prompt.rs` → `execution/prompt.rs`
   - `memory_manager.rs` → `memory/manager.rs`
   - `memory.rs` → `memory/store.rs` (will merge later)
   - `memory_provider.rs` → `memory/provider.rs` (will merge later)
   - `compactor.rs` → `memory/compactor.rs`
   - `history_coordinator.rs` → `history/coordinator.rs`
   - `indexing.rs` → `history/indexing.rs` (will merge later)
   - `indexing_queue.rs` → `history/queue.rs` (will merge later)
   - `context_builder.rs` → `history/builder.rs`
   - `stream.rs` → `streaming/stream.rs` (will merge later)
   - `stream_buffer.rs` → `streaming/buffer.rs` (will merge later)
   - `subagent.rs` → `subagents/manager.rs` (will split later)
   - `subagent_tracker.rs` → `subagents/tracker.rs`
2. Update internal imports within each file
3. Run `cargo build` to verify

### Step 5: Agent Refactoring - Phase 3 (Merge Files)
1. Merge `execution/executor.rs` + `execution/executor_core.rs` + `execution/request.rs`
2. Merge `streaming/stream.rs` + `streaming/buffer.rs`
3. Merge `history/indexing.rs` + `history/queue.rs`
4. Merge `memory/store.rs` + `memory/provider.rs`
5. Extract `subagents/runner.rs` from `subagents/manager.rs`
6. Move `skill_loader.rs` content into `core/mod.rs`
7. Extract `AgentConfig` from `core/loop_.rs` into `core/config.rs`
8. Update all internal imports
9. Run `cargo build` to verify

### Step 6: Agent Refactoring - Phase 4 (Update Public API)
1. Update `agent/mod.rs` with new re-exports
2. Update imports in other engine modules:
   - `tools/`
   - `hooks/`
   - `bus_adapter.rs`
   - `cron.rs`
3. Update `engine/src/lib.rs` agent re-exports
4. Run `cargo build --workspace` to verify

### Step 7: Verification
1. Run `cargo build --workspace --release`
2. Run `cargo test --workspace`
3. Run `cargo clippy --workspace -- -D warnings`
4. Test CLI: `cargo run --package gasket-cli -- agent -m "test message"`
5. Test gateway: `cargo run --package gasket-cli -- gateway`

## Breaking Changes & Compatibility

### External API Changes

Most external code uses `gasket_engine::` which re-exports everything, so impact is minimal.

**Potential breakage:**
```rust
// OLD (will break)
use gasket_vault::VaultStore;
use gasket_bus::MessageBus;

// NEW (fix)
use gasket_engine::vault::VaultStore;
use gasket_engine::bus::MessageBus;
```

### Internal API Changes

**Agent module imports:**
```rust
// OLD
use crate::agent::executor::ToolExecutor;
use crate::agent::executor_core::AgentExecutor;
use crate::agent::memory_manager::MemoryManager;

// NEW
use crate::agent::execution::{AgentExecutor, ToolExecutor};
use crate::agent::memory::MemoryManager;
```

### Compatibility Strategy

1. **CLI impact:** None - CLI uses `gasket_engine::` which maintains all re-exports
2. **Channels/Providers impact:** None - they use `gasket_engine::` or `gasket_types::`
3. **External crates:** Minimal - only if they directly imported `gasket_vault` or `gasket_bus`

## Benefits

### Performance
- **Reduced cross-crate boundaries:** Vault and bus are now internal modules, eliminating crate boundary overhead
- **Better inlining opportunities:** Compiler can inline across module boundaries more aggressively
- **Reduced compilation units:** Fewer crates means faster incremental builds

### Maintainability
- **Clearer module boundaries:** Each subsystem has a clear responsibility
- **Reduced file count:** 20 → 16 files total (10 implementation + 6 mod.rs) makes navigation easier
- **Better code organization:** Related functionality is grouped together
- **Easier to understand:** Clear hierarchy shows system architecture

### Developer Experience
- **Simpler workspace:** Fewer crates to manage
- **Faster builds:** Fewer compilation units
- **Better IDE support:** Clearer module structure improves autocomplete and navigation
- **Easier refactoring:** Related code is co-located

## Risks & Mitigations

### Risk 1: Breaking External Code
**Mitigation:** Maintain re-exports in `gasket_engine::` for all public APIs

### Risk 2: Merge Conflicts
**Mitigation:** Do this refactoring in a single PR, coordinate with team

### Risk 3: Test Failures
**Mitigation:** Run full test suite after each step, fix issues incrementally

### Risk 4: Large PR Difficult to Review
**Mitigation:** 
- Use git's `--move` detection to show file moves clearly
- Separate commits for each step (vault, bus, agent structure, agent merges)
- Provide detailed commit messages explaining each change

## Success Criteria

1. ✅ All tests pass: `cargo test --workspace`
2. ✅ No clippy warnings: `cargo clippy --workspace -- -D warnings`
3. ✅ CLI works: Can run agent and gateway commands
4. ✅ File count reduced: Agent module has 16 files (10 implementation + 6 mod.rs) instead of 20
5. ✅ Workspace simplified: 2 fewer crates (vault, bus removed)
6. ✅ Build time improved: Faster incremental builds
7. ✅ Clear module boundaries: Each subsystem has distinct responsibility

## Future Improvements

After this refactoring, consider:

1. **Further consolidation:** If any modules are still too fragmented
2. **Performance profiling:** Measure actual performance improvements
3. **Documentation:** Update architecture docs to reflect new structure
4. **Testing:** Add integration tests for each subsystem
5. **Benchmarking:** Compare build times before/after

## Why This Approach (Approach 2)

We chose the "Big Bang Refactor" approach because:

1. **Vault/Bus are small and isolated:** Low risk to move them in one go
2. **Agent module needs holistic redesign:** Incremental changes would create temporary inconsistencies
3. **Clear vision:** We know exactly what the target structure should be
4. **Faster overall:** One big change is faster than multiple small PRs
5. **Clean result:** No temporary cruft or half-migrated state
6. **Easier to review:** All changes in context, clear before/after comparison

The risk is mitigated by:
- Comprehensive testing at each step
- Clear rollback plan (revert single commit)
- Detailed implementation steps
- Strong type system catches most errors at compile time
