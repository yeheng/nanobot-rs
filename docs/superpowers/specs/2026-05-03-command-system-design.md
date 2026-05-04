# Command System — Design Spec

**Date**: 2026-05-03
**Status**: Draft — pending implementation plan
**Scope**: Slash-command dispatcher for CLI (and future Web), bot channels untouched
**Persona**: Designed under the Linus Torvalds role defined in `CLAUDE.md`

---

## 1. Overview

A client-side slash-command dispatcher that replaces the hardcoded `/help /new
/exit` if-else in `gasket/cli/src/commands/agent.rs:328`. It lives in a new
`gasket-command` crate and is consumed by the CLI today and the future Web
frontend tomorrow. Bot channels (Telegram, Discord, Slack, email, etc.) keep
their current "transparent passthrough" contract — they never see the
dispatcher.

The dispatcher:
- Recognises six built-in commands written in Rust.
- Loads user-defined commands from `~/.gasket/commands/*.md` (front-matter +
  Markdown body).
- Restricts user commands to "prompt template + optional tool whitelist" — no
  scripting, no multi-step orchestration, no flow engine.
- Touches the engine API in exactly one place: `AgentSession::process_direct`
  gains an optional `tool_filter` parameter.

### 1.1 Why a Separate Crate

Putting the dispatcher inside `engine/` would expose it to channels and
providers, both of which transitively depend on engine. A bot channel could
then accidentally route through the dispatcher and break the existing
passthrough contract. The new `gasket-command` crate depends only on
`gasket-types`. CLI and future Web depend on it; engine does not. The boundary
is enforced by the build graph, not by discipline.

### 1.2 Why Not Resume the Flow Command System

The deleted `2026-05-03-flow-command-system-design.md` mixed three orthogonal
concerns: a wiki write guard, a slash-command dispatcher, and a multi-phase
flow engine. This spec narrows scope to **only the dispatcher** (the "B layer"
identified during brainstorming). The other two concerns can each be designed
separately when their own real demand surfaces.

### 1.3 Linus-Style Decision Summary

| Layer | Question | Answer |
|---|---|---|
| Real or imagined? | Hardcoded `/help /new /exit` in CLI is fragile, blocks user-extensibility | Real |
| Simpler alternative? | Keep growing the if/else; no extension story | Worse, not simpler |
| Breaking changes? | Bot channels keep passthrough; engine API gains an `Option` parameter with `None` default | Effectively zero |

---

## 2. Architecture

### 2.1 Crate Topology

```
workspace
├── types          (no change, gains SessionSummary / ModelSwitchInfo)
├── storage        (no change)
├── engine         ★ adds optional tool_filter parameter on process_direct
├── command   ★NEW (Dispatcher / Registry / Handlers / YamlLoader)
├── cli            ★ Reedline loop routes through dispatcher
├── channels       (no change — bot channels keep passthrough)
├── providers      (no change)
└── web   (future) will depend on gasket-command
```

Dependency edges:

- `command → types`
- `cli → command, engine, types`
- `engine → types, storage, ...`
- `command` does **not** depend on `engine`. Built-in handlers that need
  engine capabilities receive them via a `CommandHost` trait whose
  implementation lives in CLI (and later Web).

### 2.2 User Input Path

```
                ┌──────────────────┐
                │  CLI Reedline    │
                │   read_line()    │
                └────────┬─────────┘
                         │ String
                ┌────────▼─────────┐
                │  Dispatcher      │   ← gasket-command crate
                │  ::route(line)   │
                └────────┬─────────┘
                         │
              ┌──────────┴──────────┐
              │ starts with "/"?    │
              └──────────┬──────────┘
                ┌────────▼─────────┐
                │   yes → lookup    │
                │   no  → Passthrough│
                └────┬─────────┬───┘
                     │         │
       ┌─────────────▼──┐   ┌──▼──────────────────┐
       │ Handler runs    │   │ AgentSession        │
       │ (local / host / │   │ ::process_direct()  │
       │  prompt rewrite)│   │ existing path       │
       └────────────────┘   └─────────────────────┘
```

`Dispatcher::route()` returns `RouteOutcome`, never `Err`. Three variants:

- `Handled(CommandResult)` — handler is done; CLI just renders the result.
- `Rewrite { prompt, tool_filter }` — YAML command rewrote the input; CLI
  calls `process_direct(prompt, key, tool_filter)`.
- `Passthrough(line)` — not a command; CLI sends the line to the LLM as
  before.

