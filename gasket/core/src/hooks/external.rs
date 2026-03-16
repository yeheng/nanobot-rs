//! External shell hook engine — async-safe subprocess execution.
//!
//! Executes shell scripts with JSON on stdin, parses JSON from stdout.
//! All scripts are run via `tokio::process::Command` to avoid blocking
//! the async runtime.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, warn};

/// Maximum time a hook script is allowed to run before being killed.
const HOOK_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum bytes to read from a hook script's stdout.
const MAX_STDOUT_BYTES: usize = 1_048_576; // 1 MB

// ── JSON Schema ─────────────────────────────────────────────

/// Input sent to a hook script via stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalHookInput {
    /// Event name: "pre_request" or "post_response"
    pub event: String,
    /// Session identifier (e.g., "telegram:12345")
    pub session_id: String,
    /// The user message (for pre_request) or agent response (for post_response)
    pub user_message: String,
    /// Extra metadata (tools used, etc.)
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Output received from a hook script via stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalHookOutput {
    /// "continue" or "abort"
    #[serde(default = "default_action")]
    pub action: String,
    /// Modified message (optional — if present, replaces the original)
    pub modified_message: Option<String>,
    /// Error message shown to user when action is "abort"
    pub error: Option<String>,
}

fn default_action() -> String {
    "continue".to_string()
}

impl ExternalHookOutput {
    /// Returns `true` if the hook wants to abort the request.
    pub fn is_abort(&self) -> bool {
        self.action.eq_ignore_ascii_case("abort")
    }
}

// ── Runner ──────────────────────────────────────────────────

/// Script availability cache — avoids filesystem I/O on hot paths.
#[derive(Debug, Clone, Copy)]
struct ScriptAvailability {
    /// Whether pre_request.sh exists and is executable
    has_pre_request: bool,
    /// Whether post_response.sh exists and is executable
    has_post_response: bool,
}

impl ScriptAvailability {
    fn scan(hooks_dir: &Path) -> Self {
        let pre_request_path = hooks_dir.join("pre_request.sh");
        let post_response_path = hooks_dir.join("post_response.sh");

        let has_pre_request = pre_request_path.exists() && is_executable(&pre_request_path);
        let has_post_response = post_response_path.exists() && is_executable(&post_response_path);

        if has_pre_request || has_post_response {
            debug!(
                "Hook scripts available: pre_request={}, post_response={}",
                has_pre_request, has_post_response
            );
        }

        Self {
            has_pre_request,
            has_post_response,
        }
    }
}

/// Executes external shell hook scripts from a directory.
///
/// If the hooks directory doesn't exist or a script is missing,
/// the runner silently returns `None` — hooks are optional.
#[derive(Clone)]
pub struct ExternalHookRunner {
    hooks_dir: Option<PathBuf>,
    /// Cached script availability — scanned once at startup.
    script_availability: ScriptAvailability,
}

impl ExternalHookRunner {
    /// Create a runner that looks for scripts in the given directory.
    ///
    /// Scans for script availability once at creation time, avoiding
    /// filesystem I/O on every message.
    pub fn new(hooks_dir: PathBuf) -> Self {
        let (hooks_dir, script_availability) = if hooks_dir.is_dir() {
            let availability = ScriptAvailability::scan(&hooks_dir);
            (Some(hooks_dir), availability)
        } else {
            debug!("Hooks directory not found: {}", hooks_dir.display());
            (
                None,
                ScriptAvailability {
                    has_pre_request: false,
                    has_post_response: false,
                },
            )
        };
        Self {
            hooks_dir,
            script_availability,
        }
    }

    /// Create a no-op runner (for subagents or testing).
    pub fn noop() -> Self {
        Self {
            hooks_dir: None,
            script_availability: ScriptAvailability {
                has_pre_request: false,
                has_post_response: false,
            },
        }
    }

    /// Run the `pre_request.sh` hook.
    ///
    /// Returns `Ok(Some(output))` if the script produced valid JSON,
    /// `Ok(None)` if no script exists or stdout was empty,
    /// `Err` on execution or parse failure.
    pub async fn run_pre_request(
        &self,
        session_key: &str,
        user_message: &str,
    ) -> anyhow::Result<Option<ExternalHookOutput>> {
        // Fast path: check cached availability before any I/O
        if !self.script_availability.has_pre_request {
            return Ok(None);
        }

        let input = ExternalHookInput {
            event: "pre_request".to_string(),
            session_id: session_key.to_string(),
            user_message: user_message.to_string(),
            metadata: serde_json::Value::Object(serde_json::Map::new()),
        };
        self.run_script("pre_request.sh", &input).await
    }

