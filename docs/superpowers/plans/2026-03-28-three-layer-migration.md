# Three-Layer Architecture Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transform gasket-core into a pure Facade layer by migrating all business logic to gasket-engine, eliminating circular dependencies.

**Architecture:** Three-layer separation with gasket-types at the base, gasket-bus/history/engine in the middle, and gasket-core as a thin re-export facade.

**Tech Stack:** Rust 2021, tokio, thiserror, tiktoken-rs

**Spec:** `docs/superpowers/specs/2026-03-27-three-layer-migration-design.md`

---

## File Structure

### Files to Create (PR1)
- `gasket/engine/src/error.rs` - Error types (AgentError, ProviderError, etc.)
- `gasket/engine/src/token_tracker.rs` - Token usage tracking
- `gasket/engine/src/config/mod.rs` - Config module entry
- `gasket/engine/src/config/tools.rs` - ExecToolConfig and related types

### Files to Create (PR2)
- `gasket/engine/src/hooks/mod.rs` - Hook registry entry
- `gasket/engine/src/hooks/external.rs` - External hook runner
- `gasket/engine/src/hooks/history.rs` - History recall hook
- `gasket/engine/src/hooks/registry.rs` - Hook registry implementation
- `gasket/engine/src/hooks/types.rs` - Hook types (HookContext, HookPoint, etc.)
- `gasket/engine/src/hooks/vault.rs` - Vault hook
- `gasket/engine/src/skills/mod.rs` - Skills module entry
- `gasket/engine/src/skills/loader.rs` - Skill file loader
- `gasket/engine/src/skills/metadata.rs` - Skill metadata
- `gasket/engine/src/skills/registry.rs` - Skills registry
- `gasket/engine/src/skills/skill.rs` - Skill struct
- `gasket/engine/src/cron/mod.rs` - Cron module entry
- `gasket/engine/src/cron/service.rs` - Cron service implementation

### Files to Modify (PR1)
- `gasket/engine/src/lib.rs` - Add new module exports
- `gasket/engine/src/agent/context.rs` - Update imports
- `gasket/engine/src/agent/pipeline.rs` - Update imports
- `gasket/engine/src/agent/loop_.rs` - Update imports
- `gasket/engine/src/agent/executor_core.rs` - Update imports
- `gasket/engine/src/agent/summarization.rs` - Update imports
- `gasket/engine/src/agent/memory.rs` - Update imports
- `gasket/engine/src/tools/shell.rs` - Update imports
- `gasket/engine/src/tools/registry.rs` - Update imports
- `gasket/engine/src/tools/history_search.rs` - Update imports
- `gasket/core/src/error.rs` - Convert to re-export
- `gasket/core/src/token_tracker.rs` - Convert to re-export

### Files to Modify (PR2)
- `gasket/engine/src/agent/loop_.rs` - Update hooks/vault imports
- `gasket/engine/src/agent/subagent.rs` - Update hooks imports
- `gasket/engine/src/agent/skill_loader.rs` - Update skills imports
- `gasket/engine/src/tools/cron.rs` - Update cron imports
- `gasket/engine/Cargo.toml` - Remove gasket-core dependency
- `gasket/core/src/hooks/mod.rs` - Convert to re-export
- `gasket/core/src/skills/mod.rs` - Convert to re-export
- `gasket/core/src/cron/mod.rs` - Convert to re-export

### Files to Delete (PR3)
- `gasket/core/src/agent/` - Moved to engine
- `gasket/core/src/tools/` - Moved to engine
- `gasket/core/src/hooks/` - Moved to engine
- `gasket/core/src/skills/` - Moved to engine
- `gasket/core/src/cron/` - Moved to engine
- `gasket/core/src/vault/` - Moved to engine
- `gasket/core/src/error.rs` - Moved to engine
- `gasket/core/src/token_tracker.rs` - Moved to engine
- `gasket/core/src/config/` - Moved to engine
- `gasket/core/src/memory/` - Already re-export
- `gasket/core/src/search/` - Already re-export
- `gasket/core/src/channels/` - Already re-export
- `gasket/core/src/providers/` - Already re-export
- `gasket/core/src/heartbeat/` - Evaluate if needed

---

## Pre-Migration Baseline

