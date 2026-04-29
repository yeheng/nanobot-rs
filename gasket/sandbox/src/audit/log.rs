//! Audit log implementation
//!
//! Provides file-based audit logging with rotation support.
//! Uses a buffered writer for efficient I/O performance.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::Mutex;
use tracing::{debug, info};

use super::AuditEvent;
use crate::config::AuditConfig;
use crate::error::{Result, SandboxError};

/// Internal state for the audit log.
///
/// Both the writer and current_size are protected by the same mutex
/// to prevent TOCTOU race conditions during rotation.
struct LogState {
    /// Buffered writer with file handle (kept open)
    writer: Option<BufWriter<File>>,
    /// Current file size for rotation tracking
    current_size: u64,
}

/// Audit log writer with buffered I/O.
///
/// Uses a BufWriter to minimize system calls during write operations.
/// The writer is held open across multiple write calls for efficiency.
pub struct AuditLog {
    /// Log file path
    path: PathBuf,
    /// Maximum file size in bytes
    max_size_bytes: u64,
    /// Whether to attach captured stdout/stderr to command-end events
    log_output: bool,
    /// Combined state protected by a single mutex to prevent TOCTOU races
    state: Arc<Mutex<LogState>>,
}

impl AuditLog {
    /// Create a new audit log
    pub fn new(config: &AuditConfig) -> Result<Self> {
        let path = config.log_file.clone().unwrap_or_else(|| {
            let config_dir = dirs::config_dir()
                .or_else(|| dirs::home_dir().map(|h| h.join(".gasket")))
                .unwrap_or_else(|| PathBuf::from("."));
            config_dir.join("audit.log")
        });

        let max_size_bytes = config.max_size_mb * 1024 * 1024;

        Ok(Self {
            path,
            max_size_bytes,
            log_output: config.log_output,
            state: Arc::new(Mutex::new(LogState {
                writer: None,
                current_size: 0,
            })),
        })
    }