### 2.3 Channel Contract

| Channel | Current behavior | After this spec |
|---|---|---|
| CLI (Reedline) | Hardcoded `/help /new /exit` | Goes through dispatcher |
| Web (TBD) | Not implemented | Will use dispatcher when wired up |
| Telegram | Passes `/start` etc. to LLM | **Unchanged** |
| Discord / Slack / others | Passthrough | **Unchanged** |

Bot channel crates have no path to import `gasket-command`. The boundary is
architectural.

---

## 3. Data Structures

### 3.1 Command and Handler

```rust
// gasket/command/src/types.rs

pub struct Command {
    pub name: String,                  // canonical, no leading "/"
    pub description: String,           // one-liner for /help
    pub aliases: Vec<String>,          // each without leading "/"
    pub kind: CommandKind,
}

pub enum CommandKind {
    /// Built-in: a Rust function, registered in code.
    Builtin(BuiltinHandler),

    /// User YAML: prompt template + optional tool whitelist.
    /// On match, dispatcher does not call the LLM; it produces RouteOutcome::Rewrite.
    Yaml {
        prompt_template: String,
        allowed_tools: Option<Vec<String>>,
    },
}

pub type BuiltinHandler = Arc<
    dyn for<'a> Fn(&'a str, &'a dyn CommandHost)
            -> BoxFuture<'a, CommandResult>
        + Send + Sync,
>;
```

**Linus simplifications applied:**

| Not introduced | Why |
|---|---|
| `enum HandlerKind { Sync, Async }` | All handlers are async; sync code wraps in `async move {}` |
| `trait Command` with `Arc<dyn Command>` | Static enum is enough; jump-to-def in IDE works |
| Version field, ACL, dynamic registration | YAGNI |

### 3.2 Routing Result

```rust
pub enum RouteOutcome {
    Handled(CommandResult),
    Rewrite { prompt: String, tool_filter: Option<Vec<String>> },
    Passthrough(String),
}

pub enum CommandResult {
    Print(String),       // ordinary text output
    Quit,                // CLI terminates the REPL
    Error(String),       // print as error, REPL continues
}
```

Key invariant: `Dispatcher::route()` always returns `RouteOutcome`, never
`Result`. Errors visible to the user are encoded as
`CommandResult::Error(...)`. This keeps the CLI dispatch site to a single
match without a second `Err` arm.

### 3.3 Dispatcher

```rust
pub struct Dispatcher {
    commands: HashMap<String, Arc<Command>>,    // canonical name → Command
    aliases: HashMap<String, String>,           // alias → canonical
    host: Arc<dyn CommandHost>,
}

impl Dispatcher {
    pub fn builder() -> DispatcherBuilder { ... }
    pub async fn route(&self, line: &str) -> RouteOutcome { ... }
    pub fn list_commands(&self) -> Vec<&Command> { ... }    // for /help
}

pub struct DispatcherBuilder {
    builtins: Vec<Command>,
    user_yaml_dir: Option<PathBuf>,             // typically ~/.gasket/commands
    host: Option<Arc<dyn CommandHost>>,
}

impl DispatcherBuilder {
    pub fn register_builtin(self, cmd: Command) -> Self { ... }
    pub fn user_dir(self, p: PathBuf) -> Self { ... }
    pub fn host(self, h: Arc<dyn CommandHost>) -> Self { ... }
    pub fn build(self) -> Result<Dispatcher, BuildError> { ... }
}
```

`build()` walks `user_yaml_dir`, parses each `*.md`, and merges results into
`commands`/`aliases` according to §3.5 precedence rules.

### 3.4 CommandHost Trait

```rust
// gasket/command/src/host.rs

#[async_trait]
pub trait CommandHost: Send + Sync {
    async fn clear_session(&self, key: &SessionKey);
    async fn list_sessions(&self) -> Vec<SessionSummary>;
    async fn current_model(&self) -> String;
    async fn switch_model(&self, new: &str) -> Result<ModelSwitchInfo, String>;
}
```

`SessionSummary` and `ModelSwitchInfo` live in `gasket-types`, so
`gasket-command` does not reach into engine. The CLI provides a
`CliCommandHost { agent: Arc<AgentSession> }` that forwards each method to the
existing `AgentSession` API.

Four methods at day-1: three for `/sessions /new /model` plus
`current_model` so `/model` with no argument can print the active model
without needing to invoke `switch_model`.