- [ ] **Step 1: Capture current state**

```bash
# Record current dependency count
cargo tree -p gasket-engine 2>/dev/null | head -50 > /tmp/pre-migration-deps.txt

# Record current import count (use gasket_core::)
grep -r "use gasket_core::" gasket/engine/src/ | wc -l
# Expected: 16

# Record current pub mod count in gasket-core
grep -c "pub mod" gasket/core/src/lib.rs
# Expected: ~15

# Verify current build works
cargo build --workspace && cargo test --workspace
```

---

## PR1: Core Dependency Layer

### Task 1.1: Migrate error.rs to gasket-engine

**Files:**
- Create: `gasket/engine/src/error.rs`
- Modify: `gasket/engine/src/lib.rs`
- Modify: `gasket/core/src/error.rs`

- [ ] **Step 1: Copy error.rs to gasket-engine**

```bash
cp gasket/core/src/error.rs gasket/engine/src/error.rs
```

- [ ] **Step 2: Update gasket-engine/src/lib.rs to export error module**

```rust
//! Core execution engine for gasket AI assistant

pub mod agent;
pub mod tools;
pub mod bus_adapter;
pub mod error;

pub use agent::*;
pub use tools::*;
pub use bus_adapter::*;
pub use error::*;
```

- [ ] **Step 3: Convert gasket-core/src/error.rs to re-export**

```rust
//! Error types re-exported from gasket-engine
pub use gasket_engine::error::*;
```

- [ ] **Step 4: Update gasket-engine imports to use local error module**

In `gasket/engine/src/agent/context.rs`, change:
```rust
// Before
use gasket_core::error::AgentError;

// After
use crate::error::AgentError;
```

In `gasket/engine/src/agent/pipeline.rs`, change:
```rust
// Before
use gasket_core::error::AgentError;

// After
use crate::error::AgentError;
```

In `gasket/engine/src/agent/loop_.rs`, change:
```rust
// Before
use gasket_core::error::AgentError;

// After
use crate::error::AgentError;
```

In `gasket/engine/src/agent/executor_core.rs`, change:
```rust
// Before
use gasket_core::error::AgentError;

// After
use crate::error::AgentError;
```

- [ ] **Step 5: Run build to verify**

```bash
cargo build --workspace
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/error.rs gasket/engine/src/lib.rs gasket/core/src/error.rs \
        gasket/engine/src/agent/context.rs gasket/engine/src/agent/pipeline.rs \
        gasket/engine/src/agent/loop_.rs gasket/engine/src/agent/executor_core.rs
git commit -m "refactor(engine): migrate error module from core to engine

- Copy error.rs to gasket-engine
- Update engine lib.rs to export error module
- Convert core/error.rs to re-export from engine
- Update engine agent files to use local error module

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 1.2: Migrate token_tracker.rs to gasket-engine

**Files:**
- Create: `gasket/engine/src/token_tracker.rs`
- Modify: `gasket/engine/src/lib.rs`
- Modify: `gasket/core/src/token_tracker.rs`

- [ ] **Step 1: Copy token_tracker.rs to gasket-engine**

```bash
cp gasket/core/src/token_tracker.rs gasket/engine/src/token_tracker.rs
```

- [ ] **Step 2: Update gasket-engine/src/lib.rs**

```rust
//! Core execution engine for gasket AI assistant

pub mod agent;
pub mod tools;
pub mod bus_adapter;
pub mod error;
pub mod token_tracker;

pub use agent::*;
pub use tools::*;
pub use bus_adapter::*;
pub use error::*;
pub use token_tracker::*;
```

- [ ] **Step 3: Convert gasket-core/src/token_tracker.rs to re-export**

```rust
//! Token tracking re-exported from gasket-engine
pub use gasket_engine::token_tracker::*;
```

- [ ] **Step 4: Update gasket-engine/src/agent/executor_core.rs**

```rust
// Before
use gasket_core::token_tracker::{ModelPricing, TokenUsage};

// After
use crate::token_tracker::{ModelPricing, TokenUsage};
```

- [ ] **Step 5: Run build to verify**

```bash
cargo build --workspace
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/token_tracker.rs gasket/engine/src/lib.rs \
        gasket/core/src/token_tracker.rs gasket/engine/src/agent/executor_core.rs
