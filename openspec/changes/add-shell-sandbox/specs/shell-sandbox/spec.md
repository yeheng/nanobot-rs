## ADDED Requirements

### Requirement: Sandbox Execution Backend
The system SHALL support executing shell commands inside an OS-level sandbox using bubblewrap (`bwrap`) on Linux. When sandbox mode is enabled and `bwrap` is available, all commands dispatched by `ExecTool` MUST run inside a bubblewrap container with namespace isolation (mount, PID, IPC).

#### Scenario: Sandboxed command execution on Linux
- **WHEN** sandbox is enabled in configuration AND `bwrap` binary is found on `$PATH`
- **THEN** the system SHALL execute the command inside a bubblewrap sandbox with isolated mount namespace, PID namespace, and IPC namespace

#### Scenario: Sandbox unavailable fallback
- **WHEN** sandbox is enabled in configuration BUT `bwrap` binary is NOT found on `$PATH`
- **THEN** the system SHALL log a warning, fall back to direct execution with ulimit-based resource limits, and include a warning in the tool metadata

#### Scenario: Sandbox disabled
- **WHEN** sandbox is NOT enabled in configuration
- **THEN** the system SHALL execute commands directly via `bash -c` as it does today, preserving full backward compatibility

### Requirement: Workspace Directory Configuration
The system SHALL support a configurable workspace directory that serves as the working directory and read-write mount point for shell command execution. The default workspace path MUST be `$HOME/workspace`.

#### Scenario: Default workspace path
- **WHEN** no workspace path is specified in configuration
- **THEN** the system SHALL use `$HOME/workspace` as the workspace directory and create it if it does not exist

#### Scenario: Custom workspace path
- **WHEN** a workspace path is specified via `tools.exec.workspace` in `config.yaml`
- **THEN** the system SHALL use the specified path as the workspace directory

#### Scenario: Workspace directory in sandboxed mode
- **WHEN** sandbox mode is enabled
- **THEN** the workspace directory SHALL be mounted read-write at `/workspace` inside the sandbox, and the host root filesystem SHALL be mounted read-only

### Requirement: Sandbox Filesystem Isolation
When sandbox mode is enabled, the system SHALL restrict filesystem access so that only the workspace directory is writable. The host root filesystem MUST be mounted read-only. A size-limited tmpfs MUST be provided at `/tmp` inside the sandbox.

#### Scenario: Write attempt outside workspace in sandbox
- **WHEN** a sandboxed command attempts to write to a path outside the workspace mount
- **THEN** the operating system SHALL deny the write with a permission error (enforced by the sandbox namespace, not by the application)

#### Scenario: Temporary files in sandbox
- **WHEN** a sandboxed command writes to `/tmp`
- **THEN** the write SHALL succeed within the tmpfs size limit (default: 64MB) and the data SHALL NOT persist after the command exits