    /// Run the `post_response.sh` hook.
    ///
    /// This is fire-and-wait: we wait for the script but don't use
    /// the output to modify the response.
    pub async fn run_post_response(
        &self,
        session_key: &str,
        response_content: &str,
        tools_used: &str,
    ) -> anyhow::Result<Option<ExternalHookOutput>> {
        // Fast path: check cached availability before any I/O
        if !self.script_availability.has_post_response {
            return Ok(None);
        }

        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "tools_used".to_string(),
            serde_json::Value::String(tools_used.to_string()),
        );
        let input = ExternalHookInput {
            event: "post_response".to_string(),
            session_id: session_key.to_string(),
            user_message: response_content.to_string(),
            metadata: serde_json::Value::Object(metadata),
        };
        self.run_script("post_response.sh", &input).await
    }

    /// Core execution engine: run a script with JSON input, parse JSON output.
    async fn run_script(
        &self,
        script_name: &str,
        input: &ExternalHookInput,
    ) -> anyhow::Result<Option<ExternalHookOutput>> {
        let hooks_dir = match &self.hooks_dir {
            Some(dir) => dir,
            None => return Ok(None),
        };

        let script_path = hooks_dir.join(script_name);

        let json_input = serde_json::to_string(input)?;
        debug!(
            "Running hook: {} (input: {} bytes)",
            script_name,
            json_input.len()
        );

        // Spawn the subprocess directly - let the OS handle shebang interpretation.
        // This ensures timeout kill signals reach the actual script process,
        // not an intermediate shell wrapper that would leave orphan processes.
        let mut child = Command::new(&script_path)
            .kill_on_drop(true) // Ensure process is killed if timeout occurs
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        // Write JSON to stdin in background to avoid IPC deadlock
        if let Some(mut stdin) = child.stdin.take() {
            let json_string = json_input.clone();
            tokio::spawn(async move {
                let _ = stdin.write_all(json_string.as_bytes()).await;
                // stdin dropped here, sending EOF
            });
        }

        // Wait with timeout - kill_on_drop(true) ensures the process is killed on timeout
        let output = match tokio::time::timeout(HOOK_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Err(anyhow::anyhow!(
                    "Hook {} failed to execute: {}",
                    script_name,
                    e
                ));
            }
            Err(_) => {
                // Timeout — kill_on_drop(true) will kill the process when child is dropped
                warn!(
                    "Hook {} timed out after {:?}, process killed",
                    script_name, HOOK_TIMEOUT
                );
                return Err(anyhow::anyhow!(
                    "Hook {} timed out after {:?}",
                    script_name,
                    HOOK_TIMEOUT
                ));
            }
        };

        // Log stderr (debugging info from the script)
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        if !stderr_str.is_empty() {
            debug!("Hook {} stderr: {}", script_name, stderr_str.trim());
        }

        // Check exit code
        if !output.status.success() {
            warn!(
                "Hook {} exited with status {}: {}",
                script_name,
                output.status,
                stderr_str.trim()
            );
            return Err(anyhow::anyhow!(
                "Hook {} exited with status {}",
                script_name,
                output.status
            ));
        }

        // Parse stdout
        let stdout = &output.stdout;
        if stdout.is_empty() || stdout.len() > MAX_STDOUT_BYTES {
            if stdout.len() > MAX_STDOUT_BYTES {
                warn!(
                    "Hook {} stdout exceeds {} bytes, ignoring",
                    script_name, MAX_STDOUT_BYTES
                );
            }
            return Ok(None); // Empty stdout = "do nothing, continue"
        }

        let stdout_str = String::from_utf8_lossy(stdout);
        let stdout_trimmed = stdout_str.trim();
        if stdout_trimmed.is_empty() {
            return Ok(None);
        }

        match serde_json::from_str::<ExternalHookOutput>(stdout_trimmed) {
            Ok(output) => {
                debug!("Hook {} output: action={}", script_name, output.action);
                Ok(Some(output))
            }
            Err(e) => {
                warn!(
                    "Hook {} produced invalid JSON (ignoring): {} — raw: {}",
                    script_name, e, stdout_trimmed
                );
                Ok(None)
            }
        }
    }
}

/// Check if a file is executable (Unix only).
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

/// On non-Unix platforms, assume scripts are executable.
#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

// ── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_input_serialization() {
        let input = ExternalHookInput {
            event: "pre_request".to_string(),
            session_id: "telegram:12345".to_string(),
            user_message: "hello world".to_string(),
            metadata: serde_json::json!({}),
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("pre_request"));
        assert!(json.contains("telegram:12345"));
        assert!(json.contains("hello world"));
    }

    #[test]
    fn test_hook_output_deserialization_continue() {
        let json = r#"{"action": "continue"}"#;
        let output: ExternalHookOutput = serde_json::from_str(json).unwrap();
        assert_eq!(output.action, "continue");
        assert!(!output.is_abort());
        assert!(output.modified_message.is_none());
        assert!(output.error.is_none());
    }

    #[test]
    fn test_hook_output_deserialization_abort() {
        let json = r#"{"action": "abort", "error": "禁止操作"}"#;
        let output: ExternalHookOutput = serde_json::from_str(json).unwrap();
        assert!(output.is_abort());
        assert_eq!(output.error, Some("禁止操作".to_string()));
    }

    #[test]
    fn test_hook_output_deserialization_with_modified_message() {
        let json = r#"{"action": "continue", "modified_message": "sanitized input"}"#;
        let output: ExternalHookOutput = serde_json::from_str(json).unwrap();
        assert!(!output.is_abort());
        assert_eq!(output.modified_message, Some("sanitized input".to_string()));
    }

    #[test]
    fn test_hook_output_default_action() {
        let json = r#"{}"#;
        let output: ExternalHookOutput = serde_json::from_str(json).unwrap();
        assert_eq!(output.action, "continue");
        assert!(!output.is_abort());
    }

    #[test]
    fn test_noop_runner() {
        let runner = ExternalHookRunner::noop();
        assert!(runner.hooks_dir.is_none());
        assert!(!runner.script_availability.has_pre_request);
        assert!(!runner.script_availability.has_post_response);
    }

    #[tokio::test]
    async fn test_noop_runner_returns_none() {
        let runner = ExternalHookRunner::noop();
        let result = runner.run_pre_request("test:session", "hello").await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_missing_hooks_dir_returns_none() {
        let runner = ExternalHookRunner::new(PathBuf::from("/nonexistent/path"));
        assert!(!runner.script_availability.has_pre_request);
        assert!(!runner.script_availability.has_post_response);
        let result = runner.run_pre_request("test:session", "hello").await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
