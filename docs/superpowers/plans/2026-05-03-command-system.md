# Command System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the slash-command dispatcher described in
`docs/superpowers/specs/2026-05-03-command-system-design.md`. The CLI REPL
hands every input through the dispatcher; built-ins handle local concerns,
user YAML files inject prompt templates, and the engine API gains a single
optional `tool_filter` parameter.

**Architecture:** A new `gasket-command` workspace crate holds the
`Dispatcher`, `Command` registry, parser, template renderer, completer, and
six built-ins. It depends only on `gasket-types`. CLI provides a
`CliCommandHost` impl that bridges to `AgentSession`. Bot channels (Telegram,
Discord, Slack) keep their existing passthrough behavior — they never see
this crate.

**Tech Stack:** Rust 2021, tokio, async-trait, serde + serde_yaml,
thiserror, tracing, reedline (CLI), tempfile (tests).

---

## Spec Reference

- Spec: `docs/superpowers/specs/2026-05-03-command-system-design.md`
- Personas / coding standards: `ROLE.md`, project `CLAUDE.md`

## Scope Check

The spec is one focused subsystem (client-side slash dispatcher). It does
not require decomposition into multiple plans. The previously deleted
`flow-command-system-design.md` mixed three concerns; this spec narrowed
scope to just the dispatcher, so a single plan is correct.

## File Structure

| Path | Action | Responsibility |
|---|---|---|
| `gasket/Cargo.toml` | Modify | Add `command` to `workspace.members` |
| `gasket/command/Cargo.toml` | Create | Manifest for the new crate |
| `gasket/command/src/lib.rs` | Create | Re-exports of public types |
| `gasket/command/src/types.rs` | Create | `Command`, `CommandKind`, `CommandResult`, `RouteOutcome`, `BuiltinHandler` |
| `gasket/command/src/error.rs` | Create | `BuildError` enum |
| `gasket/command/src/host.rs` | Create | `CommandHost` trait |
| `gasket/command/src/parser.rs` | Create | `parse` + `ParsedInput` |
| `gasket/command/src/template.rs` | Create | `render` |
| `gasket/command/src/dispatcher.rs` | Create | `Dispatcher`, `DispatcherBuilder` |
| `gasket/command/src/yaml_loader.rs` | Create | `load_user_commands(dir)` |
| `gasket/command/src/completer.rs` | Create | `CommandCompleter` for Reedline |
| `gasket/command/src/builtins/mod.rs` | Create | re-exports |
| `gasket/command/src/builtins/exit.rs` | Create | `/exit` |
| `gasket/command/src/builtins/clear.rs` | Create | `/clear` |
| `gasket/command/src/builtins/help.rs` | Create | `/help` |
| `gasket/command/src/builtins/new.rs` | Create | `/new` |
| `gasket/command/src/builtins/sessions.rs` | Create | `/sessions` |
| `gasket/command/src/builtins/model.rs` | Create | `/model` |
| `gasket/command/tests/end_to_end.rs` | Create | Full build → route flow |
| `gasket/types/src/command.rs` | Create | `SessionSummary`, `ModelSwitchInfo` |
| `gasket/types/src/lib.rs` | Modify | Re-export `command::*` |
| `gasket/engine/src/kernel/context.rs` | Modify | Add `tool_filter: Option<Vec<String>>` to `KernelConfig` |
| `gasket/engine/src/kernel/request_handler.rs` | Modify | Apply filter when assembling chat request |
| `gasket/engine/src/session/mod.rs` | Modify | Extend `process_direct` and `process_direct_streaming_with_channel` |
| `gasket/engine/src/bus_adapter.rs` | Modify | Pass `None` to keep behavior |
| `gasket/cli/src/commands/agent.rs` | Modify | Replace hardcoded slash if/else with dispatcher; pass `None` at non-rewrite call sites |
| `gasket/cli/src/commands/command_host.rs` | Create | `CliCommandHost` impl of `CommandHost` |

## Conventions

- **Commit prefix:** `feat(command)`, `feat(types)`, `feat(engine)`, `feat(cli)`, `test(command)`. Match project pattern (see `git log --oneline`).
- **Run a single test:** `cargo test -p gasket-command --lib parser_test::table -- --nocapture`
- **Run all crate tests:** `cargo test -p gasket-command`
- **Run workspace build:** `cargo build --workspace`
- Each task ends with a commit. Pre-commit hooks run `cargo clippy --fix && cargo fmt && cargo build`. Don't bypass.

---

## Tasks

### Task 1 — Bootstrap `gasket-command` crate

**What:** Create the new workspace member crate with manifest and empty `lib.rs`.

**Why:** Spec §2.1 requires a separate crate so the dispatcher does not depend on `engine`. This task establishes the build target before any code exists, keeping every later task focused on one file.

**Where:**
- Modify: `gasket/Cargo.toml`
- Create: `gasket/command/Cargo.toml`
- Create: `gasket/command/src/lib.rs`

**How:** Add `command` to `workspace.members`. Write a Cargo.toml that uses workspace inheritance for shared deps and pulls in `tokio`, `async-trait`, `serde` (derive), `serde_yaml`, `thiserror`, `tracing`, plus path-dep on `gasket-types`. `lib.rs` is just a doc comment.

**Test Case & Acceptance Criteria:**
- `cargo build -p gasket-command` succeeds.
- `cargo build --workspace` still passes.

- [ ] **Step 1: Add command to workspace members**

Edit `gasket/Cargo.toml`. The `[workspace] members = [...]` array currently has 10 entries. Append `"command"`:

```toml
[workspace]
resolver = "2"
members = [
    "types",
    "storage",
    "embedding",
    "broker",
    "engine",
    "cli",
    "providers",
    "channels",
    "sandbox",
    "wiki",
    "command",
]
```

- [ ] **Step 2: Create the crate manifest**

Write `gasket/command/Cargo.toml`:

```toml
[package]
name = "gasket-command"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Slash-command dispatcher for the gasket CLI and Web frontends"

[dependencies]
async-trait = { workspace = true }
chrono = { workspace = true, features = ["serde"] }
gasket-types = { path = "../types" }
serde = { workspace = true, features = ["derive"] }
serde_yaml = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["fs", "sync", "macros"] }
tracing = { workspace = true }
futures = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }
```

If any of `serde_yaml`, `chrono`, `tempfile`, `futures` is not yet in `[workspace.dependencies]`, add it there with the version already used elsewhere in the workspace (`grep -rn '<crate> = ' gasket/*/Cargo.toml`).

- [ ] **Step 3: Create empty lib.rs**

Write `gasket/command/src/lib.rs`:

```rust
//! Slash-command dispatcher for gasket clients (CLI today, Web tomorrow).
//!
//! This crate intentionally does not depend on `gasket-engine`. Built-in
//! handlers that need engine capabilities receive them through the
//! [`CommandHost`] trait, whose implementation lives in the consuming crate.
```

- [ ] **Step 4: Verify build**

Run:

```bash
cargo build -p gasket-command
cargo build --workspace
```

Both must succeed.

- [ ] **Step 5: Commit**

```bash
git add gasket/Cargo.toml gasket/command/
git commit -m "feat(command): bootstrap gasket-command crate skeleton

Empty workspace member with manifest and lib.rs only. Subsequent tasks
fill in types, dispatcher, and built-in commands.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §2.1

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2 — Add `SessionSummary` and `ModelSwitchInfo` to `gasket-types`

**What:** Define two plain-data types in `gasket-types` so `gasket-command` can name them in its `CommandHost` trait without depending on `engine`.

**Why:** Spec §3.4 states these types belong in `gasket-types` to keep `gasket-command → gasket-types` as the only edge.

**Where:**
- Create: `gasket/types/src/command.rs`
- Modify: `gasket/types/src/lib.rs`

**How:** A small module exporting two structs with `Debug + Clone + Serialize + Deserialize`. Add a unit test that round-trips through serde to verify the derives are correct.

**Test Case & Acceptance Criteria:**
- `cargo test -p gasket-types command::tests::round_trip` passes.
- `cargo build --workspace` passes.

- [ ] **Step 1: Write failing tests**

Create `gasket/types/src/command.rs`:

```rust
//! Shared types used across command-related crates.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::SessionKey;

/// One row in the output of `/sessions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionSummary {
    pub key: SessionKey,
    pub message_count: usize,
    pub last_active: Option<DateTime<Utc>>,
}

/// Result of a successful `/model <id>` switch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelSwitchInfo {
    pub previous: String,
    pub current: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn round_trip_session_summary() {
        let original = SessionSummary {
            key: SessionKey::new(crate::ChannelType::Cli, "interactive"),
            message_count: 42,
            last_active: Some(Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap()),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: SessionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_model_switch_info() {
        let original = ModelSwitchInfo {
            previous: "openai/gpt-4.1".into(),
            current: "openrouter/anthropic/claude-4.5-sonnet".into(),
        };
        let yaml = serde_yaml::to_string(&original).unwrap();
        let decoded: ModelSwitchInfo = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(original, decoded);
    }
}
```

- [ ] **Step 2: Verify the tests fail to compile**

Run:

```bash
cargo test -p gasket-types
```

Expected: compilation fails because `crate::command` is not declared in `lib.rs`.

- [ ] **Step 3: Wire the module into `gasket-types`**

Add the module declaration and re-exports to `gasket/types/src/lib.rs`. Find a sensible spot (alongside other `pub mod`s) and add:

```rust
pub mod command;
pub use command::{ModelSwitchInfo, SessionSummary};
```

If `gasket-types` does not already depend on `chrono` with the `serde` feature, add it; check with `grep chrono gasket/types/Cargo.toml`.

- [ ] **Step 4: Verify the tests pass**

Run:

```bash
cargo test -p gasket-types command
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add gasket/types/
git commit -m "feat(types): add SessionSummary and ModelSwitchInfo

These types are referenced by the gasket-command CommandHost trait. Living
in gasket-types keeps the dispatcher crate from depending on engine.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §3.4

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3 — Define core types in `gasket-command`

**What:** Add `Command`, `CommandKind`, `CommandResult`, `RouteOutcome`, and the `BuiltinHandler` type alias.

**Why:** Spec §3.1 and §3.2. These are the data structures every later task references. No methods yet — pure shapes.

**Where:**
- Create: `gasket/command/src/types.rs`
- Modify: `gasket/command/src/lib.rs`

**How:** One file with the enum/struct definitions. Use `Arc<dyn Fn>` for `BuiltinHandler` exactly as in the spec. No serde derives — these types never cross a wire.

**Test Case & Acceptance Criteria:**
- `cargo build -p gasket-command` succeeds.
- A trivial test that constructs each variant compiles and runs.

- [ ] **Step 1: Write the failing smoke test**

Create `gasket/command/src/types.rs`:

```rust
//! Core data types for the dispatcher.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::host::CommandHost;

/// A registered command, either a built-in Rust handler or a user YAML entry.
pub struct Command {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub kind: CommandKind,
}

pub enum CommandKind {
    Builtin(BuiltinHandler),
    Yaml {
        prompt_template: String,
        allowed_tools: Option<Vec<String>>,
    },
}

pub type BuiltinHandler = Arc<
    dyn for<'a> Fn(&'a str, &'a dyn CommandHost) -> BoxFuture<'a, CommandResult>
        + Send
        + Sync,
>;

/// Top-level result of `Dispatcher::route`.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteOutcome {
    Handled(CommandResult),
    Rewrite {
        prompt: String,
        tool_filter: Option<Vec<String>>,
    },
    Passthrough(String),
}

/// What a built-in handler asks the caller to do after it runs.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandResult {
    Print(String),
    Quit,
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_outcome_variants_construct() {
        let _h = RouteOutcome::Handled(CommandResult::Quit);
        let _r = RouteOutcome::Rewrite {
            prompt: "x".into(),
            tool_filter: None,
        };
        let _p = RouteOutcome::Passthrough("y".into());
    }
}
```

- [ ] **Step 2: Stub `host` module so the import resolves**

Create `gasket/command/src/host.rs` with just the trait declaration so `types.rs` compiles:

```rust
//! Bridge trait between the dispatcher and the host application (CLI / Web).

use async_trait::async_trait;
use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};

#[async_trait]
pub trait CommandHost: Send + Sync {
    async fn clear_session(&self, key: &SessionKey);
    async fn list_sessions(&self) -> Vec<SessionSummary>;
    async fn current_model(&self) -> String;
    async fn switch_model(&self, new: &str) -> Result<ModelSwitchInfo, String>;
}
```

(Task 5 verifies this trait separately; we need it now only because `BuiltinHandler` references `dyn CommandHost`.)

- [ ] **Step 3: Wire modules into `lib.rs`**

Edit `gasket/command/src/lib.rs`:

```rust
//! Slash-command dispatcher for gasket clients (CLI today, Web tomorrow).
//!
//! This crate intentionally does not depend on `gasket-engine`. Built-in
//! handlers that need engine capabilities receive them through the
//! [`CommandHost`] trait, whose implementation lives in the consuming crate.

pub mod host;
pub mod types;

pub use host::CommandHost;
pub use types::{BuiltinHandler, Command, CommandKind, CommandResult, RouteOutcome};
```

- [ ] **Step 4: Verify build and test**

Run:

```bash
cargo test -p gasket-command --lib types::tests::route_outcome_variants_construct
```

Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): define core types