git commit -m "refactor(engine): migrate token_tracker module from core to engine

- Copy token_tracker.rs to gasket-engine
- Update engine lib.rs to export token_tracker module
- Convert core/token_tracker.rs to re-export from engine
- Update executor_core.rs to use local token_tracker module

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 1.3: Migrate config/tools.rs (ExecToolConfig) to gasket-engine

**Files:**
- Create: `gasket/engine/src/config/mod.rs`
- Create: `gasket/engine/src/config/tools.rs`
- Modify: `gasket/engine/src/lib.rs`
- Modify: `gasket/engine/src/tools/shell.rs`

- [ ] **Step 1: Create config directory**

```bash
mkdir -p gasket/engine/src/config
```

- [ ] **Step 2: Copy entire config/tools.rs file**

```bash
cp gasket/core/src/config/tools.rs gasket/engine/src/config/tools.rs
```

The file contains all necessary types:
- `ExecToolConfig` - Shell tool configuration
- `CommandPolicyConfig` - Command allowlist/denylist
- `ResourceLimitsConfig` - CPU/memory limits
- `ToolsConfig` - Container for all tool configs
- `WebToolsConfig` - Web fetch/search config
- `SandboxConfig` - Sandbox configuration

- [ ] **Step 3: Create gasket/engine/src/config/mod.rs**

```rust
//! Configuration types for gasket-engine

mod tools;

pub use tools::{
    CommandPolicyConfig, ExecToolConfig, ResourceLimitsConfig, SandboxConfig, ToolsConfig,
    WebToolsConfig,
};
```

- [ ] **Step 4: Update gasket-engine/src/lib.rs**

```rust
//! Core execution engine for gasket AI assistant

pub mod agent;
pub mod tools;
pub mod bus_adapter;
pub mod error;
pub mod token_tracker;
pub mod config;

pub use agent::*;
pub use tools::*;
pub use bus_adapter::*;
pub use error::*;
pub use token_tracker::*;
pub use config::*;
```

- [ ] **Step 5: Update gasket-engine/src/tools/shell.rs**

```rust
// Before
use gasket_core::config::ExecToolConfig;

// After
use crate::config::ExecToolConfig;
```

- [ ] **Step 6: Run build to verify**

```bash
cargo build --workspace
```

Expected: PASS (serde and other dependencies already in Cargo.toml)

- [ ] **Step 7: Commit**

```bash
git add gasket/engine/src/config/ gasket/engine/src/lib.rs gasket/engine/src/tools/shell.rs
git commit -m "refactor(engine): migrate config/tools (ExecToolConfig) from core to engine

- Create config module in gasket-engine
- Copy ExecToolConfig and related types
- Update shell.rs to use local config module

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 1.4: Update memory/search imports in gasket-engine

**Files:**
- Modify: `gasket/engine/src/agent/summarization.rs`
- Modify: `gasket/engine/src/agent/memory.rs`
- Modify: `gasket/engine/src/tools/registry.rs`
- Modify: `gasket/engine/src/tools/history_search.rs`

- [ ] **Step 1: Update gasket-engine/src/agent/summarization.rs**

```rust
// Before
use gasket_core::memory::SqliteStore;
use gasket_core::search::{top_k_similar, TextEmbedder};

// After
use gasket_storage::SqliteStore;
use gasket_semantic::{top_k_similar, TextEmbedder};
```

- [ ] **Step 2: Update gasket-engine/src/agent/memory.rs**

```rust
// Before
use gasket_core::memory::SqliteStore;

// After
use gasket_storage::SqliteStore;
```

- [ ] **Step 3: Update gasket/engine/src/tools/registry.rs**

```rust
// Before
use gasket_core::search::{top_k_similar, TextEmbedder};

// After
use gasket_semantic::{top_k_similar, TextEmbedder};
```

- [ ] **Step 4: Update gasket/engine/src/tools/history_search.rs**

```rust
// Before
use gasket_core::memory::SqliteStore;

