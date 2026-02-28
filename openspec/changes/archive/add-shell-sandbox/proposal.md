# Change: Add Shell Tool Security Sandbox

## Why
The current `ExecTool` executes arbitrary shell commands with no OS-level isolation. Its own documentation acknowledges this gap and recommends using a real sandbox. In deployments where nanobot is exposed via chat channels (Telegram, Discord, Slack), a compromised or misbehaving LLM response could execute destructive commands on the host. This proposal adds a bubblewrap-based sandbox, command policy engine, and resource limits to provide defense-in-depth for shell execution.

## What Changes
- **Sandbox execution backend**: Integrate bubblewrap (`bwrap`) on Linux for namespace-isolated command execution (mount, PID, IPC isolation). Filesystem is read-only except for the workspace directory. Falls back to unsandboxed execution with `ulimit`-based limits when `bwrap` is unavailable.
- **Configurable workspace directory**: New `tools.exec.workspace` config field (default: `$HOME/.nanobot`) that separates the agent's working directory from the nanobot config directory (`~/.nanobot`).
- **Command allowlist/denylist**: Advisory policy layer to catch accidental misuse and provide audit logging. Allowlist restricts to named binaries; denylist blocks matching patterns. Policy is evaluated before execution.
- **Resource limits**: Configurable memory limit (default: 512MB), CPU time limit (default: 60s), wall-clock timeout (existing, default: 120s), and output size limit (default: 1MB). Enforced via bwrap rlimits in sandbox mode, `ulimit` in fallback mode.
- **Filesystem access control**: In sandbox mode, host root is mounted read-only, workspace is mounted read-write, `/tmp` is a size-limited tmpfs. Writes outside the workspace are denied by the OS.

## Impact
- Affected specs: `shell-sandbox` (new), `shell-command-policy` (new), `shell-resource-limits` (new)
- Affected code:
  - `nanobot-core/src/tools/shell.rs` (major refactor)
  - `nanobot-core/src/config/schema.rs` (config extension)
  - `nanobot-cli/src/main.rs` (tool registration update)
  - New files: `tools/sandbox.rs`, `tools/command_policy.rs`, `tools/resource_limits.rs`
- Backward compatible: All changes are additive. Existing configurations continue to work (sandbox defaults to disabled).