The trait surface grows only when a new built-in actually requires a method.
No preemptive additions.

### 3.5 Precedence Rules

At `DispatcherBuilder::build()` time:

- Built-in canonical name always wins.
- Built-in alias colliding with a user YAML name → built-in wins, `warn!`.
- Two user YAMLs with the same name → first scanned wins, `warn!`. Scan order
  is lexicographic by file path, so the result is deterministic across runs.
- All collisions log a warning; none are fatal.

This is the client-side translation of "never break userspace": a malformed
user YAML cannot prevent the REPL from starting.

---

## 4. Parsing, Lookup, Templating

### 4.1 Parser

```rust
// gasket/command/src/parser.rs

pub enum ParsedInput<'a> {
    Command { name: &'a str, args: &'a str },
    NotCommand,
}

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

Driven test cases:

| Input | Expected |
|---|---|
| `"/help"` | `Command{name:"help", args:""}` |
| `"/translate hello world"` | `Command{name:"translate", args:"hello world"}` |
| `"/translate   hello   world  "` | `Command{name:"translate", args:"hello   world"}` |
| `"  /help"` | `Command{name:"help", args:""}` |
| `"/"` | `NotCommand` |
| `"/  "` | `NotCommand` |
| `""` | `NotCommand` |
| `"hello"` | `NotCommand` |
| `"//cmd"` | `Command{name:"/cmd", args:""}` (lookup fails → unknown command Error) |

The parser deliberately does **not** support quoted args, `--flag`-style
keywords, or nested subcommands. Each handler is free to apply its own
parsing (e.g. clap) to `args` if needed.

### 4.2 Lookup and Dispatch

```rust
async fn dispatch(&self, name: &str, args: &str) -> RouteOutcome {
    let canonical = self.aliases.get(name).map(String::as_str).unwrap_or(name);

    let Some(cmd) = self.commands.get(canonical) else {
        return RouteOutcome::Handled(CommandResult::Error(
            format!("unknown command: /{name}    (type /help to see commands)"),
        ));
    };

    match &cmd.kind {
        CommandKind::Builtin(handler) => {
            RouteOutcome::Handled(handler(args, self.host.as_ref()).await)
        }
        CommandKind::Yaml { prompt_template, allowed_tools } => {
            RouteOutcome::Rewrite {
                prompt: render_template(prompt_template, args),
                tool_filter: allowed_tools.clone(),
            }
        }
    }
}
```

### 4.3 Template Rendering

```rust
// gasket/command/src/template.rs

pub fn render(template: &str, user_input: &str) -> String {
    template.replace("{{user_input}}", user_input)
}
```

Day-1 supports exactly one placeholder: `{{user_input}}`. No template-engine
dependency. No conditionals. Unknown placeholders pass through verbatim to the
LLM. Adding a second variable later is mechanical (`replace` → small
hashmap-driven loop) and not worth premature abstraction.

### 4.4 Tab Completion

```rust
// gasket/command/src/completer.rs

pub struct CommandCompleter {
    candidates: Vec<String>,    // canonical names + aliases, all prefixed with "/"
}

impl reedline::Completer for CommandCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<reedline::Suggestion> {
        if !line.starts_with('/') {
            return vec![];
        }
        // filter candidates that start with line[..pos]
    }
}
```

Activates only when the input starts with `/`; otherwise yields to the
default completer.

### 4.5 End-to-End Trace

User types `/translate Hello world`, where `~/.gasket/commands/translate.md`
declares `allowed_tools: []` (empty whitelist = forbid all tools):

1. `agent.rs` reads the line via Reedline.
2. `dispatcher.route(line).await`.
3. Parser returns `Command { name: "translate", args: "Hello world" }`.
4. `dispatch` looks up `commands["translate"]` (a `Yaml` kind).
5. `render_template` produces the rendered prompt body (the markdown body
   of the `.md` file with `{{user_input}}` replaced by `"Hello world"`).
6. Returns `RouteOutcome::Rewrite { prompt, tool_filter: Some(vec![]) }`.
7. `agent.rs` calls `agent.process_direct(prompt, &key, Some(vec![])).await`.
8. The kernel sees an empty tool whitelist for this single invocation, so
   no tools are exposed to the LLM. Existing LLM streaming path handles
   the rest. Bot channels never see the rewrite.

Had `translate.md` omitted `allowed_tools` entirely, step 6 would emit
`tool_filter: None` and step 7 would pass `None`, preserving the kernel's
default tool set.

---

## 5. Day-1 Built-in Commands

Six commands, scoped to "session state, REPL state, metadata listing".
Anything that calls the LLM with a custom prompt is a YAML command, not a
built-in.

| Canonical | Aliases | Behavior |
|---|---|---|
| `/help` | `/?` | List all commands (built-in and user), sorted, with description |
| `/exit` | `/quit`, `/q`, `:q` | Return `CommandResult::Quit`; CLI terminates the REPL |
| `/new` | — | Call `host.clear_session(key)`; print "Session cleared" |
| `/sessions` | `/ls` | Call `host.list_sessions()`; render as a table |
| `/model` | — | No args → print current; with arg → call `host.switch_model(id)` |
| `/clear` | — | Print ANSI clear-screen; no engine call |

### 5.1 Output Mockups

`/help`:

```
Built-in commands:
  /clear        Clear the terminal screen
  /exit         Exit the REPL  (aliases: /quit, /q, :q)
  /help         Show available commands  (aliases: /?)
  /model        Show or switch the active model
  /new          Start a new conversation
  /sessions     List recent sessions  (aliases: /ls)