// After
use gasket_storage::SqliteStore;
```

- [ ] **Step 5: Run build to verify**

```bash
cargo build --workspace
```

Expected: PASS

- [ ] **Step 6: Verify import count reduced**

```bash
grep -r "use gasket_core::" gasket/engine/src/ | wc -l
# Expected: ~8 (hooks: 2 files, skills: 1, cron: 1, vault: 1)
# Note: loop_.rs has multiple gasket_core imports on separate lines
# Remaining imports:
#   - gasket_core::hooks::* (loop_.rs, subagent.rs)
#   - gasket_core::skills::* (skill_loader.rs)
#   - gasket_core::cron::CronService (tools/cron.rs)
#   - gasket_core::vault::* (loop_.rs)
```

- [ ] **Step 7: Commit**

```bash
git add gasket/engine/src/agent/summarization.rs gasket/engine/src/agent/memory.rs \
        gasket/engine/src/tools/registry.rs gasket/engine/src/tools/history_search.rs
git commit -m "refactor(engine): use gasket-storage/semantic directly instead of via core

- Update summarization.rs to use gasket-storage and gasket-semantic
- Update memory.rs to use gasket-storage
- Update registry.rs to use gasket-semantic
- Update history_search.rs to use gasket-storage

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 1.5: PR1 Final Verification

- [ ] **Step 1: Run full build**

```bash
cargo build --workspace
```

- [ ] **Step 2: Run all tests**

```bash
cargo test --workspace
```

- [ ] **Step 3: Verify dependency reduction**

```bash
cargo tree -p gasket-engine --invert | grep gasket-core
# Should still show gasket-core (we haven't removed the dependency yet)
```

- [ ] **Step 4: Verify import count**

```bash
grep -r "use gasket_core::" gasket/engine/src/ | wc -l
# Expected: ~8 (hooks, skills, cron, vault imports remain)
```

---

## PR2: Extended Functionality Layer

### Task 2.1: Migrate hooks module to gasket-engine

**Files:**
- Create: `gasket/engine/src/hooks/mod.rs`
- Create: `gasket/engine/src/hooks/external.rs`
- Create: `gasket/engine/src/hooks/history.rs`
- Create: `gasket/engine/src/hooks/registry.rs`
- Create: `gasket/engine/src/hooks/types.rs`
- Create: `gasket/engine/src/hooks/vault.rs`
- Modify: `gasket/engine/src/lib.rs`
- Modify: `gasket/core/src/hooks/mod.rs`

- [ ] **Step 1: Remove empty hooks directory and copy full module**

```bash
rm -rf gasket/engine/src/hooks
cp -r gasket/core/src/hooks gasket/engine/src/hooks
```

- [ ] **Step 2: Update gasket-engine/src/lib.rs**

Add `pub mod hooks;` and `pub use hooks::*;`

- [ ] **Step 3: Convert gasket-core/src/hooks/mod.rs to re-export**

```rust
//! Hooks re-exported from gasket-engine
pub use gasket_engine::hooks::*;
```

- [ ] **Step 4: Update gasket-engine imports**

In `gasket/engine/src/agent/loop_.rs`:
```rust
// Before
use gasket_core::hooks::{HookRegistry, HookContext, HookPoint, PipelineHook};

// After
use crate::hooks::{HookRegistry, HookContext, HookPoint, PipelineHook};
```

In `gasket/engine/src/agent/subagent.rs`:
```rust
// Before
use gasket_core::hooks::HookRegistry;

// After
use crate::hooks::HookRegistry;
```

- [ ] **Step 5: Run build to verify**

```bash
cargo build --workspace
```

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/hooks/ gasket/engine/src/lib.rs gasket/core/src/hooks/mod.rs \
        gasket/engine/src/agent/loop_.rs gasket/engine/src/agent/subagent.rs
git commit -m "refactor(engine): migrate hooks module from core to engine

- Copy hooks module to gasket-engine
- Convert core/hooks to re-export
- Update engine imports to use local hooks module

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 2.2: Migrate skills module to gasket-engine

**Files:**
- Create: `gasket/engine/src/skills/mod.rs`
- Create: `gasket/engine/src/skills/loader.rs`
- Create: `gasket/engine/src/skills/metadata.rs`
- Create: `gasket/engine/src/skills/registry.rs`
- Create: `gasket/engine/src/skills/skill.rs`
- Modify: `gasket/engine/src/lib.rs`
- Modify: `gasket/core/src/skills/mod.rs`

