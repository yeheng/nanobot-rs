//! Windows host execution backend (limited isolation via Job Objects)
//!
//! **WARNING**: This backend provides **basic** resource constraints via Windows
//! Job Objects — memory limits, CPU time limits, and lowered priority. It does
//! NOT provide filesystem isolation or true sandboxing. For proper sandboxing on
//! Windows, use WSL2 with bwrap.
//!
//! This is intentionally named `HostExecutor` to make the lack of true
//! sandboxing obvious at every call site.

use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use tracing::{debug, warn};
use winapi::shared::minwindef::FALSE;
use winapi::um::handleapi::CloseHandle;
use winapi::um::jobapi2::{AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject};
use winapi::um::winnt::{
    JobObjectExtendedLimitInformation, HANDLE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PRIORITY_CLASS, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOB_OBJECT_LIMIT_PROCESS_TIME,
};

use crate::backend::{ExecutionResult, Platform, SandboxBackend};
use crate::config::SandboxConfig;
use crate::error::{Result, SandboxError};

/// RAII wrapper for a Windows Job Object handle.
struct JobObject(HANDLE);

// SAFETY: HANDLE is an opaque kernel object handle, safe to send between threads.
// The RAII wrapper ensures exclusive ownership and proper cleanup via Drop.
unsafe impl Send for JobObject {}

impl Drop for JobObject {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// Host execution backend — runs commands via cmd.exe with **basic** Job Object
/// resource limits.
///
/// Applies:
/// - Per-process memory limit (from `SandboxConfig.limits.max_memory_mb`)
/// - Per-process CPU time limit (from `SandboxConfig.limits.max_cpu_secs`)
/// - Active process limit (from `SandboxConfig.limits.max_processes`)
/// - Lowered priority class (`BELOW_NORMAL_PRIORITY_CLASS`)
/// - `KILL_ON_JOB_CLOSE` so the child dies if the job handle is dropped
pub struct HostExecutor {
    // No persistent state — Job Objects are created per execution
}

impl HostExecutor {
    /// Create a new host executor backend.
    pub fn new() -> Self {
        warn!(
            "HostExecutor: Commands will run with BASIC resource limits only. \
             For proper sandboxing, use WSL2 with bwrap."
        );
        Self {}
    }

    /// Validate that `working_dir` is within the configured workspace.
    /// Returns the canonicalized path on success.
    fn validate_working_dir(&self, working_dir: &Path, config: &SandboxConfig) -> Result<PathBuf> {
        if !working_dir.exists() {
            return Err(SandboxError::PathNotAllowed {
                path: working_dir.to_path_buf(),
            });
        }
        if !working_dir.is_dir() {
            return Err(SandboxError::PathNotAllowed {
                path: working_dir.to_path_buf(),
            });
        }

        let canonical = working_dir
            .canonicalize()
            .map_err(|_| SandboxError::PathNotAllowed {
                path: working_dir.to_path_buf(),
            })?;

        // If a workspace is configured, enforce that working_dir stays inside it.
        if let Some(ref workspace) = config.workspace {
            let ws_canonical =
                workspace
                    .canonicalize()
                    .map_err(|_| SandboxError::PathNotAllowed {
                        path: workspace.clone(),
                    })?;
            if !canonical.starts_with(&ws_canonical) {
                warn!(
                    "HostExecutor: working_dir {:?} is outside workspace {:?}. Blocking execution.",
                    canonical, ws_canonical
                );
                return Err(SandboxError::PathNotAllowed { path: canonical });
            }
        }

        Ok(canonical)
    }

    fn build_command_internal(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<Command> {
        let validated = self.validate_working_dir(working_dir, config)?;

        let mut command = Command::new("cmd");
        command.arg("/C").arg(cmd).current_dir(&validated);

        debug!("Windows command: {:?}", command);
        Ok(command)
    }

    /// Create a Windows Job Object with the resource limits from `config`.
    fn create_job_object(&self, config: &SandboxConfig) -> Option<JobObject> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                warn!("HostExecutor: CreateJobObjectW failed");
                return None;
            }