User commands  (~/.gasket/commands):
  /explain      Explain a concept concisely
  /review       Linus-style code review
  /translate    Translate to Mandarin Chinese  (aliases: /tr)
```

`/sessions`:

```
SESSION KEY                    MESSAGES   LAST ACTIVE
cli/interactive                42         3 minutes ago
telegram/123456789             7          2 hours ago
discord/987654321/general      0          —
```

`/model` (no args): `Current model: openrouter/anthropic/claude-4.5-sonnet`

`/model openai/gpt-4.1`:
`Switched: openrouter/anthropic/claude-4.5-sonnet → openai/gpt-4.1`

`/new`: `✓ Session cleared (cli/interactive)`

`/clear`: prints `\x1B[2J\x1B[H`.

### 5.2 Excluded From Day-1 Built-ins

| Candidate | Reason for exclusion |
|---|---|
| `/wiki` | `gasket wiki` is already a clap subcommand. Bringing it into the REPL means lifting clap into the dispatcher. Day-2. |
| `/save` | Sessions are already SqliteStore-persisted. Solving a non-problem. |
| `/undo` | No reversible LLM API. |
| `/debug` | `RUST_LOG` env var already controls verbosity; restart suffices. |
| `/cost` | Token tracking exists in engine; surfacing in the REPL is a separate UX concern. Day-2. |

---

## 6. User YAML Commands

### 6.1 File Layout

`~/.gasket/commands/<anything>.md` — front-matter YAML + Markdown body.

```markdown
---
name: translate
description: Translate text to natural Mandarin Chinese
aliases: [tr]
allowed_tools: []
---

Translate the following to natural, idiomatic Mandarin Chinese.
Preserve formatting. Do not add commentary unless ambiguity exists.

{{user_input}}
```

### 6.2 Schema

| Field | Type | Default | Meaning |
|---|---|---|---|
| `name` | String | required | Canonical name, no leading `/` |
| `description` | String | required | One-liner for `/help` |
| `aliases` | `Vec<String>` | `[]` | No leading `/`; empty = none |
| `allowed_tools` | `Option<Vec<String>>` | absent | absent = unrestricted; `[]` = no tools at all; `[...]` = whitelist |

The `Option<Vec> + empty array` distinction is intentional: absent = "no
opinion, default behavior", `[]` = "explicitly forbid all tools". Absence-as-
meaning, no magic sentinels.

The body (everything after the second `---`) is the prompt template. The only
placeholder Day-1 is `{{user_input}}`.

### 6.3 Loading Behavior

At startup, `DispatcherBuilder::build()` walks `user_yaml_dir`:

| Condition | Outcome |
|---|---|
| Directory does not exist | Silent; user_commands = ∅ |
| File is not `.md` | Skip; `trace::debug` |
| File has no front-matter | `warn!`; skip |
| Front-matter YAML parse fails | `warn!`; skip |
| Required field (`name`/`description`) missing | `warn!`; skip |
| Name collides with built-in | `warn!`; built-in wins; user file dropped |
| Name collides with another user file | `warn!`; first scanned wins |

No file-level error is fatal. In the worst case the dispatcher degrades to
"built-ins only" — the REPL stays usable.

### 6.4 Example User Commands

`~/.gasket/commands/translate.md` — see §6.1.

`~/.gasket/commands/review.md`:

```markdown
---
name: review
description: Linus-style code review of pasted snippet
aliases: []
---

Review the following code snippet in Linus Torvalds' style:
- Taste rating: good taste / passable / garbage
- Critical problems (if any)
- Direction for improvement (eliminate special cases, reduce nesting, fix the data structure)

{{user_input}}
```

`~/.gasket/commands/explain.md`:

```markdown
---
name: explain
description: Explain a concept concisely with examples
aliases: [ex]
allowed_tools: [wiki_search, wiki_read]
---

Explain the following concept concisely. If domain knowledge applies,
search the wiki first. Provide one canonical example.

{{user_input}}
```

---

## 7. Engine Touch Point

Exactly one engine API change.

### 7.1 Signature Change

```rust
// Before
pub async fn process_direct(
    &self,
    content: &str,
    session_key: &SessionKey,
) -> Result<AgentResponse, AgentError>;

// After
pub async fn process_direct(
    &self,
    content: &str,
    session_key: &SessionKey,
    tool_filter: Option<Vec<String>>,
) -> Result<AgentResponse, AgentError>;
```

The same change applies to `process_direct_streaming_with_channel`. All
existing call sites pass `None`; behavior is unchanged for them.

### 7.2 Filter Application

`tool_filter = Some(set)` causes the kernel, at the point where it composes
the tool spec list for the LLM, to retain only tools whose name is in `set`.
`tool_filter = None` (the default for all paths except YAML rewrites)
preserves the existing behavior verbatim.

The filter applies only to **this single invocation**. It does not mutate any
shared `ToolRegistry` state.

---

## 8. Error Handling and Invariants

### 8.1 Error Levels

| Source | Level | Behavior |
|---|---|---|
| `DispatcherBuilder::build()`: missing `host` | Fatal | `Err(BuildError::MissingHost)` — programmer bug |
| `build()`: duplicate built-in name | Fatal | `Err(BuildError::DuplicateBuiltin)` — programmer bug |
| `~/.gasket/commands/` does not exist | Silent | empty user_commands |
| User YAML parse fails | Warn | Skip file; dispatcher still builds |
| User YAML name collides with built-in | Warn | Built-in wins; user file dropped |
| User YAML name collides with another user | Warn | First scanned wins |
| `route()` gets unknown `/cmd` | User-visible | `CommandResult::Error("...")` |
| `route()` gets non-command text | Not an error | `RouteOutcome::Passthrough` |
| Built-in handler returns failure (e.g. `host.switch_model`) | User-visible | `CommandResult::Error` |
| Built-in handler panics | Fatal | Propagate; programmer bug, not caught |

### 8.2 Invariants

```
INV-1: After successful build, every alias maps to a registered canonical name
INV-2: route() returns RouteOutcome, never Err
INV-3: CommandResult::Quit can only originate from a built-in (never YAML)
INV-4: render_template is pure (same input → same output, no side effect)
INV-5: parser::parse allocates only the returned value (slices borrow input)
INV-6: User-YAML parse errors during build never cause build() to return Err
```

INV-2 is the central convenience: the CLI dispatch site has a single match
expression, no second `Err` arm.

### 8.3 BuildError

```rust
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("CommandHost not set; call .host() before build()")]
    MissingHost,

    #[error("duplicate built-in name: /{0}")]
    DuplicateBuiltin(String),

    #[error("user_dir is set but cannot be read: {0}")]
    UserDirIO(#[from] std::io::Error),
}
```

Three variants only. Per-file YAML errors are warnings, not BuildError
variants.

---

## 9. Testing

### 9.1 Layout

```
gasket/command/src/
├── parser.rs            mod tests with §4.1 cases
├── template.rs          mod tests
├── dispatcher.rs        mod tests using MockCommandHost
├── yaml_loader.rs       mod tests for malformed front-matter
└── builtins/*.rs        per-command tests

gasket/command/tests/
├── end_to_end.rs        full build → route flow
└── reload.rs            startup loads user_dir contents

gasket/cli/tests/
└── agent_dispatch.rs    Reedline → dispatcher integration
```

### 9.2 MockCommandHost

```rust
pub struct MockCommandHost {
    pub clear_calls: Mutex<Vec<SessionKey>>,
    pub sessions_data: Vec<SessionSummary>,
    pub current_model_value: String,
    pub model_switch_result: Result<ModelSwitchInfo, String>,
}

#[async_trait]
impl CommandHost for MockCommandHost { ... }
```

Hand-written, four methods. No `mockall` dependency.

### 9.3 Required Test Cases

| Scenario | Test |
|---|---|
| `/help` lists every command | `dispatcher_test::help_lists_everything` |
| `/exit` returns `Handled(Quit)` | `dispatcher_test::exit_returns_quit` |
| `/new` calls `host.clear_session` exactly once | `dispatcher_test::new_clears_session` |
| `/translate hello` (YAML hit) returns Rewrite with rendered prompt | `dispatcher_test::yaml_rewrite` |
| `/unknownxyz` returns `Handled(Error(...))` | `dispatcher_test::unknown_command` |
| Plain text passes through verbatim | `dispatcher_test::passthrough` |
| Broken `~/.gasket/commands/foo.md` is skipped | `yaml_loader_test::broken_skipped` |
| User YAML colliding with built-in: built-in wins, warn fires | `dispatcher_test::collision_builtin_wins` |
| All 8 parser table rows | `parser_test::table` |

---

## 10. Implementation Order

Four batches. Each ends with a green workspace build and passing tests.

### Batch 1 — Crate skeleton + data structures

| Step | Task |
|---|---|
| 1.1 | New `gasket/command/Cargo.toml` (deps: tokio, serde, serde_yaml, tracing, thiserror, async-trait, gasket-types) |
| 1.2 | `src/types.rs`: `Command`, `CommandKind`, `CommandResult`, `RouteOutcome` |
| 1.3 | `src/error.rs`: `BuildError` |
| 1.4 | `src/host.rs`: `CommandHost` trait; `SessionSummary`, `ModelSwitchInfo` go in `gasket-types` |
| 1.5 | `src/parser.rs` + 8 unit tests |
| 1.6 | `src/template.rs` + tests |

### Batch 2 — Dispatcher + YAML loader

| Step | Task |
|---|---|
| 2.1 | `src/dispatcher.rs` with `route` and `dispatch`; tests use MockCommandHost |
| 2.2 | `src/yaml_loader.rs`: scan dir, parse front-matter, skip broken files |
| 2.3 | `DispatcherBuilder::build()` glues 1.x + 2.x |

### Batch 3 — Built-in commands

| Step | Task |
|---|---|
| 3.1 | `src/builtins/{exit,clear,help}.rs` (no host calls) |
| 3.2 | `src/builtins/{new,sessions,model}.rs` with MockCommandHost tests |
| 3.3 | `src/completer.rs`: Reedline `Completer` impl |

### Batch 4 — Engine touch point + CLI integration

| Step | Task |
|---|---|
| 4.1 | Add `tool_filter: Option<Vec<String>>` to `process_direct` and `process_direct_streaming_with_channel`; existing call sites pass `None` |
| 4.2 | Plumb `tool_filter` through `RuntimeContext` to the kernel's tool-spec assembly point; filter there |
| 4.3 | Implement `CliCommandHost` in `cli/src/commands/agent.rs`; replace the hardcoded if/else with `dispatcher.route(...)` |
| 4.4 | End-to-end smoke: built-ins work; a sample `~/.gasket/commands/translate.md` rewrites successfully |

Estimated effort: B1 ≈ half day, B2 ≈ one day, B3 ≈ half day, B4 ≈ one day.
**~3 working days total.**

---

## 11. Out of Scope

Things this spec deliberately does not address. Each is a candidate for a
follow-up plan if the need is real.

- **`/wiki` REPL subcommand.** The top-level `gasket wiki` clap subcommand
  exists; bringing it into the REPL means lifting clap into the dispatcher.
  Day-2.
- **C-layer flow orchestration.** The earlier deleted
  `flow-command-system-design.md` covered phase-based plan-act-review flows.
  Not resumed here.
- **Web frontend wiring.** This spec ensures the dispatcher API is reusable;
  the actual Web UI integration is a separate plan.
- **Bot channel integration.** Telegram, Discord, Slack and other bot
  channels do **not** route through dispatcher. Their `/start`-style messages
  remain LLM input, preserving existing contracts.
- **Hot reload of user YAML.** Startup scan only; restart to pick up changes.
- **Multi-variable templating.** Day-1 supports only `{{user_input}}`. Adding
  more variables is mechanical when needed.
- **Command ACL / permissions.** Single-user context, no meaning.
- **Token / cost surface inside dispatcher.** Engine already tracks tokens;
  surfacing in REPL is a separate UX concern.