Command, CommandKind, CommandResult, RouteOutcome, BuiltinHandler.
Static enum dispatch — no trait objects on the per-command boundary.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §3.1, §3.2

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4 — `BuildError` enum

**What:** Add the build-time error type with three variants.

**Why:** Spec §8.3. The dispatcher builder is the single `Result` boundary in the crate; all per-file YAML errors are warnings, not variants.

**Where:**
- Create: `gasket/command/src/error.rs`
- Modify: `gasket/command/src/lib.rs`

**How:** thiserror derive, three variants, `#[from] std::io::Error` for the IO case.

**Test Case & Acceptance Criteria:**
- `cargo build -p gasket-command` succeeds.
- A unit test verifies each variant's `Display` output.

- [ ] **Step 1: Write the failing tests**

Create `gasket/command/src/error.rs`:

```rust
//! Build-time errors for the dispatcher builder.

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("CommandHost not set; call .host() before build()")]
    MissingHost,

    #[error("duplicate built-in name: /{0}")]
    DuplicateBuiltin(String),

    #[error("user_dir is set but cannot be read: {0}")]
    UserDirIO(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_host_message() {
        let e = BuildError::MissingHost;
        assert_eq!(
            e.to_string(),
            "CommandHost not set; call .host() before build()"
        );
    }

    #[test]
    fn duplicate_builtin_message() {
        let e = BuildError::DuplicateBuiltin("help".into());
        assert_eq!(e.to_string(), "duplicate built-in name: /help");
    }

    #[test]
    fn from_io_error() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let e: BuildError = io_err.into();
        match e {
            BuildError::UserDirIO(inner) => {
                assert_eq!(inner.kind(), io::ErrorKind::PermissionDenied);
            }
            _ => panic!("unexpected variant"),
        }
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`**

Add to `gasket/command/src/lib.rs`:

```rust
pub mod error;
pub use error::BuildError;
```

- [ ] **Step 3: Verify the tests pass**

```bash
cargo test -p gasket-command --lib error
```

Expected: 3 passed.

- [ ] **Step 4: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): add BuildError enum

Three variants only. Per-file YAML parse failures are warnings, not
build errors, so they do not appear here.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §8.3

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 5 — `CommandHost` trait (verification)

**What:** Confirm the trait stub created in Task 3 has the four methods documented in the spec, and add a small example impl in tests so the trait is exercised by the type system.

**Why:** Spec §3.4 lists four methods. Task 3 stubbed the file so `types.rs` could compile; this task verifies the surface and documents it.

**Where:**
- Modify: `gasket/command/src/host.rs`

**How:** Replace the stub with the documented version, including doc comments per method. Add a `#[cfg(test)]` example impl that uses each method.

**Test Case & Acceptance Criteria:**
- `cargo test -p gasket-command --lib host` passes.

- [ ] **Step 1: Replace `host.rs` with the documented version**

Overwrite `gasket/command/src/host.rs`:

```rust
//! Bridge trait between the dispatcher and the host application.
//!
//! `gasket-command` does not depend on `gasket-engine`. Built-in handlers
//! reach engine capabilities (clear session, list sessions, switch model)
//! through this trait. The CLI and the future Web frontend each provide
//! their own implementation.

use async_trait::async_trait;
use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};

#[async_trait]
pub trait CommandHost: Send + Sync {
    /// Clear the conversation history for the given session.
    async fn clear_session(&self, key: &SessionKey);

    /// Recent sessions visible to this host, newest first.
    async fn list_sessions(&self) -> Vec<SessionSummary>;

    /// The currently active model id (e.g. "openai/gpt-4.1").
    async fn current_model(&self) -> String;

    /// Switch the active model. Returns previous and current ids on success.
    async fn switch_model(&self, new: &str) -> Result<ModelSwitchInfo, String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use gasket_types::ChannelType;
    use std::sync::Mutex;

    pub struct StubHost {
        pub current: Mutex<String>,
        pub cleared: Mutex<Vec<SessionKey>>,
    }

    #[async_trait]
    impl CommandHost for StubHost {
        async fn clear_session(&self, key: &SessionKey) {
            self.cleared.lock().unwrap().push(key.clone());
        }
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            self.current.lock().unwrap().clone()
        }
        async fn switch_model(&self, new: &str) -> Result<ModelSwitchInfo, String> {
            let mut g = self.current.lock().unwrap();
            let previous = g.clone();
            *g = new.to_string();
            Ok(ModelSwitchInfo {
                previous,
                current: new.to_string(),
            })
        }
    }

    #[tokio::test]
    async fn stub_host_round_trip() {
        let host = StubHost {
            current: Mutex::new("a".into()),
            cleared: Mutex::new(vec![]),
        };
        let info = host.switch_model("b").await.unwrap();
        assert_eq!(info.previous, "a");
        assert_eq!(info.current, "b");
        assert_eq!(host.current_model().await, "b");
        let key = SessionKey::new(ChannelType::Cli, "x");
        host.clear_session(&key).await;
        assert_eq!(host.cleared.lock().unwrap().len(), 1);
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p gasket-command --lib host
```

Expected: 1 passed.

- [ ] **Step 3: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): document CommandHost trait

Four methods (clear_session, list_sessions, current_model, switch_model)
matching the day-1 built-in command needs. Test-only StubHost exercises
the surface.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §3.4

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 6 — Parser (TDD)

**What:** Implement `parse(input) -> ParsedInput` covering the nine cases in spec §4.1.

**Why:** First non-trivial logic in the crate. Pure function, easy to TDD, sets the discipline tone.

**Where:**
- Create: `gasket/command/src/parser.rs`
- Modify: `gasket/command/src/lib.rs`

**How:** Write all nine table tests first. Run them and watch them fail. Implement `parse`. Run again, watch pass.

**Test Case & Acceptance Criteria:**
- All nine table rows pass.

- [ ] **Step 1: Write the failing tests + empty parser stub**

Create `gasket/command/src/parser.rs`:

```rust
//! Slash-command parser. Turns a user input line into either a (name, args)
//! pair or "this is not a command".

#[derive(Debug, PartialEq)]
pub enum ParsedInput<'a> {
    Command { name: &'a str, args: &'a str },
    NotCommand,
}

pub fn parse(_input: &str) -> ParsedInput<'_> {
    todo!("implemented in step 3")
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! cmd {
        ($name:expr, $args:expr) => {
            ParsedInput::Command {
                name: $name,
                args: $args,
            }
        };
    }

    #[test]
    fn parse_help_no_args() {
        assert_eq!(parse("/help"), cmd!("help", ""));
    }

    #[test]
    fn parse_command_with_args() {
        assert_eq!(
            parse("/translate hello world"),
            cmd!("translate", "hello world")
        );
    }

    #[test]
    fn parse_preserves_internal_whitespace_and_trims_outer() {
        assert_eq!(
            parse("/translate   hello   world  "),
            cmd!("translate", "hello   world")
        );
    }

    #[test]
    fn parse_strips_leading_whitespace() {
        assert_eq!(parse("  /help"), cmd!("help", ""));
    }

    #[test]
    fn parse_lone_slash_is_not_command() {
        assert_eq!(parse("/"), ParsedInput::NotCommand);
    }

    #[test]
    fn parse_slash_with_only_whitespace_is_not_command() {
        assert_eq!(parse("/  "), ParsedInput::NotCommand);
    }

    #[test]
    fn parse_empty_string_is_not_command() {
        assert_eq!(parse(""), ParsedInput::NotCommand);
    }

    #[test]
    fn parse_plain_text_is_not_command() {
        assert_eq!(parse("hello"), ParsedInput::NotCommand);
    }

    #[test]
    fn parse_double_slash_yields_unknown_name() {
        // The dispatcher will report this as "unknown command: //cmd".
        // Parser only reports the lexical split.
        assert_eq!(parse("//cmd"), cmd!("/cmd", ""));
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`**

Add to `gasket/command/src/lib.rs`:

```rust
pub mod parser;
```

- [ ] **Step 3: Verify the tests fail**

Run:

```bash
cargo test -p gasket-command --lib parser
```

Expected: 9 tests panic with `not yet implemented`.

- [ ] **Step 4: Implement `parse`**

Replace the `parse` body in `gasket/command/src/parser.rs`:

```rust
pub fn parse(input: &str) -> ParsedInput<'_> {
    let trimmed = input.trim_start();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return ParsedInput::NotCommand;
    };
    let mut iter = rest.splitn(2, char::is_whitespace);
    let name = iter.next().unwrap_or("");
    let args = iter.next().unwrap_or("").trim();
    if name.is_empty() {
        ParsedInput::NotCommand
    } else {
        ParsedInput::Command { name, args }
    }
}
```

- [ ] **Step 5: Verify the tests pass**

```bash
cargo test -p gasket-command --lib parser
```

Expected: 9 passed.

- [ ] **Step 6: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): add slash-command parser

Pure function, splitn(2) on first whitespace, trim outer args, empty
name after slash means not a command. Nine table-driven tests cover
the boundary cases listed in the spec.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §4.1

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 7 — Template renderer (TDD)

**What:** Implement `render(template, user_input) -> String` that substitutes `{{user_input}}` only.

**Why:** Spec §4.3. Pure function, day-1 supports exactly one placeholder. Adding a second variable later is mechanical.

**Where:**
- Create: `gasket/command/src/template.rs`
- Modify: `gasket/command/src/lib.rs`

**How:** Tests first. Implementation is one line.

**Test Case & Acceptance Criteria:**
- All four tests pass.

- [ ] **Step 1: Write failing tests + empty render stub**

Create `gasket/command/src/template.rs`:

```rust
//! Day-1 template renderer. Single-placeholder string replacement.

pub fn render(_template: &str, _user_input: &str) -> String {
    todo!("step 3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_user_input() {
        assert_eq!(render("X {{user_input}} Y", "hello"), "X hello Y");
    }

    #[test]
    fn substitutes_multiple_occurrences() {
        assert_eq!(
            render("{{user_input}}-{{user_input}}", "a"),
            "a-a"
        );
    }

    #[test]
    fn unknown_placeholder_passes_through() {
        assert_eq!(render("{{foo}}", "ignored"), "{{foo}}");
    }

    #[test]
    fn empty_user_input_yields_empty_substitution() {
        assert_eq!(
            render("Translate: {{user_input}} done.", ""),
            "Translate:  done."
        );
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`**

Add to `gasket/command/src/lib.rs`:

```rust
pub mod template;
```

- [ ] **Step 3: Verify failure**

```bash
cargo test -p gasket-command --lib template
```

Expected: 4 panics.

- [ ] **Step 4: Implement `render`**

Replace the body:

```rust
pub fn render(template: &str, user_input: &str) -> String {
    template.replace("{{user_input}}", user_input)
}
```

- [ ] **Step 5: Verify pass**

```bash
cargo test -p gasket-command --lib template
```

Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): add single-placeholder template renderer

Day-1 supports only {{user_input}}. Unknown placeholders pass through
to the LLM verbatim.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §4.3

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 8 — Dispatcher: route() with builtin / unknown / passthrough (TDD)

**What:** Implement the core `Dispatcher::route()` for the three simplest paths: built-in command match, unknown `/cmd`, and non-command passthrough. Aliases and YAML are added in Task 9.

**Why:** Spec §4.2. Splitting the route logic into two tasks lets us land a small, testable subset first.

**Where:**
- Create: `gasket/command/src/dispatcher.rs`
- Modify: `gasket/command/src/lib.rs`

**How:** Define a `MockCommandHost` test helper. Write three failing tests. Implement `Dispatcher::route` with just the parser + canonical lookup + builtin invocation.

**Test Case & Acceptance Criteria:**
- 3 dispatcher tests pass.

- [ ] **Step 1: Write the failing tests + skeleton**

Create `gasket/command/src/dispatcher.rs`:

```rust
//! Slash-command dispatcher.

use std::collections::HashMap;
use std::sync::Arc;

use crate::host::CommandHost;
use crate::parser::{parse, ParsedInput};
use crate::types::{Command, CommandKind, CommandResult, RouteOutcome};

pub struct Dispatcher {
    pub(crate) commands: HashMap<String, Arc<Command>>,
    pub(crate) aliases: HashMap<String, String>,
    pub(crate) host: Arc<dyn CommandHost>,
}

impl Dispatcher {
    pub async fn route(&self, _line: &str) -> RouteOutcome {
        todo!("step 3")
    }

    pub fn list_commands(&self) -> Vec<&Command> {
        let mut v: Vec<&Command> = self.commands.values().map(|a| a.as_ref()).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BuiltinHandler;
    use async_trait::async_trait;
    use futures::FutureExt;
    use gasket_types::{ChannelType, ModelSwitchInfo, SessionKey, SessionSummary};
    use std::sync::Mutex;

    pub struct MockCommandHost {
        pub clear_calls: Mutex<Vec<SessionKey>>,
    }

    impl MockCommandHost {
        pub fn new() -> Self {
            Self {
                clear_calls: Mutex::new(vec![]),
            }
        }
    }

    #[async_trait]
    impl CommandHost for MockCommandHost {
        async fn clear_session(&self, key: &SessionKey) {
            self.clear_calls.lock().unwrap().push(key.clone());
        }
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            "test-model".into()
        }
        async fn switch_model(&self, new: &str) -> Result<ModelSwitchInfo, String> {
            Ok(ModelSwitchInfo {
                previous: "test-model".into(),
                current: new.into(),
            })
        }
    }

    fn echo_handler() -> BuiltinHandler {
        Arc::new(|args: &str, _host: &dyn CommandHost| {
            let s = format!("echo: {}", args);
            async move { CommandResult::Print(s) }.boxed()
        })
    }

    fn make_dispatcher_with(commands: Vec<Command>) -> Dispatcher {
        let mut map: HashMap<String, Arc<Command>> = HashMap::new();
        for c in commands {
            map.insert(c.name.clone(), Arc::new(c));
        }
        Dispatcher {
            commands: map,
            aliases: HashMap::new(),
            host: Arc::new(MockCommandHost::new()),
        }
    }

    #[tokio::test]
    async fn builtin_match_invokes_handler() {
        let cmd = Command {
            name: "echo".into(),
            description: "echoes args".into(),
            aliases: vec![],
            kind: CommandKind::Builtin(echo_handler()),
        };
        let d = make_dispatcher_with(vec![cmd]);

        let outcome = d.route("/echo hello world").await;

        assert_eq!(
            outcome,
            RouteOutcome::Handled(CommandResult::Print("echo: hello world".into()))
        );
    }

    #[tokio::test]
    async fn unknown_command_returns_error() {
        let d = make_dispatcher_with(vec![]);
        let outcome = d.route("/whatisthis").await;
        match outcome {
            RouteOutcome::Handled(CommandResult::Error(msg)) => {
                assert!(msg.contains("/whatisthis"), "msg = {msg}");
                assert!(msg.contains("/help"), "should hint /help; msg = {msg}");
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn non_command_text_passes_through_verbatim() {
        let d = make_dispatcher_with(vec![]);
        let outcome = d.route("hello world").await;
        assert_eq!(outcome, RouteOutcome::Passthrough("hello world".into()));
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`**

Add to `gasket/command/src/lib.rs`:

```rust
pub mod dispatcher;
pub use dispatcher::Dispatcher;
```

- [ ] **Step 3: Verify the tests fail**

```bash
cargo test -p gasket-command --lib dispatcher
```

Expected: 3 panics from `todo!`.

- [ ] **Step 4: Implement `route()`**

Replace the `route` and add a private `dispatch` in `dispatcher.rs`:

```rust
impl Dispatcher {
    pub async fn route(&self, line: &str) -> RouteOutcome {
        match parse(line) {
            ParsedInput::NotCommand => RouteOutcome::Passthrough(line.to_string()),
            ParsedInput::Command { name, args } => self.dispatch(name, args).await,
        }
    }

    async fn dispatch(&self, name: &str, args: &str) -> RouteOutcome {
        let canonical = self
            .aliases
            .get(name)
            .map(String::as_str)
            .unwrap_or(name);

        let Some(cmd) = self.commands.get(canonical) else {
            return RouteOutcome::Handled(CommandResult::Error(format!(
                "unknown command: /{name}    (type /help to see commands)"
            )));
        };

        match &cmd.kind {
            CommandKind::Builtin(handler) => {
                RouteOutcome::Handled(handler(args, self.host.as_ref()).await)
            }
            CommandKind::Yaml { .. } => unimplemented!("Task 9 adds Yaml handling"),
        }
    }

    pub fn list_commands(&self) -> Vec<&Command> {
        let mut v: Vec<&Command> = self.commands.values().map(|a| a.as_ref()).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}
```

- [ ] **Step 5: Verify the tests pass**

```bash
cargo test -p gasket-command --lib dispatcher
```

Expected: 3 passed.

- [ ] **Step 6: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): dispatcher route for builtin/unknown/passthrough

Three of the five routing paths. Aliases and YAML rewrite added in
the next task. MockCommandHost test helper lives next to the dispatcher
tests so other tasks can reuse it.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §4.2

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 9 — Dispatcher: aliases + YAML rewrite (TDD)

**What:** Extend `dispatch` to resolve aliases and to handle `CommandKind::Yaml` by emitting `RouteOutcome::Rewrite`.

**Why:** Spec §3.5 (alias precedence) and §4.2 (YAML produces Rewrite, not Handled). This completes the routing matrix.

**Where:**
- Modify: `gasket/command/src/dispatcher.rs`

**How:** Two new tests. The alias path already exists in code from Task 8 (the `self.aliases.get` lookup); we just need a test that exercises it. The YAML path replaces the `unimplemented!`.

**Test Case & Acceptance Criteria:**
- 5 dispatcher tests pass total (3 from Task 8 + 2 new).

- [ ] **Step 1: Write the failing tests**

Append inside the `#[cfg(test)] mod tests` of `dispatcher.rs`:

```rust
#[tokio::test]
async fn alias_resolves_to_canonical() {
    let cmd = Command {
        name: "exit".into(),
        description: "exit".into(),
        aliases: vec!["q".into(), "quit".into()],
        kind: CommandKind::Builtin(Arc::new(|_, _| {
            async { CommandResult::Quit }.boxed()
        })),
    };
    let mut map: HashMap<String, Arc<Command>> = HashMap::new();
    let arc = Arc::new(cmd);
    map.insert(arc.name.clone(), arc.clone());
    let mut aliases = HashMap::new();
    aliases.insert("q".into(), "exit".into());
    aliases.insert("quit".into(), "exit".into());
    let d = Dispatcher {
        commands: map,
        aliases,
        host: Arc::new(MockCommandHost::new()),
    };

    assert_eq!(
        d.route("/q").await,
        RouteOutcome::Handled(CommandResult::Quit)
    );
    assert_eq!(
        d.route("/quit").await,
        RouteOutcome::Handled(CommandResult::Quit)
    );
}

#[tokio::test]
async fn yaml_kind_produces_rewrite_with_filter() {
    let cmd = Command {
        name: "translate".into(),
        description: "translate".into(),
        aliases: vec![],
        kind: CommandKind::Yaml {
            prompt_template: "Translate to Mandarin: {{user_input}}".into(),
            allowed_tools: Some(vec!["wiki_search".into()]),
        },
    };
    let d = make_dispatcher_with(vec![cmd]);

    let outcome = d.route("/translate Hello world").await;

    assert_eq!(
        outcome,
        RouteOutcome::Rewrite {
            prompt: "Translate to Mandarin: Hello world".into(),
            tool_filter: Some(vec!["wiki_search".into()]),
        }
    );
}

#[tokio::test]
async fn yaml_with_no_tool_filter_passes_none() {
    let cmd = Command {
        name: "review".into(),
        description: "review".into(),
        aliases: vec![],
        kind: CommandKind::Yaml {
            prompt_template: "Review:\n{{user_input}}".into(),
            allowed_tools: None,
        },
    };
    let d = make_dispatcher_with(vec![cmd]);

    let outcome = d.route("/review my code").await;

    assert_eq!(
        outcome,
        RouteOutcome::Rewrite {
            prompt: "Review:\nmy code".into(),
            tool_filter: None,
        }
    );
}
```

- [ ] **Step 2: Verify two of the three tests fail**

Run:

```bash
cargo test -p gasket-command --lib dispatcher
```

Expected: `alias_resolves_to_canonical` passes (already wired by Task 8). The two YAML tests panic with `unimplemented`.

- [ ] **Step 3: Implement YAML branch**

In `dispatcher.rs`, replace the `CommandKind::Yaml { .. } => unimplemented!(...)` line with:

```rust
CommandKind::Yaml {
    prompt_template,
    allowed_tools,
} => RouteOutcome::Rewrite {
    prompt: crate::template::render(prompt_template, args),
    tool_filter: allowed_tools.clone(),
},
```

- [ ] **Step 4: Verify all dispatcher tests pass**

```bash
cargo test -p gasket-command --lib dispatcher
```

Expected: 5 passed (or 6 — depending on whether you ran the new tests already).

- [ ] **Step 5: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): dispatcher resolves aliases and rewrites YAML commands

Five routing paths now covered: builtin, alias→builtin, yaml→Rewrite,
unknown, passthrough. tool_filter on Yaml propagates to RouteOutcome.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §3.5, §4.2

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 10 — YAML loader (TDD)

**What:** Implement `load_user_commands(dir: &Path) -> Vec<Command>` that scans a directory for `*.md` files, parses front-matter, and returns the valid commands. Bad files emit `tracing::warn!` and are skipped.

**Why:** Spec §6.3. The dispatcher builder calls this during `build()`.

**Where:**
- Create: `gasket/command/src/yaml_loader.rs`
- Modify: `gasket/command/src/lib.rs`

**How:** Front-matter parsing reuses the same shape as `gasket/engine/src/skills/loader.rs` (lines starting with `---` delimit a YAML block). For day-1 we hand-roll the split rather than abstracting a shared utility (YAGNI). Tests use `tempfile::TempDir`.

**Test Case & Acceptance Criteria:**
- 7 tests pass: valid file, broken yaml, missing required field, no front-matter, non-md skipped, empty dir, missing dir.

- [ ] **Step 1: Add the loader skeleton**

Create `gasket/command/src/yaml_loader.rs`:

```rust
//! Loads user-defined slash commands from `*.md` files with YAML front-matter.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::warn;

use crate::types::{Command, CommandKind};

#[derive(Deserialize)]
struct FrontMatter {
    name: String,
    description: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    allowed_tools: Option<Vec<String>>,
}

pub async fn load_user_commands(dir: &Path) -> Vec<Command> {
    let mut entries: Vec<PathBuf> = match collect_md_paths(dir).await {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    entries.sort();
    let mut out = Vec::new();
    for path in entries {
        match load_one(&path).await {
            Ok(cmd) => out.push(cmd),
            Err(reason) => warn!(?path, reason, "skipping user command file"),
        }
    }
    out
}

async fn collect_md_paths(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    if !tokio::fs::try_exists(dir).await? {
        return Ok(vec![]);
    }
    let mut rd = tokio::fs::read_dir(dir).await?;
    let mut out = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("md") {
            out.push(p);
        }
    }
    Ok(out)
}

async fn load_one(path: &Path) -> Result<Command, String> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("read failed: {e}"))?;
    let (front, body) = split_front_matter(&raw).ok_or("missing front-matter")?;
    let fm: FrontMatter =
        serde_yaml::from_str(front).map_err(|e| format!("yaml parse: {e}"))?;
    if fm.name.trim().is_empty() {
        return Err("name field is empty".into());
    }
    if fm.description.trim().is_empty() {
        return Err("description field is empty".into());
    }
    Ok(Command {
        name: fm.name,
        description: fm.description,
        aliases: fm.aliases,
        kind: CommandKind::Yaml {
            prompt_template: body.trim_start().to_string(),
            allowed_tools: fm.allowed_tools,
        },
    })
}

fn split_front_matter(raw: &str) -> Option<(&str, &str)> {
    let stripped = raw.strip_prefix("---")?;
    let stripped = stripped.strip_prefix('\n').or_else(|| stripped.strip_prefix("\r\n"))?;
    let end = stripped.find("\n---")?;
    let front = &stripped[..end];
    let after = &stripped[end + 4..];
    let body = after.strip_prefix('\n').or_else(|| after.strip_prefix("\r\n")).unwrap_or(after);
    Some((front, body))
}
```

- [ ] **Step 2: Wire into `lib.rs`**

Add:

```rust
pub mod yaml_loader;
```

- [ ] **Step 3: Write failing tests**

Append `#[cfg(test)] mod tests` to `yaml_loader.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    async fn write(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let p = dir.path().join(name);
        tokio::fs::write(&p, content).await.unwrap();
        p
    }

    fn good_translate() -> &'static str {
        "---\n\
name: translate\n\
description: Translate text to Mandarin\n\
aliases: [tr]\n\
allowed_tools: []\n\
---\n\
\n\
Translate the following:\n\
{{user_input}}\n"
    }

    #[tokio::test]
    async fn loads_valid_command() {
        let dir = TempDir::new().unwrap();
        write(&dir, "translate.md", good_translate()).await;

        let cmds = load_user_commands(dir.path()).await;

        assert_eq!(cmds.len(), 1);
        let c = &cmds[0];
        assert_eq!(c.name, "translate");
        assert_eq!(c.aliases, vec!["tr".to_string()]);
        match &c.kind {
            CommandKind::Yaml { prompt_template, allowed_tools } => {
                assert!(prompt_template.contains("{{user_input}}"));
                assert_eq!(allowed_tools, &Some(vec![]));
            }
            _ => panic!("expected Yaml kind"),
        }
    }

    #[tokio::test]
    async fn skips_broken_yaml() {
        let dir = TempDir::new().unwrap();
        write(&dir, "broken.md", "---\nthis: is: bad: yaml\n---\nbody\n").await;

        let cmds = load_user_commands(dir.path()).await;
        assert_eq!(cmds.len(), 0);
    }

    #[tokio::test]
    async fn skips_missing_name() {
        let dir = TempDir::new().unwrap();
        write(&dir, "no-name.md", "---\ndescription: x\n---\nbody\n").await;

        let cmds = load_user_commands(dir.path()).await;
        assert_eq!(cmds.len(), 0);
    }

    #[tokio::test]
    async fn skips_missing_front_matter() {
        let dir = TempDir::new().unwrap();
        write(&dir, "plain.md", "no front matter here\n").await;

        let cmds = load_user_commands(dir.path()).await;
        assert_eq!(cmds.len(), 0);
    }

    #[tokio::test]
    async fn ignores_non_md_files() {
        let dir = TempDir::new().unwrap();
        write(&dir, "translate.md", good_translate()).await;
        write(&dir, "notes.txt", good_translate()).await;
        write(&dir, "README", good_translate()).await;

        let cmds = load_user_commands(dir.path()).await;
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].name, "translate");
    }

    #[tokio::test]
    async fn empty_dir_returns_empty_vec() {
        let dir = TempDir::new().unwrap();
        let cmds = load_user_commands(dir.path()).await;
        assert!(cmds.is_empty());
    }

    #[tokio::test]
    async fn missing_dir_returns_empty_vec_silently() {
        let dir = TempDir::new().unwrap();
        let nope = dir.path().join("does-not-exist");
        let cmds = load_user_commands(&nope).await;
        assert!(cmds.is_empty());
    }

    #[tokio::test]
    async fn lex_order_is_deterministic() {
        let dir = TempDir::new().unwrap();
        // intentionally write out of order
        write(&dir, "z-zulu.md", &swap_name(good_translate(), "zulu")).await;
        write(&dir, "a-alpha.md", &swap_name(good_translate(), "alpha")).await;
        write(&dir, "m-mike.md", &swap_name(good_translate(), "mike")).await;

        let names: Vec<String> =
            load_user_commands(dir.path()).await.into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["alpha", "mike", "zulu"]);
    }

    fn swap_name(template: &str, new_name: &str) -> String {
        template.replace("name: translate", &format!("name: {new_name}"))
    }
}
```