    /// Create with a specific path
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max_size_bytes: 100 * 1024 * 1024, // 100 MB default
            log_output: false,
            state: Arc::new(Mutex::new(LogState {
                writer: None,
                current_size: 0,
            })),
        }
    }

    /// Whether captured command output should be attached to audit events.
    pub fn log_output(&self) -> bool {
        self.log_output
    }

    /// Initialize the audit log
    pub async fn initialize(&self) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).await.map_err(|e| {
                    SandboxError::AuditError(format!("Failed to create log directory: {}", e))
                })?;
            }
        }

        // Get current file size
        let size = if self.path.exists() {
            fs::metadata(&self.path).await.map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        self.state.lock().await.current_size = size;
        debug!("Audit log initialized: {:?} ({} bytes)", self.path, size);
        Ok(())
    }

    /// Get or create the buffered writer.
    async fn get_writer(&self) -> Result<tokio::io::BufWriter<File>> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .map_err(|e| SandboxError::AuditError(format!("Failed to open log file: {}", e)))?;

        Ok(BufWriter::new(file))
    }

    /// Write an event to the log
    ///
    /// All operations (size check, rotation, write, size update) happen
    /// within a single lock guard to prevent TOCTOU race conditions.
    pub async fn write(&self, event: &AuditEvent) -> Result<()> {
        // Serialize event
        let mut line = serde_json::to_string(event)
            .map_err(|e| SandboxError::AuditError(format!("Failed to serialize event: {}", e)))?;
        line.push('\n');

        let line_len = line.len() as u64;

        // All state modifications happen within a single lock scope.
        // The tokio::sync::Mutex guard is held across await points to prevent
        // TOCTOU race conditions during rotation.
        let mut state = self.state.lock().await;

        // Check for rotation
        if state.current_size + line_len > self.max_size_bytes {
            // Close the writer before rotation
            state.writer = None;

            self.rotate().await?;

            state.current_size = 0;
        }

        // Lazily initialize writer if needed
        if state.writer.is_none() {
            state.writer = Some(self.get_writer().await?);
        }

        let writer = state.writer.as_mut().unwrap();
        writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| SandboxError::AuditError(format!("Failed to write to log: {}", e)))?;

        // Flush to ensure data is persisted
        writer
            .flush()
            .await
            .map_err(|e| SandboxError::AuditError(format!("Failed to flush log: {}", e)))?;

        // Update size tracking (still within same lock scope)
        state.current_size += line_len;

        Ok(())
    }

    /// Rotate the log file using a numeric suffix so previous rotations are
    /// preserved (`audit.log.1` is the most recent rotation, `audit.log.2`
    /// the next, etc., up to `MAX_KEEP`).
    async fn rotate(&self) -> Result<()> {
        info!("Rotating audit log: {:?}", self.path);

        if !self.path.exists() {
            return Ok(());
        }

        const MAX_KEEP: u32 = 5;
        let stem = self.path.to_string_lossy();

        // Drop the oldest if it would exceed our retention.
        let oldest = PathBuf::from(format!("{}.{}", stem, MAX_KEEP));
        if oldest.exists() {
            fs::remove_file(&oldest).await.map_err(|e| {
                SandboxError::AuditError(format!("Failed to delete old log: {}", e))
            })?;
        }

        // Shift .N → .N+1, descending so we don't overwrite ourselves.
        for i in (1..MAX_KEEP).rev() {
            let from = PathBuf::from(format!("{}.{}", stem, i));
            let to = PathBuf::from(format!("{}.{}", stem, i + 1));
            if from.exists() {
                fs::rename(&from, &to).await.map_err(|e| {
                    SandboxError::AuditError(format!("Failed to shift rotated log: {}", e))
                })?;
            }
        }

        // Move the active log into slot .1.
        let first = PathBuf::from(format!("{}.1", stem));
        fs::rename(&self.path, &first)
            .await
            .map_err(|e| SandboxError::AuditError(format!("Failed to rotate log: {}", e)))?;

        Ok(())
    }

    /// Log a command execution
    ///
    /// `session_id` is propagated as-is — if `None`, the event is logged
    /// without a session correlator. (Previously, `None` was replaced with a
    /// fresh random UUID, which made `command_start` and `command_end`
    /// pairs impossible to correlate across calls.)
    pub async fn log_command(
        &self,
        command: &str,
        working_dir: &std::path::Path,
        session_id: Option<uuid::Uuid>,
    ) -> Result<()> {
        let mut event = AuditEvent::command_start(command, working_dir.display().to_string());
        if let Some(id) = session_id {
            event = event.with_session_id(id);
        }
        self.write(&event).await
    }

    /// Log command completion
    pub async fn log_command_end(
        &self,
        command: &str,
        exit_code: Option<i32>,
        duration_ms: u64,
        timed_out: bool,
        session_id: Option<uuid::Uuid>,
    ) -> Result<()> {
        let mut event = AuditEvent::command_end(command, exit_code, duration_ms, timed_out);
        if let Some(id) = session_id {
            event = event.with_session_id(id);
        }
        self.write(&event).await
    }

    /// Log a permission event
    pub async fn log_permission(
        &self,
        operation: &str,
        granted: bool,
        reason: Option<&str>,
        session_id: Option<uuid::Uuid>,
    ) -> Result<()> {
        let event = if granted {
            AuditEvent::permission_granted(operation, "allowed")
        } else {
            AuditEvent::permission_denied(operation, reason.unwrap_or("Unknown"))
        };

        let event = if let Some(id) = session_id {
            event.with_session_id(id)
        } else {
            event
        };

        self.write(&event).await
    }

    /// Log a security event
    pub async fn log_security(
        &self,
        category: &str,
        description: &str,
        severity: &str,
    ) -> Result<()> {
        let event = AuditEvent::security_event(category, description, severity);
        self.write(&event).await
    }

    /// Check if logging is enabled
    pub fn is_enabled(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_log_write() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("test_audit.log");
        let log = AuditLog::with_path(&path);

        log.initialize().await.unwrap();

        let event = AuditEvent::command_start("ls -la", "/tmp");
        log.write(&event).await.unwrap();

        // Verify file was created
        assert!(path.exists());

        // Read and verify content
        let content = fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("command_start"));
    }
}
