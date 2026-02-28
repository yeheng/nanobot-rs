## Context

The current `ExecTool` (`nanobot-core/src/tools/shell.rs`) executes arbitrary shell commands via `bash -c` with no OS-level isolation. The tool's own documentation acknowledges that string-based filtering is ineffective and recommends a real sandbox (Docker, bubblewrap). This proposal adds that real sandbox layer.

**Key constraints:**
- Must support Linux (primary) and macOS (best-effort/fallback)
- nsjail and bubblewrap are Linux-only; macOS has `sandbox-exec` (deprecated) but no direct equivalent
- The project forbids `unsafe` code (`unsafe_code = "forbid"`)
- Must not break the existing non-sandboxed flow (sandbox is opt-in via config)

**Stakeholders:** Anyone deploying nanobot where the AI agent can execute shell commands, particularly in multi-user or internet-facing scenarios (Telegram, Discord, Slack channels).

## Goals / Non-Goals

### Goals
- Provide OS-level process isolation for shell commands using bubblewrap (`bwrap`) on Linux
- Allow a configurable workspace directory (default: `$HOME/.nanobot`) separate from the nanobot config directory (`~/.nanobot`)
- Support command allowlist/denylist as a policy layer (advisory, not a security boundary)
- Enforce resource limits: CPU time, memory, wall-clock timeout
- Restrict filesystem access: read-only root, read-write only in workspace

### Non-Goals
- Full Docker/container orchestration (too heavy for single-command execution)
- nsjail support in v1 (bubblewrap is simpler, more widely available, sufficient)
- macOS sandbox-exec support (deprecated API; macOS falls back to non-sandboxed with resource limits via `ulimit`)
- Network isolation in v1 (can be added later via bwrap `--unshare-net`)
- Sandboxing MCP tools or file system tools (separate concern)

## Decisions

### Decision 1: Use bubblewrap (`bwrap`) as the sandbox backend

**Why:** bubblewrap is lightweight (single binary, no daemon), widely packaged on Linux distros, requires no root (uses user namespaces), and maps cleanly to our per-command execution model. nsjail requires more complex configuration and is less commonly pre-installed.

**Alternatives considered:**
- **nsjail**: More features (protobuf config, cgroupv2 support) but heavier dependency, less commonly available, overkill for single-command sandboxing
- **Docker**: Too much overhead per command (container startup time ~200ms+), requires Docker daemon
- **Firejail**: Security concerns (setuid binary, history of privilege escalation CVEs)
- **landlock (Linux 5.13+)**: Rust-native but kernel-version dependent, no process resource limits

**Fallback:** When `bwrap` is not available, fall back to unsandboxed execution with `ulimit`-based resource limits and a clear warning log.

### Decision 2: Workspace directory is user-configurable, defaults to `$HOME/.nanobot`

**Why:** The current working directory is `~/.nanobot` (the config directory), which mixes config files with agent work output. A dedicated workspace directory provides a cleaner separation and a natural mount point for sandboxed execution.

**Configuration path:** `tools.exec.workspace` in `config.yaml`.

### Decision 3: Command policy (allowlist/denylist) is advisory, not a security boundary

**Why:** As the existing code comments correctly note, the shell is Turing-complete. String-based filtering is trivially bypassed. The policy layer exists to:
1. Catch accidental misuse (e.g., `rm -rf /`)
2. Provide audit logging for sensitive commands
3. Give operators a knob to limit agent behavior at the prompt level

The actual security boundary is the sandbox itself (filesystem isolation + resource limits).

### Decision 4: Resource limits via cgroups (inside bwrap) + tokio timeout (outside)

**Why:** Two layers of enforcement:
- **Inner (bwrap):** `--rlimit-*` flags for memory/CPU per-process limits
- **Outer (tokio):** Existing `tokio::time::timeout` as a hard wall-clock kill switch

This ensures even if the sandboxed process forks or spawns children, the outer timeout kills the entire process group.

## Architecture

```
┌─────────────────────────────────────────────┐
│                  ExecTool                    │
│  ┌──────────────┐  ┌─────────────────────┐  │
│  │ CommandPolicy │  │  SandboxProvider    │  │
│  │ (allowlist/   │  │  ┌───────────────┐  │  │
│  │  denylist)    │  │  │ BwrapSandbox  │  │  │
│  │              │  │  ├───────────────┤  │  │
│  │              │  │  │ FallbackExec  │  │  │
│  │              │  │  └───────────────┘  │  │
│  └──────────────┘  └─────────────────────┘  │
│  ┌──────────────────────────────────────┐   │
│  │         ResourceLimits               │   │
│  │  (timeout, max_memory, max_cpu_secs) │   │
│  └──────────────────────────────────────┘   │
└─────────────────────────────────────────────┘
```

### Sandbox Execution Flow

1. `ExecTool::execute()` receives command
2. `CommandPolicy::check()` evaluates allowlist/denylist → reject or allow (with log)
3. `SandboxProvider::exec()` dispatches:
   - **If `bwrap` available + sandbox enabled:** Build bwrap command with bind mounts and rlimits
   - **Else:** Fall back to direct `bash -c` with `ulimit` prefix
4. `tokio::time::timeout` wraps the whole execution

### Bwrap Mount Layout

```
/                    → bind-ro from host /
/workspace           → bind-rw from $HOME/.nanobot (configurable)
/tmp                 → tmpfs (size-limited)
/dev                 → minimal devtmpfs (null, zero, urandom)
/proc                → new proc namespace
```

## Risks / Trade-offs

| Risk | Mitigation |
|------|-----------|
| `bwrap` not installed on target system | Auto-detect at startup, warn and fall back to unsandboxed mode |
| User namespaces disabled on some Linux distros | Document requirement, detect via `/proc/sys/kernel/unprivileged_userns_clone` |
| Performance overhead of bwrap per command | Minimal (~5ms extra per invocation); acceptable for AI agent commands |
| Denylist bypass via shell encoding tricks | Denylist is advisory only; sandbox is the real security boundary |
| macOS users get no sandbox | Clearly documented; macOS uses ulimit-based resource limits as best-effort |

## Migration Plan

1. Add new config fields to `ExecToolConfig` with backward-compatible defaults (sandbox disabled, workspace = `$HOME/.nanobot`)
2. Existing configurations continue to work without changes
3. Users opt-in to sandbox via `tools.exec.sandbox.enabled: true`
4. No breaking changes to the `Tool` trait or `ToolRegistry`

## Open Questions

- Should we add `--unshare-net` (network isolation) as an opt-in flag in v1 or defer to v2?
- Should the denylist include a sensible default set (e.g., `rm -rf /`, `mkfs`, `dd if=/dev/zero`) or start completely empty?