- [ ] **Step 4: Verify**

```bash
cargo test -p gasket-command --lib yaml_loader
```

Expected: 8 passed.

- [ ] **Step 5: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): load user commands from front-matter Markdown

Scans a directory for *.md files, parses YAML front-matter, drops
broken/incomplete files with a tracing::warn. Lex-ordered for
deterministic collision resolution. Missing dir is silent.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §6.3

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 11 — `DispatcherBuilder::build()` with collision rules (TDD)

**What:** Implement the builder that ties built-ins, user YAML, and the host together. Enforce precedence: built-in canonical > built-in alias > user YAML > nothing.

**Why:** Spec §3.3 and §3.5. Until now `Dispatcher` is hand-constructed in tests; production code needs this builder.

**Where:**
- Modify: `gasket/command/src/dispatcher.rs`

**How:** Add `DispatcherBuilder` struct, `register_builtin` / `user_dir` / `host` methods, and `build()`. Tests cover: missing host fails, duplicate builtin fails, builtin wins over user YAML name collision, two user YAMLs collide and lex-first wins.

**Test Case & Acceptance Criteria:**
- 4 builder tests pass.
- All previous dispatcher tests still pass.

- [ ] **Step 1: Add the builder skeleton**

Append to `gasket/command/src/dispatcher.rs` (after the `impl Dispatcher` block):

```rust
use std::path::PathBuf;

use crate::error::BuildError;
use crate::yaml_loader::load_user_commands;

pub struct DispatcherBuilder {
    builtins: Vec<Command>,
    user_yaml_dir: Option<PathBuf>,
    host: Option<Arc<dyn CommandHost>>,
}

impl Dispatcher {
    pub fn builder() -> DispatcherBuilder {
        DispatcherBuilder::new()
    }
}

impl DispatcherBuilder {
    pub fn new() -> Self {
        Self {
            builtins: Vec::new(),
            user_yaml_dir: None,
            host: None,
        }
    }

    pub fn register_builtin(mut self, cmd: Command) -> Self {
        self.builtins.push(cmd);
        self
    }

    pub fn user_dir(mut self, p: PathBuf) -> Self {
        self.user_yaml_dir = Some(p);
        self
    }

    pub fn host(mut self, h: Arc<dyn CommandHost>) -> Self {
        self.host = Some(h);
        self
    }

    pub async fn build(self) -> Result<Dispatcher, BuildError> {
        let host = self.host.ok_or(BuildError::MissingHost)?;

        let mut commands: HashMap<String, Arc<Command>> = HashMap::new();
        let mut aliases: HashMap<String, String> = HashMap::new();

        // 1. Built-ins first; duplicates are programmer bugs and fail the build.
        for cmd in self.builtins {
            if commands.contains_key(&cmd.name) {
                return Err(BuildError::DuplicateBuiltin(cmd.name.clone()));
            }
            for a in &cmd.aliases {
                aliases.insert(a.clone(), cmd.name.clone());
            }
            commands.insert(cmd.name.clone(), Arc::new(cmd));
        }

        // 2. User commands; collisions warn-and-drop, never fatal.
        if let Some(dir) = self.user_yaml_dir {
            for cmd in load_user_commands(&dir).await {
                if commands.contains_key(&cmd.name) || aliases.contains_key(&cmd.name) {
                    tracing::warn!(
                        name = cmd.name,
                        "user command name collides with a built-in; dropping user definition"
                    );
                    continue;
                }
                let mut conflicting_alias = false;
                for a in &cmd.aliases {
                    if commands.contains_key(a) || aliases.contains_key(a) {
                        tracing::warn!(
                            name = cmd.name,
                            alias = a,
                            "user command alias collides with a built-in; dropping user definition"
                        );
                        conflicting_alias = true;
                        break;
                    }
                }
                if conflicting_alias {
                    continue;
                }
                let arc = Arc::new(cmd);
                for a in &arc.aliases {
                    aliases.insert(a.clone(), arc.name.clone());
                }
                commands.insert(arc.name.clone(), arc);
            }
        }

        Ok(Dispatcher {
            commands,
            aliases,
            host,
        })
    }
}

impl Default for DispatcherBuilder {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 2: Add a `pub use` line to `lib.rs`**

Update `gasket/command/src/lib.rs`:

```rust
pub use dispatcher::{Dispatcher, DispatcherBuilder};
```

- [ ] **Step 3: Write failing tests**

Append to the `#[cfg(test)] mod tests` of `dispatcher.rs`:

```rust
use crate::error::BuildError;
use std::path::PathBuf;
use tempfile::TempDir;

fn make_builtin(name: &str, alias: &[&str]) -> Command {
    Command {
        name: name.into(),
        description: format!("desc-{name}"),
        aliases: alias.iter().map(|s| s.to_string()).collect(),
        kind: CommandKind::Builtin(Arc::new(|_, _| {
            async { CommandResult::Print("ok".into()) }.boxed()
        })),
    }
}

#[tokio::test]
async fn build_fails_without_host() {
    let res = DispatcherBuilder::new()
        .register_builtin(make_builtin("help", &[]))
        .build()
        .await;
    assert!(matches!(res, Err(BuildError::MissingHost)));
}

#[tokio::test]
async fn build_fails_on_duplicate_builtin() {
    let res = DispatcherBuilder::new()
        .host(Arc::new(MockCommandHost::new()))
        .register_builtin(make_builtin("help", &[]))
        .register_builtin(make_builtin("help", &[]))
        .build()
        .await;
    assert!(matches!(res, Err(BuildError::DuplicateBuiltin(_))));
}

#[tokio::test]
async fn user_yaml_colliding_with_builtin_is_dropped() {
    let dir = TempDir::new().unwrap();
    let yaml_help = "---\nname: help\ndescription: bogus help\n---\nbody\n";
    tokio::fs::write(dir.path().join("help.md"), yaml_help)
        .await
        .unwrap();

    let d = DispatcherBuilder::new()
        .host(Arc::new(MockCommandHost::new()))
        .user_dir(dir.path().to_path_buf())
        .register_builtin(make_builtin("help", &[]))
        .build()
        .await
        .unwrap();

    let help = d.commands.get("help").unwrap();
    assert_eq!(help.description, "desc-help");
}

#[tokio::test]
async fn two_user_yamls_with_same_name_first_wins() {
    let dir = TempDir::new().unwrap();
    let body_a = "---\nname: foo\ndescription: from-a\n---\nbody-a\n";
    let body_z = "---\nname: foo\ndescription: from-z\n---\nbody-z\n";
    tokio::fs::write(dir.path().join("a.md"), body_a).await.unwrap();
    tokio::fs::write(dir.path().join("z.md"), body_z).await.unwrap();

    let d = DispatcherBuilder::new()
        .host(Arc::new(MockCommandHost::new()))
        .user_dir(dir.path().to_path_buf())
        .build()
        .await
        .unwrap();

    let foo = d.commands.get("foo").unwrap();
    assert_eq!(foo.description, "from-a");
}

#[tokio::test]
async fn build_smoke_with_host_and_one_builtin() {
    let d = DispatcherBuilder::new()
        .host(Arc::new(MockCommandHost::new()))
        .register_builtin(make_builtin("ping", &[]))
        .build()
        .await
        .unwrap();
    assert!(d.commands.contains_key("ping"));
}
```

- [ ] **Step 4: Run, fix any compile errors, verify**

```bash
cargo test -p gasket-command --lib dispatcher
```

Expected: 10 passed (5 from earlier + 5 new). If the second-user-wins test reveals that `load_user_commands` doesn't actually drop the duplicate, fix the loader to skip names already in the result. (Loader returns a `Vec` from disk; the builder is what enforces uniqueness.)

If you find that two user YAMLs with the same name *both* end up in the loader's `Vec`, add the dedup in the builder loop: track a separate `seen` set and `continue` on a hit, with a `warn!`.

- [ ] **Step 5: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): DispatcherBuilder with collision precedence

Builtin canonical and aliases registered first. User YAML files loaded
in lex order; any collision with a built-in or with an earlier user
command is logged and dropped. Missing host returns BuildError; duplicate
built-in names return BuildError. Per-file YAML errors are warnings.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §3.3, §3.5

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 12 — Built-ins: `/exit` and `/clear`

**What:** Add the two simplest built-in handlers. Neither calls `host`. `/exit` returns `Quit`; `/clear` returns the ANSI clear-screen string.

**Why:** Spec §5. Cheapest path to a working built-in surface.

**Where:**
- Create: `gasket/command/src/builtins/mod.rs`
- Create: `gasket/command/src/builtins/exit.rs`
- Create: `gasket/command/src/builtins/clear.rs`
- Modify: `gasket/command/src/lib.rs`

**How:** Each file exports one constructor `pub fn exit() -> Command` / `pub fn clear() -> Command`. Tests verify the produced `RouteOutcome`.

**Test Case & Acceptance Criteria:**
- 2 tests pass: `/exit` produces Quit; `/clear` prints the ANSI sequence.

- [ ] **Step 1: Create the builtins module**

Create `gasket/command/src/builtins/mod.rs`:

```rust
//! Built-in slash commands.

pub mod clear;
pub mod exit;

pub use clear::clear;
pub use exit::exit;
```

Update `gasket/command/src/lib.rs`:

```rust
pub mod builtins;
```

- [ ] **Step 2: Implement `/exit`**

Create `gasket/command/src/builtins/exit.rs`:

```rust
use std::sync::Arc;

use futures::FutureExt;

use crate::host::CommandHost;
use crate::types::{Command, CommandKind, CommandResult};

pub fn exit() -> Command {
    Command {
        name: "exit".into(),
        description: "Exit the REPL".into(),
        aliases: vec!["quit".into(), "q".into(), ":q".into()],
        kind: CommandKind::Builtin(Arc::new(|_args: &str, _host: &dyn CommandHost| {
            async { CommandResult::Quit }.boxed()
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::DispatcherBuilder;
    use crate::types::RouteOutcome;
    use std::sync::Arc;

    // Reuse MockCommandHost via the dispatcher tests by re-declaring a
    // minimal one here. Keeping it private avoids a public test helper.
    struct H;
    use async_trait::async_trait;
    use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};
    #[async_trait]
    impl CommandHost for H {
        async fn clear_session(&self, _: &SessionKey) {}
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            "m".into()
        }
        async fn switch_model(&self, _new: &str) -> Result<ModelSwitchInfo, String> {
            Ok(ModelSwitchInfo {
                previous: "m".into(),
                current: "m".into(),
            })
        }
    }

    #[tokio::test]
    async fn exit_canonical_returns_quit() {
        let d = DispatcherBuilder::new()
            .host(Arc::new(H))
            .register_builtin(exit())
            .build()
            .await
            .unwrap();
        assert_eq!(d.route("/exit").await, RouteOutcome::Handled(CommandResult::Quit));
    }

    #[tokio::test]
    async fn exit_aliases_resolve() {
        let d = DispatcherBuilder::new()
            .host(Arc::new(H))
            .register_builtin(exit())
            .build()
            .await
            .unwrap();
        for s in &["/quit", "/q", "/:q"] {
            assert_eq!(
                d.route(s).await,
                RouteOutcome::Handled(CommandResult::Quit),
                "alias {s}"
            );
        }
    }
}
```

- [ ] **Step 3: Implement `/clear`**

Create `gasket/command/src/builtins/clear.rs`:

```rust
use std::sync::Arc;

use futures::FutureExt;

use crate::host::CommandHost;
use crate::types::{Command, CommandKind, CommandResult};

const ANSI_CLEAR: &str = "\x1B[2J\x1B[H";

pub fn clear() -> Command {
    Command {
        name: "clear".into(),
        description: "Clear the terminal screen".into(),
        aliases: vec![],
        kind: CommandKind::Builtin(Arc::new(|_args: &str, _host: &dyn CommandHost| {
            async { CommandResult::Print(ANSI_CLEAR.to_string()) }.boxed()
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};
    use std::sync::Arc;

    use crate::dispatcher::DispatcherBuilder;
    use crate::types::RouteOutcome;

    struct H;
    #[async_trait]
    impl CommandHost for H {
        async fn clear_session(&self, _: &SessionKey) {}
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            "m".into()
        }
        async fn switch_model(&self, _: &str) -> Result<ModelSwitchInfo, String> {
            Ok(ModelSwitchInfo {
                previous: "m".into(),
                current: "m".into(),
            })
        }
    }

    #[tokio::test]
    async fn clear_emits_ansi_sequence() {
        let d = DispatcherBuilder::new()
            .host(Arc::new(H))
            .register_builtin(clear())
            .build()
            .await
            .unwrap();
        match d.route("/clear").await {
            RouteOutcome::Handled(CommandResult::Print(s)) => {
                assert_eq!(s, "\x1B[2J\x1B[H");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }
}
```

- [ ] **Step 4: Verify**

```bash
cargo test -p gasket-command --lib builtins
```

Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): add /exit and /clear built-ins

Two no-host built-ins. /exit + aliases (/quit /q /:q) returns Quit;
/clear emits the standard ANSI clear-screen escape.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §5

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 13 — Built-in: `/help`

**What:** Add the `/help` command that lists every registered built-in and user command in the format from spec §5.1.

**Why:** Discoverability. Without `/help`, the dispatcher is opaque to the user.

**Where:**
- Create: `gasket/command/src/builtins/help.rs`
- Modify: `gasket/command/src/builtins/mod.rs`
- Modify: `gasket/command/src/dispatcher.rs` (small extension — see step 1)

**How:** `/help` needs the dispatcher's command list. Since the `BuiltinHandler` signature doesn't take the dispatcher, plumb the list through a small piece of state. Cleanest approach: have `Dispatcher::route` look up `/help` specially, OR have the builder hand the list to `help()` at construction time as an `Arc<Mutex<Vec<HelpEntry>>>` filled in on `build()`. We pick the second approach — the help builtin accepts a snapshot at build time.

**Test Case & Acceptance Criteria:**
- 1 test: `/help` output contains every registered command's name and description.

- [ ] **Step 1: Add a help-list snapshot type**

Append to `gasket/command/src/types.rs`:

```rust
/// One row in the `/help` output.
#[derive(Debug, Clone, PartialEq)]
pub struct HelpEntry {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub source: HelpSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HelpSource {
    Builtin,
    User,
}
```

Re-export from `lib.rs`:

```rust
pub use types::{BuiltinHandler, Command, CommandKind, CommandResult, HelpEntry, HelpSource, RouteOutcome};
```

- [ ] **Step 2: Add `Dispatcher::help_listing` and a slot the help builtin reads from**

Modify `gasket/command/src/dispatcher.rs`. Add a shared snapshot held by the builder and the help builtin:

```rust
use std::sync::OnceLock;

pub(crate) type HelpSnapshot = OnceLock<Vec<HelpEntry>>;

pub fn shared_help_snapshot() -> Arc<HelpSnapshot> {
    Arc::new(OnceLock::new())
}
```

Inside `DispatcherBuilder::build`, after constructing `commands` and `aliases` but before returning `Dispatcher`, create the snapshot if `self.help_snapshot` is set and fill it:

(Add a new field to the builder.)

```rust
pub struct DispatcherBuilder {
    builtins: Vec<Command>,
    user_yaml_dir: Option<PathBuf>,
    host: Option<Arc<dyn CommandHost>>,
    help_snapshot: Option<Arc<HelpSnapshot>>,
}

impl DispatcherBuilder {
    pub fn help_snapshot(mut self, slot: Arc<HelpSnapshot>) -> Self {
        self.help_snapshot = Some(slot);
        self
    }
    // ... existing methods unchanged ...
}
```

Initialise it in `new()`:

```rust
help_snapshot: None,
```

After building `commands` and `aliases` in `build()`, populate the snapshot:

```rust
if let Some(slot) = self.help_snapshot.clone() {
    let mut entries: Vec<HelpEntry> = commands
        .values()
        .map(|c| HelpEntry {
            name: c.name.clone(),
            description: c.description.clone(),
            aliases: c.aliases.clone(),
            source: match &c.kind {
                CommandKind::Builtin(_) => HelpSource::Builtin,
                CommandKind::Yaml { .. } => HelpSource::User,
            },
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let _ = slot.set(entries);
}
```

- [ ] **Step 3: Implement `/help`**

Create `gasket/command/src/builtins/help.rs`:

```rust
use std::sync::Arc;

use futures::FutureExt;

use crate::dispatcher::HelpSnapshot;
use crate::host::CommandHost;
use crate::types::{Command, CommandKind, CommandResult, HelpEntry, HelpSource};

pub fn help(snapshot: Arc<HelpSnapshot>) -> Command {
    Command {
        name: "help".into(),
        description: "Show available commands".into(),
        aliases: vec!["?".into()],
        kind: CommandKind::Builtin(Arc::new(move |_args: &str, _host: &dyn CommandHost| {
            let snap = snapshot.clone();
            async move {
                let entries: &[HelpEntry] = match snap.get() {
                    Some(v) => v,
                    None => return CommandResult::Error("help snapshot not initialised".into()),
                };
                CommandResult::Print(render_help(entries))
            }
            .boxed()
        })),
    }
}

fn render_help(entries: &[HelpEntry]) -> String {
    let (builtin, user): (Vec<&HelpEntry>, Vec<&HelpEntry>) = entries
        .iter()
        .partition(|e| matches!(e.source, HelpSource::Builtin));

    let mut out = String::new();
    out.push_str("Built-in commands:\n");
    for e in &builtin {
        out.push_str(&format_row(e));
    }
    if !user.is_empty() {
        out.push_str("\nUser commands  (~/.gasket/commands):\n");
        for e in &user {
            out.push_str(&format_row(e));
        }
    }
    out
}

fn format_row(e: &HelpEntry) -> String {
    let alias_suffix = if e.aliases.is_empty() {
        String::new()
    } else {
        let aliases: Vec<String> = e.aliases.iter().map(|a| format!("/{a}")).collect();
        format!("  (aliases: {})", aliases.join(", "))
    };
    format!("  /{:<11} {}{}\n", e.name, e.description, alias_suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::{clear, exit};
    use crate::dispatcher::{shared_help_snapshot, DispatcherBuilder};
    use crate::host::CommandHost;
    use crate::types::RouteOutcome;
    use async_trait::async_trait;
    use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};
    use std::sync::Arc;

    struct H;
    #[async_trait]
    impl CommandHost for H {
        async fn clear_session(&self, _: &SessionKey) {}
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            "m".into()
        }
        async fn switch_model(&self, _: &str) -> Result<ModelSwitchInfo, String> {
            Ok(ModelSwitchInfo {
                previous: "m".into(),
                current: "m".into(),
            })
        }
    }

    #[tokio::test]
    async fn help_lists_registered_commands() {
        let snap = shared_help_snapshot();
        let d = DispatcherBuilder::new()
            .host(Arc::new(H))
            .help_snapshot(snap.clone())
            .register_builtin(exit())
            .register_builtin(clear())
            .register_builtin(help(snap.clone()))
            .build()
            .await
            .unwrap();

        match d.route("/help").await {
            RouteOutcome::Handled(CommandResult::Print(text)) => {
                assert!(text.contains("/clear"), "missing /clear: {text}");
                assert!(text.contains("/exit"), "missing /exit: {text}");
                assert!(text.contains("/help"), "missing /help: {text}");
                assert!(text.contains("Clear the terminal screen"));
                assert!(text.contains("Built-in commands:"));
            }
            other => panic!("unexpected: {:?}", other),
        }
    }
}
```

Update `gasket/command/src/builtins/mod.rs`:

```rust
pub mod clear;
pub mod exit;
pub mod help;

pub use clear::clear;
pub use exit::exit;
pub use help::help;
```

- [ ] **Step 4: Verify**

```bash
cargo test -p gasket-command --lib builtins::help
cargo test -p gasket-command
```

Expected: help test passes; no regressions.

- [ ] **Step 5: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): add /help built-in with snapshot list

Help receives a shared OnceLock snapshot of HelpEntry rows populated
during DispatcherBuilder::build. Output groups built-ins and user
commands separately, sorted alphabetically.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §5, §5.1

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 14 — Built-in: `/new`

**What:** `/new` clears the current session via `host.clear_session(&session_key)`.

**Why:** Spec §5. Replaces the existing hardcoded `/new` in `agent.rs`.

**Where:**
- Create: `gasket/command/src/builtins/new.rs`
- Modify: `gasket/command/src/builtins/mod.rs`

**How:** The handler needs a `SessionKey` to clear. Hardcoding it in the builtin would couple it to a fixed session, so the handler is constructed with an `Arc<SessionKey>` provided by the caller. CLI passes the interactive session key when registering.

**Test Case & Acceptance Criteria:**
- 1 test: `/new` causes `host.clear_session` to be called once with the configured key.

- [ ] **Step 1: Implement `/new`**

Create `gasket/command/src/builtins/new.rs`:

```rust
use std::sync::Arc;

use futures::FutureExt;
use gasket_types::SessionKey;

use crate::host::CommandHost;
use crate::types::{Command, CommandKind, CommandResult};

pub fn new(session_key: Arc<SessionKey>) -> Command {
    Command {
        name: "new".into(),
        description: "Start a new conversation".into(),
        aliases: vec![],
        kind: CommandKind::Builtin(Arc::new(move |_args: &str, host: &dyn CommandHost| {
            let key = session_key.clone();
            async move {
                host.clear_session(&key).await;
                CommandResult::Print(format!("✓ Session cleared ({})", key))
            }
            .boxed()
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use gasket_types::{ChannelType, ModelSwitchInfo, SessionKey, SessionSummary};
    use std::sync::Mutex;

    use crate::dispatcher::DispatcherBuilder;
    use crate::types::RouteOutcome;

    struct H {
        cleared: Mutex<Vec<SessionKey>>,
    }
    #[async_trait]
    impl CommandHost for H {
        async fn clear_session(&self, k: &SessionKey) {
            self.cleared.lock().unwrap().push(k.clone());
        }
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            "m".into()
        }
        async fn switch_model(&self, _: &str) -> Result<ModelSwitchInfo, String> {
            Ok(ModelSwitchInfo {
                previous: "m".into(),
                current: "m".into(),
            })
        }
    }

    #[tokio::test]
    async fn new_calls_clear_session_once_with_correct_key() {
        let key = SessionKey::new(ChannelType::Cli, "interactive");
        let host = Arc::new(H {
            cleared: Mutex::new(vec![]),
        });
        let d = DispatcherBuilder::new()
            .host(host.clone())
            .register_builtin(new(Arc::new(key.clone())))
            .build()
            .await
            .unwrap();

        let outcome = d.route("/new").await;

        match outcome {
            RouteOutcome::Handled(CommandResult::Print(msg)) => {
                assert!(msg.contains("Session cleared"));
            }
            other => panic!("{:?}", other),
        }
        let calls = host.cleared.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], key);
    }
}
```

- [ ] **Step 2: Update `builtins/mod.rs`**

```rust
pub mod new;
pub use new::new;
```

(Module list grows accordingly.)

- [ ] **Step 3: Verify**

```bash
cargo test -p gasket-command --lib builtins::new
```

Expected: 1 passed.

- [ ] **Step 4: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): add /new built-in

Calls host.clear_session(key) once. Session key is provided at builtin
construction time so the same code works for any channel/session pair.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §5

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 15 — Built-in: `/sessions`

**What:** `/sessions` lists recent sessions in a fixed-width table.

**Why:** Spec §5.1.

**Where:**
- Create: `gasket/command/src/builtins/sessions.rs`
- Modify: `gasket/command/src/builtins/mod.rs`

**How:** Pull `Vec<SessionSummary>` from host. Format using simple `format!` padding. Empty list prints a friendly "no sessions" message.

**Test Case & Acceptance Criteria:**
- 2 tests: empty list message, non-empty rendering with all rows.

- [ ] **Step 1: Implement `/sessions`**

Create `gasket/command/src/builtins/sessions.rs`:

```rust
use std::sync::Arc;

use futures::FutureExt;
use gasket_types::SessionSummary;

use crate::host::CommandHost;
use crate::types::{Command, CommandKind, CommandResult};

pub fn sessions() -> Command {
    Command {
        name: "sessions".into(),
        description: "List recent sessions".into(),
        aliases: vec!["ls".into()],
        kind: CommandKind::Builtin(Arc::new(|_args: &str, host: &dyn CommandHost| {
            async move {
                let rows = host.list_sessions().await;
                CommandResult::Print(render(&rows))
            }
            .boxed()
        })),
    }
}

fn render(rows: &[SessionSummary]) -> String {
    if rows.is_empty() {
        return "No sessions yet.".into();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "{:<30} {:>9}   {}\n",
        "SESSION KEY", "MESSAGES", "LAST ACTIVE"
    ));
    for r in rows {
        let last = match r.last_active {
            Some(t) => t.format("%Y-%m-%d %H:%M").to_string(),
            None => "—".into(),
        };
        out.push_str(&format!(
            "{:<30} {:>9}   {}\n",
            r.key.to_string(),
            r.message_count,
            last
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use gasket_types::{ChannelType, ModelSwitchInfo, SessionKey, SessionSummary};
    use std::sync::Arc;

    use crate::dispatcher::DispatcherBuilder;
    use crate::types::RouteOutcome;

    struct H {
        rows: Vec<SessionSummary>,
    }
    #[async_trait]
    impl CommandHost for H {
        async fn clear_session(&self, _: &SessionKey) {}
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            self.rows.clone()
        }
        async fn current_model(&self) -> String {
            "m".into()
        }
        async fn switch_model(&self, _: &str) -> Result<ModelSwitchInfo, String> {
            Ok(ModelSwitchInfo {
                previous: "m".into(),
                current: "m".into(),
            })
        }
    }

    #[tokio::test]
    async fn empty_list_yields_friendly_message() {
        let d = DispatcherBuilder::new()
            .host(Arc::new(H { rows: vec![] }))
            .register_builtin(sessions())
            .build()
            .await
            .unwrap();
        match d.route("/sessions").await {
            RouteOutcome::Handled(CommandResult::Print(s)) => {
                assert!(s.starts_with("No sessions"));
            }
            other => panic!("{:?}", other),
        }
    }

    #[tokio::test]
    async fn renders_table() {
        let row = SessionSummary {
            key: SessionKey::new(ChannelType::Cli, "interactive"),
            message_count: 42,
            last_active: Some(Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap()),
        };
        let d = DispatcherBuilder::new()
            .host(Arc::new(H { rows: vec![row] }))
            .register_builtin(sessions())
            .build()
            .await
            .unwrap();
        match d.route("/sessions").await {
            RouteOutcome::Handled(CommandResult::Print(s)) => {
                assert!(s.contains("SESSION KEY"));
                assert!(s.contains("42"));
                assert!(s.contains("interactive"));
                assert!(s.contains("2026-05-03"));
            }
            other => panic!("{:?}", other),
        }
    }
}
```

- [ ] **Step 2: Update `builtins/mod.rs`**

```rust
pub mod sessions;
pub use sessions::sessions;
```

- [ ] **Step 3: Verify**

```bash
cargo test -p gasket-command --lib builtins::sessions
```

Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): add /sessions built-in

Lists session keys in a fixed-width table. Empty result prints a
friendly message rather than an empty header row.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §5.1

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 16 — Built-in: `/model`

**What:** `/model` with no args shows the current model. With args, attempts to switch.

**Why:** Spec §5.1.

**Where:**
- Create: `gasket/command/src/builtins/model.rs`
- Modify: `gasket/command/src/builtins/mod.rs`

**How:** Branch on whether `args` is empty (after the parser already trimmed). On switch error, return `CommandResult::Error`.

**Test Case & Acceptance Criteria:**
- 3 tests: no args path, successful switch, switch error.

- [ ] **Step 1: Implement `/model`**

Create `gasket/command/src/builtins/model.rs`:

```rust
use std::sync::Arc;

use futures::FutureExt;

use crate::host::CommandHost;
use crate::types::{Command, CommandKind, CommandResult};

pub fn model() -> Command {
    Command {
        name: "model".into(),
        description: "Show or switch the active model".into(),
        aliases: vec![],
        kind: CommandKind::Builtin(Arc::new(|args: &str, host: &dyn CommandHost| {
            let target = args.trim().to_string();
            async move {
                if target.is_empty() {
                    let id = host.current_model().await;
                    return CommandResult::Print(format!("Current model: {id}"));
                }
                match host.switch_model(&target).await {
                    Ok(info) => CommandResult::Print(format!(
                        "Switched: {} → {}",
                        info.previous, info.current
                    )),
                    Err(e) => CommandResult::Error(format!("model switch failed: {e}")),
                }
            }
            .boxed()
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};
    use std::sync::{Arc, Mutex};

    use crate::dispatcher::DispatcherBuilder;
    use crate::types::RouteOutcome;

    struct H {
        current: Mutex<String>,
        switch: Result<(), &'static str>,
    }
    #[async_trait]
    impl CommandHost for H {
        async fn clear_session(&self, _: &SessionKey) {}
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            self.current.lock().unwrap().clone()
        }
        async fn switch_model(&self, new: &str) -> Result<ModelSwitchInfo, String> {
            match self.switch {
                Ok(()) => {
                    let mut g = self.current.lock().unwrap();
                    let prev = g.clone();
                    *g = new.to_string();
                    Ok(ModelSwitchInfo {
                        previous: prev,
                        current: new.into(),
                    })
                }
                Err(msg) => Err(msg.into()),
            }
        }
    }

    fn host_ok(initial: &str) -> Arc<H> {
        Arc::new(H {
            current: Mutex::new(initial.into()),
            switch: Ok(()),
        })
    }

    #[tokio::test]
    async fn no_args_shows_current() {
        let d = DispatcherBuilder::new()
            .host(host_ok("openai/gpt-4.1"))
            .register_builtin(model())
            .build()
            .await
            .unwrap();
        match d.route("/model").await {
            RouteOutcome::Handled(CommandResult::Print(s)) => {
                assert!(s.contains("openai/gpt-4.1"));
            }
            other => panic!("{:?}", other),
        }
    }

    #[tokio::test]
    async fn args_switches_model() {
        let d = DispatcherBuilder::new()
            .host(host_ok("openai/gpt-4.1"))
            .register_builtin(model())
            .build()
            .await
            .unwrap();
        match d.route("/model anthropic/claude-4.5-sonnet").await {
            RouteOutcome::Handled(CommandResult::Print(s)) => {
                assert!(s.contains("openai/gpt-4.1"));
                assert!(s.contains("anthropic/claude-4.5-sonnet"));
                assert!(s.contains("→"));
            }
            other => panic!("{:?}", other),
        }
    }

    #[tokio::test]
    async fn switch_error_yields_error_result() {
        let host = Arc::new(H {
            current: Mutex::new("a".into()),
            switch: Err("unknown model"),
        });
        let d = DispatcherBuilder::new()
            .host(host)
            .register_builtin(model())
            .build()
            .await
            .unwrap();
        match d.route("/model bogus").await {
            RouteOutcome::Handled(CommandResult::Error(s)) => {
                assert!(s.contains("unknown model"));
            }
            other => panic!("{:?}", other),
        }
    }
}
```

- [ ] **Step 2: Update `builtins/mod.rs`**

```rust
pub mod model;
pub use model::model;
```

- [ ] **Step 3: Verify**

```bash
cargo test -p gasket-command --lib builtins::model
```

Expected: 3 passed.

- [ ] **Step 4: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): add /model built-in

No args shows current model via host.current_model. With arg, calls
host.switch_model and prints 'previous → current'. Error from host
becomes CommandResult::Error.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §5.1

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 17 — Reedline completer

**What:** Implement `CommandCompleter` that suggests command names when the input begins with `/`.

**Why:** Spec §4.4. Tab completion in the CLI REPL.

**Where:**
- Create: `gasket/command/src/completer.rs`
- Modify: `gasket/command/src/lib.rs`
- Modify: `gasket/command/Cargo.toml` (add `reedline` dep)

**How:** Reedline's `Completer` trait. The completer holds a `Vec<String>` of every canonical name and alias, each prefixed with `/`. On non-slash input, return empty.

**Test Case & Acceptance Criteria:**
- 3 tests: matches a prefix, non-slash input returns nothing, alias is included.

- [ ] **Step 1: Add `reedline` dependency**

In `gasket/command/Cargo.toml`:

```toml
reedline = { workspace = true }
```

If reedline is not already a workspace dep, add it (the CLI crate already uses it; check `gasket/cli/Cargo.toml` for the version and reuse it).

- [ ] **Step 2: Write the completer + tests**

Create `gasket/command/src/completer.rs`:

```rust
//! Reedline tab completion for slash commands.

use reedline::{Completer, Span, Suggestion};

use crate::Dispatcher;

pub struct CommandCompleter {
    candidates: Vec<String>,
}

impl CommandCompleter {
    pub fn from_dispatcher(d: &Dispatcher) -> Self {
        let mut candidates: Vec<String> = Vec::new();
        for cmd in d.list_commands() {
            candidates.push(format!("/{}", cmd.name));
            for a in &cmd.aliases {
                candidates.push(format!("/{}", a));
            }
        }
        candidates.sort();
        candidates.dedup();
        Self { candidates }
    }
}

impl Completer for CommandCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        if !line.starts_with('/') {
            return vec![];
        }
        let prefix = &line[..pos];
        self.candidates
            .iter()
            .filter(|c| c.starts_with(prefix))
            .map(|c| Suggestion {
                value: c.clone(),
                description: None,
                style: None,
                extra: None,
                span: Span::new(0, pos),
                append_whitespace: true,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::{clear, exit, sessions};
    use crate::dispatcher::DispatcherBuilder;
    use crate::host::CommandHost;
    use async_trait::async_trait;
    use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};
    use std::sync::Arc;

    struct H;
    #[async_trait]
    impl CommandHost for H {
        async fn clear_session(&self, _: &SessionKey) {}
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            "m".into()
        }
        async fn switch_model(&self, _: &str) -> Result<ModelSwitchInfo, String> {
            Ok(ModelSwitchInfo {
                previous: "m".into(),
                current: "m".into(),
            })
        }
    }

    async fn make() -> Dispatcher {
        DispatcherBuilder::new()
            .host(Arc::new(H))
            .register_builtin(exit())
            .register_builtin(clear())
            .register_builtin(sessions())
            .build()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn suggests_matching_prefix() {
        let d = make().await;
        let mut c = CommandCompleter::from_dispatcher(&d);
        let suggestions: Vec<String> = c
            .complete("/cl", 3)
            .into_iter()
            .map(|s| s.value)
            .collect();
        assert!(suggestions.contains(&"/clear".to_string()));
    }

    #[tokio::test]
    async fn no_suggestions_for_plain_text() {
        let d = make().await;
        let mut c = CommandCompleter::from_dispatcher(&d);
        let suggestions = c.complete("clear", 5);
        assert!(suggestions.is_empty());
    }

    #[tokio::test]
    async fn aliases_are_suggested() {
        let d = make().await;
        let mut c = CommandCompleter::from_dispatcher(&d);
        let suggestions: Vec<String> = c
            .complete("/q", 2)
            .into_iter()
            .map(|s| s.value)
            .collect();
        // /q is itself an alias for /exit
        assert!(suggestions.iter().any(|s| s == "/q" || s == "/quit"));
    }
}
```

- [ ] **Step 3: Wire into `lib.rs`**

Add:

```rust
pub mod completer;
pub use completer::CommandCompleter;
```

- [ ] **Step 4: Verify**

```bash
cargo test -p gasket-command --lib completer
```

Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add gasket/command/
git commit -m "feat(command): add Reedline tab completer

CommandCompleter::from_dispatcher captures every canonical name and
alias prefixed with /. Activates only on slash input; non-slash lines
yield to the default completer.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §4.4

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 18 — Engine: extend `process_direct` with `tool_filter`

**What:** Add an `Option<Vec<String>>` parameter to `AgentSession::process_direct` and `process_direct_streaming_with_channel`. Update all four call sites to pass `None`. Plumb the filter into `KernelConfig`. (Actual filtering happens in Task 19.)

**Why:** Spec §7.1. Day-1 the parameter is wired up but inert; existing behavior is unchanged.

**Where:**
- Modify: `gasket/engine/src/session/mod.rs` (signatures + threading; lines around 426 and 446)
- Modify: `gasket/engine/src/kernel/context.rs` (KernelConfig field)
- Modify: `gasket/engine/src/bus_adapter.rs:39` and `:67` (pass `None`)
- Modify: `gasket/cli/src/commands/agent.rs` (lines 277, 302, 356, 392 — pass `None`)

**How:** Add `tool_filter: Option<Vec<String>>` to `KernelConfig` with default `None`. Add the same parameter to both `AgentSession` methods. In `process_direct_streaming_with_channel`, set `kernel_config.tool_filter = tool_filter.clone()` before calling the kernel. Update all callers.

**Test Case & Acceptance Criteria:**
- `cargo build --workspace` passes.
- All existing tests still pass — no behavior change yet.

- [ ] **Step 1: Add the field to `KernelConfig`**

Edit `gasket/engine/src/kernel/context.rs`. Add to `KernelConfig`:

```rust
#[derive(Clone)]
pub struct KernelConfig {
    pub model: String,
    pub max_iterations: u32,
    pub max_retries: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_tool_result_chars: usize,
    pub thinking_enabled: bool,
    pub tool_timeout_secs: u64,
    pub ws_summary_limit: usize,
    /// Optional whitelist of tool names visible to the LLM for this run.
    /// `None` exposes all registered tools.
    pub tool_filter: Option<Vec<String>>,
}
```

Update `KernelConfig::new`:

```rust
impl KernelConfig {
    pub fn new(model: String) -> Self {
        Self {
            model,
            max_iterations: 100,
            max_retries: 3,
            temperature: 1.0,
            max_tokens: 65536,
            max_tool_result_chars: 16000,
            thinking_enabled: false,
            tool_timeout_secs: 120,
            ws_summary_limit: 0,
            tool_filter: None,
        }
    }
}
```

