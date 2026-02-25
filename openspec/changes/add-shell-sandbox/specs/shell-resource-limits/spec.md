## ADDED Requirements

### Requirement: Execution Timeout
The system SHALL enforce a configurable wall-clock timeout for shell command execution. When the timeout is reached, the command process (and all its descendants) MUST be killed. The default timeout SHALL be 120 seconds.

#### Scenario: Command exceeds timeout
- **WHEN** a command execution exceeds the configured timeout duration
- **THEN** the system SHALL kill the process group and return a timeout error

#### Scenario: Custom timeout configuration
- **WHEN** a timeout value is specified via `tools.exec.timeout` in `config.yaml`
- **THEN** the system SHALL use the specified value as the wall-clock timeout in seconds

### Requirement: Memory Limit
The system SHALL support a configurable maximum memory limit for sandboxed command execution. When sandbox mode is enabled, the memory limit MUST be enforced via sandbox rlimits. The default memory limit SHALL be 512 MB.

#### Scenario: Command exceeds memory limit in sandbox
- **WHEN** sandbox mode is enabled AND a command attempts to allocate memory beyond the configured limit
- **THEN** the operating system SHALL deny the allocation (OOM kill) and the system SHALL report the failure

#### Scenario: Memory limit without sandbox
- **WHEN** sandbox mode is NOT enabled AND a memory limit is configured
- **THEN** the system SHALL apply the limit via `ulimit -v` as a best-effort enforcement

### Requirement: CPU Time Limit
The system SHALL support a configurable maximum CPU time limit for sandboxed command execution. The default CPU time limit SHALL be 60 seconds. This is distinct from wall-clock timeout; a process may idle without consuming CPU time.

#### Scenario: Command exceeds CPU time limit in sandbox
- **WHEN** sandbox mode is enabled AND a command consumes CPU time beyond the configured limit
- **THEN** the operating system SHALL send SIGXCPU to the process

#### Scenario: CPU limit without sandbox
- **WHEN** sandbox mode is NOT enabled AND a CPU limit is configured
- **THEN** the system SHALL apply the limit via `ulimit -t` as a best-effort enforcement

### Requirement: Maximum Output Size
The system SHALL enforce a maximum size for captured command output (stdout + stderr combined). Output exceeding the limit SHALL be truncated with a marker indicating truncation. The default limit SHALL be 1 MB.

#### Scenario: Command output exceeds limit
- **WHEN** a command produces output larger than the configured maximum
- **THEN** the system SHALL truncate the output and append a message indicating the number of bytes truncated
