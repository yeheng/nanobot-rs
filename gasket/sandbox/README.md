# nanobot-sandbox

Secure sandbox execution module for nanobot with multi-platform support, approval system, and audit logging.

## Features

- **Multi-platform support**: Linux (bwrap), macOS (sandbox-exec), Windows (Job Objects)
- **Approval system**: Fine-grained permission management with CLI and WebSocket interaction
- **Audit logging**: Comprehensive logging of all operations
- **Resource limits**: Memory, CPU time, output size, and process count limits
- **Command policy**: Allowlist/denylist for command filtering

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
nanobot-sandbox = { path = "nanobot-sandbox" }
```

## Feature Flags

- `default` - Includes `platform-native`, `approval`, and `audit` features
- `platform-native` - Platform-native sandbox (bwrap, sandbox-exec, Job Objects)
- `approval` - Permission confirmation system
- `audit` - Audit logging
- `sqlite` - SQLite storage for approval rules (optional, default uses JSON files)
- `full` - All features including SQLite

## Quick Start

```rust
use nanobot_sandbox::{ProcessManager, SandboxConfig};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a fallback (no sandbox) configuration
    let config = SandboxConfig::fallback();

    // Create a process manager
    let manager = ProcessManager::new(config);

    // Execute a command
    let result = manager.execute("echo hello", Path::new("/tmp")).await?;

    println!("Output: {}", result.stdout);
    Ok(())
}
```

## Configuration

```yaml
sandbox:
  # Enable sandbox
  enabled: true

  # Backend: auto | fallback | bwrap | sandbox-exec | docker
  backend: auto

  # Approval configuration
  approval:
    enabled: true
    default_level: ask_always  # denied | ask_always | ask_once | allowed
    session_timeout: 3600

  # Resource limits
  limits:
    max_memory_mb: 512
    max_cpu_secs: 60
    max_output_bytes: 1048576
    max_processes: 10

  # Command policy
  policy:
    allowlist: []
    denylist:
      - "rm -rf /"
      - "mkfs"

  # Audit logging
  audit:
    enabled: true
    log_file: ~/.nanobot/audit.log
```

## Platform Support

| Platform | Backend | Description |
|----------|---------|-------------|
| Linux | bwrap | Bubblewrap namespace isolation |
| macOS | sandbox-exec | Apple Seatbelt sandbox |
| Windows | Job Objects | Windows Job Objects limits |
| All | fallback | Direct execution with ulimit |

## Approval System

The approval system provides fine-grained permission management:

```rust
use nanobot_sandbox::prelude::*;

// Create approval manager
let store = JsonPermissionStore::default_location()?;
let config = ApprovalConfig::default();
let manager = ApprovalManager::new(Box::new(store), config);

// Check permission
let operation = OperationType::command("rm");
let verdict = manager.check_permission(&operation, &ExecutionContext::new()).await;

// Add a rule
let rule = ApprovalRule::new(
    OperationType::command("ls"),
    PermissionLevel::Allowed,
);
manager.add_rule(rule).await?;
```

## Audit Logging

```rust
use nanobot_sandbox::audit::{AuditLog, AuditEvent, AuditConfig};

let config = AuditConfig::default();
let log = AuditLog::new(&config)?;
log.initialize().await?;

// Log an event
let event = AuditEvent::command_start("ls -la", "/home/user");
log.write(&event).await?;
```

## License

MIT