- [ ] **Step 2: Extend `AgentSession::process_direct`**

In `gasket/engine/src/session/mod.rs` around line 426:

```rust
pub async fn process_direct(
    &self,
    content: &str,
    session_key: &SessionKey,
    tool_filter: Option<Vec<String>>,
) -> Result<AgentResponse, AgentError> {
    let (_event_rx, handle) = self
        .process_direct_streaming_with_channel(content, session_key, tool_filter)
        .await?;
    handle
        .await
        .map_err(|e| AgentError::SessionError(format!("Task join error: {}", e)))?
}
```

- [ ] **Step 3: Extend `process_direct_streaming_with_channel`**

Same file, around line 446:

```rust
pub async fn process_direct_streaming_with_channel(
    &self,
    content: &str,
    session_key: &SessionKey,
    tool_filter: Option<Vec<String>>,
) -> Result<
    (
        tokio::sync::mpsc::Receiver<ChatEvent>,
        tokio::task::JoinHandle<Result<AgentResponse, AgentError>>,
    ),
    AgentError,
> {
    let (mut ctx, aborted) = self.preprocess(content, session_key).await?;
    // ... existing body ...
```

Find the spot inside this method where `KernelConfig` is constructed or read for the kernel call, and inject:

```rust
ctx.kernel_config.tool_filter = tool_filter;
```

(The exact handle for `kernel_config` depends on the surrounding code. If the field name is different — e.g. `ctx.config` — adjust. Search for `KernelConfig` in this file to locate it.)

- [ ] **Step 4: Update all callers to pass `None`**

In each of these locations, append `None` as the new argument:

- `gasket/engine/src/bus_adapter.rs:39`:
  ```rust
  .process_direct(message, session_key, None)
  ```
- `gasket/engine/src/bus_adapter.rs:67`:
  ```rust
  .process_direct_streaming_with_channel(message, session_key, None)
  ```
- `gasket/cli/src/commands/agent.rs:277`:
  ```rust
  .process_direct_streaming_with_channel(&msg, &session_key, None)
  ```
- `gasket/cli/src/commands/agent.rs:302`:
  ```rust
  let response = agent.process_direct(&msg, &session_key, None).await?;
  ```
- `gasket/cli/src/commands/agent.rs:356`:
  ```rust
  .process_direct_streaming_with_channel(line, &interactive_session, None)
  ```
- `gasket/cli/src/commands/agent.rs:392`:
  ```rust
  match agent.process_direct(line, &interactive_session, None).await
  ```

If grep finds additional callers added since this plan was written, update them too.

- [ ] **Step 5: Verify build and tests**

```bash
cargo build --workspace
cargo test --workspace
```

Both must succeed. No behavior change yet — `tool_filter` is set on `KernelConfig` but nothing reads it.

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/ gasket/cli/
git commit -m "feat(engine): add optional tool_filter parameter to process_direct

process_direct and process_direct_streaming_with_channel accept an
Option<Vec<String>> that is plumbed into KernelConfig. All four call
sites pass None to preserve existing behavior. The filter is wired but
not yet enforced — that lands in the next commit.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §7.1

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 19 — Engine: actually filter the tool list (TDD)

**What:** Make `KernelConfig::tool_filter` take effect when `RequestHandler::build_chat_request` assembles the chat request.

**Why:** Spec §7.2. Without this, Task 18 changed only signatures, not behavior. After this, YAML commands with `allowed_tools` actually constrain the LLM.

**Where:**
- Modify: `gasket/engine/src/tools/registry.rs` (add `get_definitions_filtered`)
- Modify: `gasket/engine/src/kernel/request_handler.rs` (line 34 area)

**How:** Add a sibling method to `ToolRegistry::get_definitions` that takes `Option<&[String]>`. `None` returns the existing list. `Some(set)` returns only definitions whose name is in `set`. Use this in `build_chat_request` based on `self.config.tool_filter`. Add a unit test for `get_definitions_filtered`.

**Test Case & Acceptance Criteria:**
- `tools::registry::tests::get_definitions_filtered_*` tests pass (3 cases).
- Existing engine tests still pass.

- [ ] **Step 1: Write the failing tests**

Append to `gasket/engine/src/tools/registry.rs` `#[cfg(test)] mod tests` (create the mod if it doesn't exist). Use a minimal stub `Tool`:

```rust
#[cfg(test)]
mod filter_tests {
    use super::*;
    use crate::tools::{Tool, ToolContext};
    use async_trait::async_trait;
    use serde_json::Value;

    struct Stub(&'static str);
    #[async_trait]
    impl Tool for Stub {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "stub"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({})
        }
        async fn execute(&self, _: Value, _: &ToolContext) -> ToolResult {
            unreachable!()
        }
    }

    fn make_registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(Box::new(Stub("alpha")));
        r.register(Box::new(Stub("beta")));
        r.register(Box::new(Stub("gamma")));
        r
    }

    #[test]
    fn none_filter_returns_all() {
        let r = make_registry();
        let defs = r.get_definitions_filtered(None);
        assert_eq!(defs.len(), 3);
    }

    #[test]
    fn empty_filter_returns_none() {
        let r = make_registry();
        let defs = r.get_definitions_filtered(Some(&[]));
        assert_eq!(defs.len(), 0);
    }

    #[test]
    fn whitelist_filters_to_named() {
        let r = make_registry();
        let names = vec!["alpha".to_string(), "gamma".to_string()];
        let defs = r.get_definitions_filtered(Some(&names));
        let got: Vec<_> = defs.iter().map(|d| d.name()).collect();
        assert!(got.contains(&"alpha"));
        assert!(got.contains(&"gamma"));
        assert!(!got.contains(&"beta"));
    }
}
```

(`d.name()` may need to be adapted to the actual `ToolDefinition` accessor — inspect the type to find the right method, e.g. `&d.function.name`.)

- [ ] **Step 2: Verify the tests fail**

```bash
cargo test -p gasket-engine --lib tools::registry::filter_tests
```

Expected: compile failure ("no method `get_definitions_filtered`").

- [ ] **Step 3: Implement the filter method**

Add to `impl ToolRegistry`:

```rust
/// Like `get_definitions` but optionally filters by an allowlist.
///
/// `None` returns every registered tool (same as `get_definitions`).
/// `Some(slice)` returns only those whose name appears in the slice.
pub fn get_definitions_filtered(&self, filter: Option<&[String]>) -> Vec<ToolDefinition> {
    self.items
        .iter()
        .filter(|(name, _)| match filter {
            None => true,
            Some(set) => set.iter().any(|s| s == *name),
        })
        .map(|(_, entry)| {
            ToolDefinition::function(
                entry.tool.name(),
                entry.tool.description(),
                entry.tool.parameters(),
            )
        })
        .collect()
}
```

- [ ] **Step 4: Apply the filter in `RequestHandler::build_chat_request`**

Edit `gasket/engine/src/kernel/request_handler.rs`. Change the line `tools: Some(self.tools.get_definitions()),` to:

```rust
tools: Some(
    self.tools.get_definitions_filtered(self.config.tool_filter.as_deref()),
),
```

- [ ] **Step 5: Verify**

```bash
cargo test -p gasket-engine --lib tools::registry::filter_tests
cargo test --workspace
```

Expected: 3 new tests pass; nothing else regressed.

- [ ] **Step 6: Commit**

```bash
git add gasket/engine/
git commit -m "feat(engine): filter tool spec list by KernelConfig.tool_filter

ToolRegistry::get_definitions_filtered honours the optional allowlist;
RequestHandler::build_chat_request invokes it. None preserves the
existing 'all tools' behavior. With the previous commit, YAML commands
that declare allowed_tools now actually constrain the LLM.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §7.2

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 20 — `CliCommandHost` impl

**What:** Add a CLI-side struct that implements `CommandHost` by delegating to `AgentSession`.

**Why:** Spec §3.4. Without this, the dispatcher can't be constructed in the CLI.

**Where:**
- Create: `gasket/cli/src/commands/command_host.rs`
- Modify: `gasket/cli/src/commands/mod.rs`
- Modify: `gasket/cli/Cargo.toml` (add `gasket-command` dep)

**How:** Hold an `Arc<AgentSession>`. Implement the four methods. `current_model` calls `agent.model().to_string()`. `clear_session` calls `agent.clear_session(key)` (already exists). `list_sessions` and `switch_model` are not yet exposed on `AgentSession` — see steps below for the small additions on the engine side.

**Test Case & Acceptance Criteria:**
- `cargo build -p gasket-cli` passes.
- A unit test in `command_host.rs` instantiates `CliCommandHost` and queries `current_model`.

- [ ] **Step 1: Confirm or add `AgentSession::list_sessions` and `AgentSession::switch_model`**

Search `gasket/engine/src/session/mod.rs` for `list_sessions` and `switch_model`. If they exist, note their signatures. Otherwise:

`list_sessions`: read from `EventStore` to enumerate distinct session keys. A pragmatic minimal impl is to add:

```rust
pub async fn list_sessions(&self) -> Vec<gasket_types::SessionSummary> {
    self.context_builder
        .event_store()
        .list_sessions()
        .await
        .into_iter()
        .map(|s| gasket_types::SessionSummary {
            key: s.key,
            message_count: s.message_count,
            last_active: s.last_active,
        })
        .collect()
}
```

If `EventStore::list_sessions` does not exist, day-1 acceptable fallback: return only the current interactive session via `vec![SessionSummary { key: <interactive>, message_count: 0, last_active: None }]`. Note this in the commit message and open a follow-up task for full enumeration.

`switch_model`: minimal version that mutates `self.config.model`:

```rust
pub async fn switch_model(&self, new: &str) -> Result<gasket_types::ModelSwitchInfo, String> {
    // For day-1, we accept any model id without validating against the registry.
    // Validation is a follow-up.
    let previous = self.config.model.clone();
    // self.config is currently &KernelConfig owned; if it's behind &self,
    // wrap the field in Arc<Mutex<>> as part of this task. If wrapping is
    // too invasive, return Err with a clear message and gate /model on it.
    // ...
}
```

If `KernelConfig` is held by value behind `&self` and there is no interior mutability, the cleanest day-1 fix is to return `Err("model switch not supported in this build".into())` and treat `/model <id>` as a no-op with a clear error. The `/model` (no-arg) path still works. Capture the limitation in the commit message and a follow-up task. **Do not refactor session-wide state for this task.**

- [ ] **Step 2: Add `gasket-command` as a CLI dependency**

In `gasket/cli/Cargo.toml`:

```toml
gasket-command = { path = "../command" }
```

- [ ] **Step 3: Implement `CliCommandHost`**

Create `gasket/cli/src/commands/command_host.rs`:

```rust
//! Bridge from `gasket-command::CommandHost` to `AgentSession`.

use std::sync::Arc;

use async_trait::async_trait;
use gasket_command::CommandHost;
use gasket_engine::session::AgentSession;
use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};

pub struct CliCommandHost {
    pub agent: Arc<AgentSession>,
}

impl CliCommandHost {
    pub fn new(agent: Arc<AgentSession>) -> Self {
        Self { agent }
    }
}

#[async_trait]
impl CommandHost for CliCommandHost {
    async fn clear_session(&self, key: &SessionKey) {
        self.agent.clear_session(key).await;
    }

    async fn list_sessions(&self) -> Vec<SessionSummary> {
        self.agent.list_sessions().await
    }

    async fn current_model(&self) -> String {
        self.agent.model().to_string()
    }

    async fn switch_model(&self, new: &str) -> Result<ModelSwitchInfo, String> {
        self.agent.switch_model(new).await
    }
}
```

- [ ] **Step 4: Wire into `commands/mod.rs`**

Add:

```rust
pub mod command_host;
```

- [ ] **Step 5: Verify build**

```bash
cargo build -p gasket-cli
cargo build --workspace
```

Both must succeed.

- [ ] **Step 6: Commit**

```bash
git add gasket/cli/ gasket/engine/
git commit -m "feat(cli): add CliCommandHost adapter

CliCommandHost delegates each CommandHost method to AgentSession. The
adapter is the only place where gasket-command meets gasket-engine.

If list_sessions/switch_model required new AgentSession methods, those
land here too with a note in this commit body about any temporary
limitations.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §3.4

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 21 — Wire dispatcher into the CLI REPL

**What:** Replace the hardcoded `/help /new /exit` if/else in `agent.rs` (around lines 328-353) with a real dispatcher. Wire the completer into Reedline.

**Why:** This is the user-facing change that makes everything in this plan visible. Spec §1, §2.2, §4.5.

**Where:**
- Modify: `gasket/cli/src/commands/agent.rs`

**How:** Construct the dispatcher once before the REPL loop. For each line, call `dispatcher.route(line)`. Match on `RouteOutcome`. `Handled(Quit)` breaks the loop. `Handled(Print/Error)` prints. `Rewrite { prompt, tool_filter }` calls the existing streaming/non-streaming helpers with the `prompt` and `tool_filter` instead of the raw line. `Passthrough(line)` does the same with `tool_filter = None`.

**Test Case & Acceptance Criteria:**
- Manual smoke test of every built-in (`/help`, `/clear`, `/exit`, `/new`, `/sessions`, `/model`).
- A user YAML file at `~/.gasket/commands/translate.md` produces a Rewrite that the LLM responds to.
- Existing plain-text conversation behavior is unchanged.