- [ ] **Step 1: Remove empty skills directory and copy full module**

```bash
rm -rf gasket/engine/src/skills
cp -r gasket/core/src/skills gasket/engine/src/skills
```

- [ ] **Step 2: Update gasket-engine/src/lib.rs**

Add `pub mod skills;` and `pub use skills::*;`

- [ ] **Step 3: Convert gasket-core/src/skills/mod.rs to re-export**

```rust
//! Skills re-exported from gasket-engine
pub use gasket_engine::skills::*;
```

- [ ] **Step 4: Update gasket-engine imports**

In `gasket/engine/src/agent/skill_loader.rs`:
```rust
// Before
use gasket_core::skills::{SkillsLoader, SkillsRegistry};

// After
use crate::skills::{SkillsLoader, SkillsRegistry};
```

- [ ] **Step 5: Run build to verify**

```bash
cargo build --workspace
```

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/skills/ gasket/engine/src/lib.rs gasket/core/src/skills/mod.rs \
        gasket/engine/src/agent/skill_loader.rs
git commit -m "refactor(engine): migrate skills module from core to engine

- Copy skills module to gasket-engine
- Convert core/skills to re-export
- Update engine imports to use local skills module

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 2.3: Migrate cron module to gasket-engine

**Files:**
- Create: `gasket/engine/src/cron/mod.rs`
- Create: `gasket/engine/src/cron/service.rs`
- Modify: `gasket/engine/src/lib.rs`
- Modify: `gasket/core/src/cron/mod.rs`

- [ ] **Step 1: Remove empty cron directory and copy full module**

```bash
rm -rf gasket/engine/src/cron
cp -r gasket/core/src/cron gasket/engine/src/cron
```

- [ ] **Step 2: Update gasket-engine/src/lib.rs**

Add `pub mod cron;` and `pub use cron::*;`

- [ ] **Step 3: Convert gasket-core/src/cron/mod.rs to re-export**

```rust
//! Cron re-exported from gasket-engine
pub use gasket_engine::cron::*;
```

- [ ] **Step 4: Update gasket-engine imports**

In `gasket/engine/src/tools/cron.rs`:
```rust
// Before
use gasket_core::cron::CronService;

// After
use crate::cron::CronService;
```

- [ ] **Step 5: Run build to verify**

```bash
cargo build --workspace
```

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/src/cron/ gasket/engine/src/lib.rs gasket/core/src/cron/mod.rs \
        gasket/engine/src/tools/cron.rs
git commit -m "refactor(engine): migrate cron module from core to engine

- Copy cron module to gasket-engine
- Convert core/cron to re-export
- Update engine imports to use local cron module

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 2.4: Migrate vault module to gasket-engine

**Files:**
- Create: `gasket/engine/src/vault/mod.rs`
- Create: `gasket/engine/src/vault/injector.rs`
- Modify: `gasket/engine/src/lib.rs`
- Modify: `gasket/core/src/vault/mod.rs`

**Note:** The vault module contains 2 files:
- `mod.rs` - Module exports
- `injector.rs` - VaultInjector and related types

- [ ] **Step 1: Check what vault types are used in engine**

```bash
grep -r "gasket_core::vault" gasket/engine/src/
```

- [ ] **Step 2: Copy vault module**

```bash
rm -rf gasket/engine/src/vault 2>/dev/null
cp -r gasket/core/src/vault gasket/engine/src/vault
```

- [ ] **Step 3: Update gasket-engine/src/lib.rs**

Add `pub mod vault;` and `pub use vault::*;`

- [ ] **Step 4: Convert gasket-core/src/vault/mod.rs to re-export**

```rust
//! Vault re-exported from gasket-engine
pub use gasket_engine::vault::*;
```

- [ ] **Step 5: Update gasket-engine imports**

In `gasket/engine/src/agent/loop_.rs`:
```rust
// Before
use gasket_core::vault::{redact_secrets, VaultInjector, VaultStore};

// After
use crate::vault::{redact_secrets, VaultInjector, VaultStore};
```

- [ ] **Step 6: Run build to verify**

```bash
cargo build --workspace
```

- [ ] **Step 7: Commit**