            let limits = &config.limits;
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            let mut limit_flags = 0u32;

            if limits.max_memory_mb > 0 {
                info.ProcessMemoryLimit =
                    (u64::from(limits.max_memory_mb) * 1024 * 1024) as usize;
                limit_flags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            }

            if limits.max_cpu_secs > 0 {
                // PerProcessUserTimeLimit is in 100-nanosecond intervals.
                // LARGE_INTEGER is a union — write via raw pointer to avoid
                // needing to know the exact winapi UNION! accessors.
                let time_100ns = (limits.max_cpu_secs as i64) * 10_000_000;
                std::ptr::write(
                    &mut info.BasicLimitInformation.PerProcessUserTimeLimit as *mut _ as *mut i64,
                    time_100ns,
                );
                limit_flags |= JOB_OBJECT_LIMIT_PROCESS_TIME;
            }

            if limits.max_processes > 0 {
                info.BasicLimitInformation.ActiveProcessLimit = limits.max_processes;
                limit_flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            }

            // Lower priority so a runaway loop doesn't starve the host.
            info.BasicLimitInformation.PriorityClass =
                winapi::um::winbase::BELOW_NORMAL_PRIORITY_CLASS as _;
            limit_flags |= JOB_OBJECT_LIMIT_PRIORITY_CLASS;

            // Ensure child processes are terminated when the job handle is closed.
            limit_flags |= JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            info.BasicLimitInformation.LimitFlags = limit_flags;

            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );

            if ok == FALSE {
                warn!("HostExecutor: SetInformationJobObject failed");
                CloseHandle(job);
                return None;
            }

            Some(JobObject(job))
        }
    }
}

impl Default for HostExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SandboxBackend for HostExecutor {
    fn name(&self) -> &str {
        "host-executor"
    }

    async fn is_available(&self) -> bool {
        true
    }

    fn supported_platforms(&self) -> &[Platform] {
        &[Platform::Windows]
    }

    fn provides_filesystem_isolation(&self) -> bool {
        false
    }

    fn build_command(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<Command> {
        self.build_command_internal(cmd, working_dir, config)
    }

    async fn execute(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<ExecutionResult> {
        let validated = self.validate_working_dir(working_dir, config)?;
        let job = self.create_job_object(config);

        let cmd = cmd.to_string();
        let max_output = config.limits.max_output_bytes;

        let output = tokio::task::spawn_blocking(move || -> Result<std::process::Output> {
            let mut command = Command::new("cmd");
            command.arg("/C").arg(&cmd).current_dir(&validated);

            debug!("Windows command: {:?}", command);

            let mut child = command
                .spawn()
                .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))?;

            if let Some(ref job) = job {
                let raw_handle = child.as_raw_handle();
                let ok = unsafe { AssignProcessToJobObject(job.0, raw_handle as HANDLE) };
                if ok == FALSE {
                    warn!("HostExecutor: AssignProcessToJobObject failed");
                }
            }

            child
                .wait_with_output()
                .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))
        })
        .await
        .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))??;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let stdout = if stdout.len() > max_output {
            let original_len = stdout.len();
            let mut truncated = stdout;
            truncated.truncate(max_output);
            truncated.push_str(&format!(
                "\n\n[OUTPUT TRUNCATED: {} bytes exceeded limit of {} bytes]",
                original_len, max_output
            ));
            truncated
        } else {
            stdout
        };

        Ok(ExecutionResult {
            exit_code: output.status.code(),
            stdout,
            stderr,
            timed_out: false,
            resource_exceeded: false,
            duration_ms: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fallback_availability() {
        let backend = HostExecutor::new();
        assert!(backend.is_available().await);
    }

    #[test]
    fn test_build_command() {
        let backend = HostExecutor::new();
        let config = SandboxConfig::default();
        let cmd = backend.build_command("echo hello", Path::new("C:\\"), &config);
        assert!(cmd.is_ok());

        let cmd = cmd.unwrap();
        assert_eq!(cmd.get_program(), "cmd");

        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args[0], "/C");
    }
}