- [ ] **Step 1: Build the dispatcher in the interactive arm**

Find the interactive-mode branch in `cmd_agent` (the one starting around line 308 after `// Interactive mode`). Before the `loop {`, add:

```rust
use std::sync::Arc;

use gasket_command::builtins::{clear, exit, help, model, new as builtin_new, sessions};
use gasket_command::dispatcher::shared_help_snapshot;
use gasket_command::{CommandCompleter, CommandResult, DispatcherBuilder, RouteOutcome};

use crate::commands::command_host::CliCommandHost;

let host = Arc::new(CliCommandHost::new(agent.clone()));
let session_key_for_new = Arc::new(interactive_session.clone());
let help_snap = shared_help_snapshot();

let user_dir = dirs::home_dir()
    .map(|h| h.join(".gasket/commands"));

let mut builder = DispatcherBuilder::new()
    .host(host)
    .help_snapshot(help_snap.clone())
    .register_builtin(exit())
    .register_builtin(clear())
    .register_builtin(help(help_snap.clone()))
    .register_builtin(builtin_new(session_key_for_new.clone()))
    .register_builtin(sessions())
    .register_builtin(model());
if let Some(p) = user_dir {
    builder = builder.user_dir(p);
}
let dispatcher = builder.build().await.expect("dispatcher build failed");
```

If `dirs` is not yet a dependency of the CLI crate, check (`grep dirs gasket/cli/Cargo.toml`); if missing, add it from workspace deps.

- [ ] **Step 2: Replace Reedline construction with completer-aware version**

Find:

```rust
let mut line_editor = Reedline::create();
```

Replace with:

```rust
let completer = CommandCompleter::from_dispatcher(&dispatcher);
let mut line_editor = Reedline::create()
    .with_completer(Box::new(completer));
```

- [ ] **Step 3: Replace the hardcoded slash if/else block**

Inside the loop, find the current handling around lines 328-353:

```rust
// Handle CLI-specific slash commands locally
let cmd = line.to_lowercase();
if cmd == "/new" { ... continue; }
if cmd == "/help" { ... continue; }
```

Replace the entire slash-handling block with:

```rust
match dispatcher.route(line).await {
    RouteOutcome::Handled(CommandResult::Quit) => {
        println!("Goodbye! 🐈");
        break;
    }
    RouteOutcome::Handled(CommandResult::Print(s)) => {
        println!("{}", s);
        continue;
    }
    RouteOutcome::Handled(CommandResult::Error(s)) => {
        eprintln!("{}", s.red());
        continue;
    }
    RouteOutcome::Rewrite { prompt, tool_filter } => {
        // Fall through to the LLM with the rewritten prompt.
        run_llm_input(&agent, &interactive_session, &prompt, tool_filter, use_streaming, render_md).await?;
        continue;
    }
    RouteOutcome::Passthrough(text) => {
        run_llm_input(&agent, &interactive_session, &text, None, use_streaming, render_md).await?;
        continue;
    }
}
```

Hoist the body of the existing "process the message" block (the part that calls `process_direct_streaming_with_channel` or `process_direct`) into a helper function `run_llm_input` defined in the same module:

```rust
async fn run_llm_input(
    agent: &Arc<AgentSession>,
    session_key: &SessionKey,
    text: &str,
    tool_filter: Option<Vec<String>>,
    use_streaming: bool,
    render_md: bool,
) -> Result<()> {
    if use_streaming {
        // existing streaming body, but pass `tool_filter` instead of `None`
        let streaming_result = agent
            .process_direct_streaming_with_channel(text, session_key, tool_filter.clone())
            .await;
        // ... existing body ...
    } else {
        let response = agent.process_direct(text, session_key, tool_filter).await?;
        print_response_with_reasoning(&response, render_md);
    }
    Ok(())
}
```

(The exact lifting of the existing body is mechanical; preserve all existing print/event-handling logic verbatim.)

- [ ] **Step 4: Build and run a quick smoke test**

```bash
cargo build --workspace
cargo run -p gasket-cli -- agent
```

Inside the REPL, exercise each built-in:

```
/help
/clear
/sessions
/model
/new
/exit
```

Each should produce sensible output. `/exit` should terminate.

- [ ] **Step 5: Smoke-test a user YAML command**

Create `~/.gasket/commands/translate.md`:

```markdown
---
name: translate
description: Translate text to natural Mandarin Chinese
aliases: [tr]
---

Translate the following to natural, idiomatic Mandarin Chinese.
Preserve formatting. Do not add commentary unless ambiguity exists.

{{user_input}}
```

Restart the REPL. `/help` should list `/translate`. Type `/translate Hello world`. The LLM should produce a Mandarin translation. `/tr Hello` should also work.

- [ ] **Step 6: Verify regression**

A plain conversational input (`hello there`) should behave exactly as before.

- [ ] **Step 7: Commit**

```bash
git add gasket/cli/
git commit -m "feat(cli): route REPL input through gasket-command dispatcher

Replaces the hardcoded /help /new /exit if-else with a real dispatcher.
Adds tab completion via CommandCompleter. User commands at
~/.gasket/commands/*.md are picked up at startup. Plain conversation
is unchanged. Bot channels (Telegram, Discord, Slack) are untouched.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §1, §4.5

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 22 — End-to-end integration test

**What:** Add an integration test in `gasket/command/tests/end_to_end.rs` that builds a dispatcher with the full set of built-ins and a synthetic user YAML directory, then exercises every routing path.

**Why:** Catches regressions when later changes touch the dispatcher. Lives outside the unit-test mod so it runs against the public API.

**Where:**
- Create: `gasket/command/tests/end_to_end.rs`

**How:** `tempfile::TempDir` for the user dir; an in-test `MockHost`; assertions on each `RouteOutcome` shape.

**Test Case & Acceptance Criteria:**
- 1 integration test passes covering: built-in match, alias, YAML rewrite, unknown, passthrough, /help listing.

- [ ] **Step 1: Write the test**

Create `gasket/command/tests/end_to_end.rs`:

```rust
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gasket_command::builtins::{clear, exit, help, model, new as builtin_new, sessions};
use gasket_command::dispatcher::shared_help_snapshot;
use gasket_command::{CommandHost, CommandResult, DispatcherBuilder, RouteOutcome};
use gasket_types::{ChannelType, ModelSwitchInfo, SessionKey, SessionSummary};

struct H {
    cleared: Mutex<Vec<SessionKey>>,
    current: Mutex<String>,
}

#[async_trait]
impl CommandHost for H {
    async fn clear_session(&self, k: &SessionKey) {
        self.cleared.lock().unwrap().push(k.clone());
    }
    async fn list_sessions(&self) -> Vec<SessionSummary> {
        vec![]
    }
    async fn current_model(&self) -> String {
        self.current.lock().unwrap().clone()
    }
    async fn switch_model(&self, new: &str) -> Result<ModelSwitchInfo, String> {
        let mut g = self.current.lock().unwrap();
        let prev = g.clone();
        *g = new.to_string();
        Ok(ModelSwitchInfo {
            previous: prev,
            current: new.into(),
        })
    }
}

#[tokio::test]
async fn full_dispatcher_routing_matrix() {
    let dir = tempfile::TempDir::new().unwrap();
    tokio::fs::write(
        dir.path().join("translate.md"),
        "---\n\
name: translate\n\
description: Translate to Mandarin\n\
aliases: [tr]\n\
---\n\
\n\
Translate to Mandarin: {{user_input}}\n",
    )
    .await
    .unwrap();

    let host = Arc::new(H {
        cleared: Mutex::new(vec![]),
        current: Mutex::new("openai/gpt-4.1".into()),
    });
    let snap = shared_help_snapshot();
    let key = SessionKey::new(ChannelType::Cli, "interactive");

    let d = DispatcherBuilder::new()
        .host(host.clone())
        .help_snapshot(snap.clone())
        .user_dir(dir.path().to_path_buf())
        .register_builtin(exit())
        .register_builtin(clear())
        .register_builtin(help(snap.clone()))
        .register_builtin(builtin_new(Arc::new(key.clone())))
        .register_builtin(sessions())
        .register_builtin(model())
        .build()
        .await
        .unwrap();

    // Built-in match
    assert_eq!(
        d.route("/exit").await,
        RouteOutcome::Handled(CommandResult::Quit)
    );

    // Alias
    assert_eq!(
        d.route("/q").await,
        RouteOutcome::Handled(CommandResult::Quit)
    );

    // YAML rewrite
    assert_eq!(
        d.route("/translate Hello").await,
        RouteOutcome::Rewrite {
            prompt: "Translate to Mandarin: Hello\n".into(),
            tool_filter: None,
        }
    );

    // Alias on YAML command
    assert_eq!(
        d.route("/tr World").await,
        RouteOutcome::Rewrite {
            prompt: "Translate to Mandarin: World\n".into(),
            tool_filter: None,
        }
    );

    // Unknown command
    match d.route("/whatisthis").await {
        RouteOutcome::Handled(CommandResult::Error(msg)) => {
            assert!(msg.contains("/whatisthis"));
        }
        other => panic!("{:?}", other),
    }

    // Passthrough
    assert_eq!(
        d.route("plain text").await,
        RouteOutcome::Passthrough("plain text".into())
    );

    // /help lists built-ins and the user command
    match d.route("/help").await {
        RouteOutcome::Handled(CommandResult::Print(text)) => {
            for needle in ["/exit", "/help", "/new", "/sessions", "/model", "/clear", "/translate"] {
                assert!(text.contains(needle), "expected {needle} in:\n{text}");
            }
        }
        other => panic!("{:?}", other),
    }

    // /new triggers host.clear_session
    let _ = d.route("/new").await;
    assert_eq!(host.cleared.lock().unwrap().len(), 1);

    // /model with no args shows current
    match d.route("/model").await {
        RouteOutcome::Handled(CommandResult::Print(s)) => {
            assert!(s.contains("openai/gpt-4.1"));
        }
        other => panic!("{:?}", other),
    }

    // /model <id> switches
    match d.route("/model anthropic/claude-4.5-sonnet").await {
        RouteOutcome::Handled(CommandResult::Print(s)) => {
            assert!(s.contains("openai/gpt-4.1"));
            assert!(s.contains("anthropic/claude-4.5-sonnet"));
        }
        other => panic!("{:?}", other),
    }
}
```

- [ ] **Step 2: Run the test**

```bash
cargo test -p gasket-command --test end_to_end
```

Expected: 1 passed.

- [ ] **Step 3: Run the full workspace test suite**

```bash
cargo test --workspace
```

Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add gasket/command/tests/
git commit -m "test(command): full routing-matrix integration test

End-to-end test that builds the day-1 dispatcher with a real user-dir
and exercises every RouteOutcome variant plus /help discoverability.

Refs: docs/superpowers/specs/2026-05-03-command-system-design.md §9.1

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Notes

This section is for the implementing agent to verify before declaring the
plan complete. Run through the checklist:

**Spec coverage:**

| Spec section | Implemented in |
|---|---|
| §1 Overview | Tasks 1, 21 |
| §2 Architecture (crate, channel contract) | Tasks 1, 21 |
| §3.1 Command/CommandKind/CommandResult/RouteOutcome | Task 3 |
| §3.2 RouteOutcome variants | Tasks 3, 8, 9 |
| §3.3 Dispatcher / DispatcherBuilder | Tasks 8, 11 |
| §3.4 CommandHost trait (4 methods) | Tasks 3, 5 |
| §3.5 Precedence rules | Task 11 |
| §4.1 Parser | Task 6 |
| §4.2 Lookup and dispatch | Tasks 8, 9 |
| §4.3 Template render | Task 7 |
| §4.4 Tab completion | Task 17 |
| §4.5 End-to-end trace | Task 21 (smoke), Task 22 |
| §5 Day-1 built-ins (6) | Tasks 12, 13, 14, 15, 16 |
| §5.1 Output mockups | Tasks 13, 15, 16 |
| §6 User YAML | Task 10 |
| §7 Engine touch point | Tasks 18, 19 |
| §8 Error handling, invariants | Tasks 4, 11 |
| §9 Testing layout | Tasks 6–17 unit, Task 22 integration |
| §10 Implementation order | This plan structure |

**Placeholder scan:** No `TBD`, `TODO`, "fill in details", or "similar to
above" remain. Every step shows actual code or an exact command.

**Type consistency:**

- `BuiltinHandler` signature is identical in every task.
- `CommandResult` variants used consistently: `Print`, `Quit`, `Error`.
- `RouteOutcome` variants: `Handled`, `Rewrite`, `Passthrough`.
- `tool_filter` is `Option<Vec<String>>` everywhere.
- `CommandHost` methods: `clear_session(&SessionKey)`, `list_sessions() ->
  Vec<SessionSummary>`, `current_model() -> String`, `switch_model(&str) ->
  Result<ModelSwitchInfo, String>`.

**Out-of-scope reminder (from spec §11):**
- Bot channels are not touched. The CLI is the only consumer in this plan.
- `/wiki`, `/save`, `/undo`, `/debug`, `/cost` are explicitly excluded from
  built-ins.
- Hot reload of user YAML is excluded; restart picks up changes.

If any of the above checks fail, fix inline before declaring done.