```bash
git add gasket/engine/src/vault/ gasket/engine/src/lib.rs gasket/core/src/vault/mod.rs \
        gasket/engine/src/agent/loop_.rs
git commit -m "refactor(engine): migrate vault module from core to engine

- Copy vault module to gasket-engine
- Convert core/vault to re-export
- Update engine imports to use local vault module

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 2.5: Remove gasket-core dependency from gasket-engine

**Files:**
- Modify: `gasket/engine/Cargo.toml`

- [ ] **Step 1: Verify all imports are updated**

```bash
grep -r "use gasket_core::" gasket/engine/src/
# Expected: empty output
```

If any remain, update them before proceeding.

- [ ] **Step 2: Remove gasket-core from Cargo.toml**

In `gasket/engine/Cargo.toml`, remove the line:
```toml
gasket-core = { path = "../core" }
```

- [ ] **Step 3: Run build to verify**

```bash
cargo build --workspace
```

Expected: PASS

- [ ] **Step 4: Verify dependency is gone**

```bash
cargo tree -p gasket-engine 2>/dev/null | grep -c gasket-core
# Expected: 0
```

- [ ] **Step 5: Run all tests**

```bash
cargo test --workspace
```

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/Cargo.toml
git commit -m "refactor(engine): remove gasket-core dependency

gasket-engine is now fully independent of gasket-core.
All modules have been migrated and imports updated.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## PR3: Facade Transformation

### Task 3.1: Delete migrated modules from gasket-core

**Files:**
- Delete: `gasket/core/src/agent/`
- Delete: `gasket/core/src/tools/`
- Delete: `gasket/core/src/hooks/`
- Delete: `gasket/core/src/skills/`
- Delete: `gasket/core/src/cron/`
- Delete: `gasket/core/src/vault/`
- Delete: `gasket/core/src/error.rs`
- Delete: `gasket/core/src/token_tracker.rs`
- Delete: `gasket/core/src/config/`
- Delete: `gasket/core/src/memory/`
- Delete: `gasket/core/src/search/`
- Delete: `gasket/core/src/channels/`
- Delete: `gasket/core/src/providers/`

- [ ] **Step 1: Delete migrated directories**

```bash
rm -rf gasket/core/src/agent
rm -rf gasket/core/src/tools
rm -rf gasket/core/src/hooks
rm -rf gasket/core/src/skills
rm -rf gasket/core/src/cron
rm -rf gasket/core/src/vault
rm -rf gasket/core/src/config
rm gasket/core/src/error.rs
rm gasket/core/src/token_tracker.rs
```

- [ ] **Step 2: Delete re-export only directories**

```bash
rm -rf gasket/core/src/memory
rm -rf gasket/core/src/search
rm -rf gasket/core/src/channels
rm -rf gasket/core/src/providers
```

- [ ] **Step 3: Evaluate heartbeat module**

Check if heartbeat is used elsewhere:
```bash
grep -r "heartbeat" gasket/cli/src/ gasket/engine/src/ --include="*.rs"
```

**Decision criteria:**
- **KEEP**: heartbeat is used by gasket-cli (`cli/src/commands/gateway.rs`) for pipeline task health monitoring
- The module should be moved to gasket-engine OR kept as a re-export from gasket-core
- **DO NOT DELETE** - it has active consumers

**Action:** Move heartbeat module to gasket-engine and add re-export in gasket-core.

- [ ] **Step 4: Commit**

```bash
git add -A gasket/core/src/
git commit -m "refactor(core): remove migrated modules

All business logic has been moved to gasket-engine.
gasket-core will become a pure facade in next commit.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 3.2: Convert gasket-core to pure Facade

**Files:**
- Modify: `gasket/core/src/lib.rs`
- Modify: `gasket/core/Cargo.toml`

- [ ] **Step 1: Rewrite gasket-core/src/lib.rs**

