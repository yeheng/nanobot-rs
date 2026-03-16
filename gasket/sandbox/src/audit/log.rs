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

/// Audit log writer with buffered I/O.
///
/// Uses a BufWriter to minimize system calls during write operations.
/// The writer is held open across multiple write calls for efficiency.
pub struct AuditLog {
    /// Log file path
    path: PathBuf,
    /// Maximum file size in bytes
    max_size_bytes: u64,
    /// Whether to log command output
    _log_output: bool,
    /// Buffered writer with file handle (kept open)
    writer: Arc<Mutex<Option<BufWriter<File>>>>,
    /// Current file size for rotation tracking
    current_size: Arc<Mutex<u64>>,
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
            _log_output: config.log_output,
            writer: Arc::new(Mutex::new(None)),
            current_size: Arc::new(Mutex::new(0)),
        })
    }

    /// Create with a specific path
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max_size_bytes: 100 * 1024 * 1024, // 100 MB default
            _log_output: false,
            writer: Arc::new(Mutex::new(None)),
            current_size: Arc::new(Mutex::new(0)),
        }
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

        *self.current_size.lock().await = size;
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
    pub async fn write(&self, event: &AuditEvent) -> Result<()> {
        // Serialize event
        let mut line = serde_json::to_string(event)
            .map_err(|e| SandboxError::AuditError(format!("Failed to serialize event: {}", e)))?;
        line.push('\n');

        let line_len = line.len() as u64;

        // Check for rotation (with current_size lock held briefly)
        {
            let mut current_size = self.current_size.lock().await;
            if *current_size + line_len > self.max_size_bytes {
                // Need to rotate - drop writer first
                {
                    let mut writer_guard = self.writer.lock().await;
                    *writer_guard = None; // Close the writer
                }
                self.rotate().await?;
                *current_size = 0;
            }
        }

        // Write using buffered writer
        {
            let mut writer_guard = self.writer.lock().await;

            // Lazily initialize writer if needed
            if writer_guard.is_none() {
                *writer_guard = Some(self.get_writer().await?);
            }

            let writer = writer_guard.as_mut().unwrap();
            writer
                .write_all(line.as_bytes())
                .await
                .map_err(|e| SandboxError::AuditError(format!("Failed to write to log: {}", e)))?;

            // Flush to ensure data is persisted
            writer
                .flush()
                .await
                .map_err(|e| SandboxError::AuditError(format!("Failed to flush log: {}", e)))?;
        }

        // Update size tracking
        {
            let mut current_size = self.current_size.lock().await;
            *current_size += line_len;
        }

        Ok(())
    }

    /// Rotate the log file
    async fn rotate(&self) -> Result<()> {
        info!("Rotating audit log: {:?}", self.path);

        // Simple rotation: rename current file to .old
        let old_path = self.path.with_extension("log.old");

        if self.path.exists() {
            fs::rename(&self.path, &old_path)
                .await
                .map_err(|e| SandboxError::AuditError(format!("Failed to rotate log: {}", e)))?;
        }

        Ok(())
    }

    /// Log a command execution
    pub async fn log_command(
        &self,
        command: &str,
        working_dir: &std::path::Path,
        session_id: Option<uuid::Uuid>,
    ) -> Result<()> {
        let event = AuditEvent::command_start(command, working_dir.display().to_string())
            .with_session_id(session_id.unwrap_or_else(uuid::Uuid::new_v4));

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
        let event = AuditEvent::command_end(command, exit_code, duration_ms, timed_out)
            .with_session_id(session_id.unwrap_or_else(uuid::Uuid::new_v4));

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