```rust
//! gasket-core: Facade for gasket AI assistant framework
//!
//! This crate re-exports all gasket crates for backward compatibility.
//! It provides a single entry point for all gasket functionality.
//!
//! NOTE: This crate contains NO local implementations.
//! All functionality is provided by the underlying crates.

// Core types (canonical source)
pub use gasket_types::*;

// Message bus
pub use gasket_bus::*;

// History processing
pub use gasket_history::*;

// Core engine (agent, tools, hooks, skills, etc.)
pub use gasket_engine::*;

// LLM Providers
pub use gasket_providers::*;

// Communication Channels
pub use gasket_channels::*;

// Supporting crates
pub use gasket_vault::*;
pub use gasket_storage as storage;
pub use gasket_semantic as semantic;
```

- [ ] **Step 2: Update gasket-core/Cargo.toml**

Remove all external dependencies that are no longer needed. Keep only:
```toml
[package]
name = "gasket-core"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
description = "Facade crate for gasket AI assistant (re-exports all gasket crates)"

[features]
default = []
telegram = ["gasket-channels/telegram"]
discord = ["gasket-channels/discord"]
slack = ["gasket-channels/slack"]
email = ["gasket-channels/email"]
dingtalk = ["gasket-channels/dingtalk"]
feishu = ["gasket-channels/feishu"]
wecom = ["gasket-channels/wecom"]
webhook = ["gasket-channels/webhook"]
smart-model-selection = ["gasket-engine/smart-model-selection"]
all-channels = ["gasket-channels/all-channels"]
provider-gemini = ["gasket-providers/provider-gemini"]
provider-copilot = ["gasket-providers/provider-copilot"]
all-providers = ["gasket-providers/all-providers"]

[dependencies]
# Only internal crate dependencies needed for re-exports
gasket-types = { path = "../types" }
gasket-bus = { path = "../bus" }
gasket-history = { path = "../history" }
gasket-engine = { path = "../engine" }
gasket-providers = { path = "../providers" }
gasket-channels = { path = "../channels" }
gasket-vault = { path = "../vault" }
gasket-storage = { path = "../storage" }
gasket-semantic = { path = "../semantic", features = ["local-embedding"] }

[dev-dependencies]
# Minimal dev dependencies for facade testing
tokio-test = "0.4"
```

- [ ] **Step 3: Run build to verify**

```bash
cargo build --workspace
```

- [ ] **Step 4: Run all tests**

```bash
cargo test --workspace
```

- [ ] **Step 5: Verify no local modules**

```bash
grep -c "pub mod" gasket/core/src/lib.rs
# Expected: 0
```

- [ ] **Step 6: Verify CLI still works**

```bash
cargo run --package gasket-cli -- agent -m "hello"
```

- [ ] **Step 7: Commit**

```bash
git add gasket/core/src/lib.rs gasket/core/Cargo.toml
git commit -m "refactor(core): convert to pure facade layer

gasket-core is now ~50 lines of re-exports only.
All business logic resides in gasket-engine.

BREAKING CHANGE: gasket-core no longer contains any implementations

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 3.3: Final Verification

- [ ] **Step 1: Verify clean build**

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
```

- [ ] **Step 2: Verify dependency graph**

```bash
cargo tree -p gasket-core
# Should show: core -> engine -> (bus, history, storage, semantic, etc.)
# Should NOT show: core -> engine -> core (circular)
```

- [ ] **Step 3: Verify no circular dependencies**

```bash
cargo tree -p gasket-engine 2>/dev/null | grep gasket-core
# Expected: empty output
```

- [ ] **Step 4: Verify feature flags**

```bash
cargo build --package gasket-core --features "telegram,discord"
```

- [ ] **Step 5: Create summary commit**

```bash
git add -A
git commit -m "feat: complete three-layer architecture migration

Architecture after migration:
- gasket-types: Shared types (base layer)
- gasket-bus: Message bus (transport layer)
- gasket-history: History processing (storage layer)
- gasket-engine: Core business logic (logic layer)
- gasket-core: Pure facade (re-export layer)

Success criteria verified:
- gasket-engine has zero dependency on gasket-core
- gasket-core has no pub mod declarations
- All tests pass
- No circular dependencies
- CLI works without modification

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Success Criteria Checklist

- [ ] `cargo tree -p gasket-engine` shows no `gasket-core` dependency
- [ ] `grep -c "pub mod" gasket/core/src/lib.rs` returns `0`
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` has no new warnings
- [ ] `cargo run --package gasket-cli -- agent -m "test"` works
- [ ] Feature flags still work correctly
