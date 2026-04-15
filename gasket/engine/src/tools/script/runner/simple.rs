//! Simple mode runner for script tools.
//!
//! One-shot execution with JSON input/output via stdin/stdout.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::tools::script::manifest::ScriptManifest;
use crate::tools::script::rpc::MAX_MESSAGE_SIZE;
use crate::tools::script::runner::{ScriptError, ScriptResult};

/// Run a script in simple mode (one-shot JSON input/output).
///
/// # Arguments
///
/// * `manifest` - Script manifest with runtime configuration
/// * `manifest_dir` - Directory containing the manifest (for resolving working_dir)
/// * `args` - Parameters to send to the script as JSON
/// * `timeout_secs` - Maximum execution time before killing the process
///
/// # Returns
///
/// - `Ok(ScriptResult)` - Script completed successfully with parsed output
/// - `Err(ScriptError)` - Spawn failed, timeout, non-zero exit, or invalid JSON
///
/// # Protocol
///
/// 1. Spawn process with piped stdin/stdout/stderr
/// 2. Write `args` as JSON to stdin, then close stdin
/// 3. Wait for process completion (with timeout)
/// 4. Parse stdout as JSON
/// 5. Collect stderr separately
pub async fn run_simple(
    manifest: &ScriptManifest,
    manifest_dir: &Path,
    args: &Value,
    timeout_secs: u64,
) -> Result<ScriptResult, ScriptError> {
    let start = std::time::Instant::now();
    let mut child = spawn_process(manifest, manifest_dir)?;

    // Write args to stdin and close it
    let stdin = child
        .stdin
        .as_mut()
        .ok_or_else(|| ScriptError::Io("Failed to open stdin".to_string()))?;

    let args_json = serde_json::to_string(args)
        .map_err(|e| ScriptError::Io(format!("Failed to serialize args: {}", e)))?;
    stdin
        .write_all(args_json.as_bytes())
        .await
        .map_err(|e| ScriptError::Io(format!("Failed to write to stdin: {}", e)))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| ScriptError::Io(format!("Failed to write newline to stdin: {}", e)))?;
    let _ = stdin;

    // Wait for completion with timeout
    let timeout_duration = Duration::from_secs(timeout_secs);
    let result = match timeout(timeout_duration, child.wait()).await {
        Ok(Ok(exit_status)) => exit_status,
        Ok(Err(e)) => {
            return Err(ScriptError::Io(format!(
                "Failed to wait for process: {}",
                e
            )))
        }
        Err(_) => {
            child
                .kill()
                .await
                .map_err(|e| ScriptError::Io(format!("Failed to kill timed-out process: {}", e)))?;
            return Err(ScriptError::Timeout(timeout_secs));
        }
    };

    if !result.success() {
        return Err(ScriptError::NonZeroExit(result.code()));
    }

    // Collect stdout and stderr
    let stdout = child
        .stdout
        .ok_or_else(|| ScriptError::Io("Stdout not captured".to_string()))?;
    let stderr = child
        .stderr
        .ok_or_else(|| ScriptError::Io("Stderr not captured".to_string()))?;

    // Read stdout as JSON (with bounded line length to prevent OOM)
    let mut reader = FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_MESSAGE_SIZE));
    let stdout_line = match reader.next().await {
        Some(Ok(line)) => line,
        Some(Err(e)) => {
            return Err(ScriptError::Io(format!("Failed to read stdout: {}", e)));
        }
        None => {
            return Err(ScriptError::InvalidOutput(
                "Empty output from script".to_string(),
            ));
        }
    };

    let output: Value = serde_json::from_str(&stdout_line)
        .map_err(|e| ScriptError::InvalidOutput(format!("JSON parse error: {}", e)))?;

    // Read stderr
    let mut stderr_reader = BufReader::new(stderr);
    let mut stderr_text = String::new();
    stderr_reader
        .read_to_string(&mut stderr_text)
        .await
        .map_err(|e| ScriptError::Io(format!("Failed to read stderr: {}", e)))?;

    Ok(ScriptResult {
        output,
        stderr: stderr_text,
        duration: start.elapsed(),
    })
}

/// Spawn a child process from the manifest configuration.
pub(super) fn spawn_process(
    manifest: &ScriptManifest,
    manifest_dir: &Path,
) -> Result<Child, ScriptError> {
    let working_dir = if manifest.runtime.working_dir == "." {
        manifest_dir.to_path_buf()
    } else {
        manifest_dir.join(&manifest.runtime.working_dir)
    };

    let mut cmd = Command::new(&manifest.runtime.command);
    cmd.args(&manifest.runtime.args)
        .current_dir(&working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    for (key, value) in &manifest.runtime.env {
        cmd.env(key, value);
    }

    cmd.spawn().map_err(|e| {
        ScriptError::SpawnFailed(format!(
            "Failed to spawn '{}': {}",
            manifest.runtime.command, e
        ))
    })
}

#[cfg(test)]
mod tests {
    use crate::tools::{RuntimeConfig, ScriptProtocol};

    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn test_manifest(command: &str) -> (ScriptManifest, TempDir) {
        let dir = TempDir::new().unwrap();
        let manifest = ScriptManifest {
            name: "test_tool".to_string(),
            description: "Test tool".to_string(),
            version: "1.0.0".to_string(),
            runtime: RuntimeConfig {
                command: command.to_string(),
                args: vec![],
                working_dir: ".".to_string(),
                timeout_secs: 120,
                env: Default::default(),
            },
            protocol: ScriptProtocol::Simple,
            parameters: serde_json::json!({}),
            permissions: vec![],
        };
        (manifest, dir)
    }

    #[tokio::test]
    async fn test_simple_mode_cat() {
        let (manifest, dir) = test_manifest("cat");
        let args = json!({"hello": "world", "number": 42});

        let result = run_simple(&manifest, dir.path(), &args, 5)
            .await
            .expect("Script execution failed");

        assert_eq!(result.output, args);
        assert_eq!(result.stderr, "");
    }

    #[tokio::test]
    async fn test_simple_mode_timeout() {
        let dir = TempDir::new().unwrap();
        let manifest = ScriptManifest {
            name: "test_tool".to_string(),
            description: "Test tool".to_string(),
            version: "1.0.0".to_string(),
            runtime: RuntimeConfig {
                command: "sleep".to_string(),
                args: vec!["10".to_string()],
                working_dir: ".".to_string(),
                timeout_secs: 120,
                env: Default::default(),
            },
            protocol: ScriptProtocol::Simple,
            parameters: serde_json::json!({}),
            permissions: vec![],
        };

        let args = json!("unused");
        let timeout_secs = 1;

        let result = run_simple(&manifest, dir.path(), &args, timeout_secs).await;

        match result {
            Err(ScriptError::Timeout(t)) => assert_eq!(t, timeout_secs),
            other => panic!("Expected Timeout error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_spawn_fails_for_bad_command() {
        let (manifest, dir) = test_manifest("nonexistent_command_xyz123");
        let args = json!({});
        let result = run_simple(&manifest, dir.path(), &args, 5).await;

        match result {
            Err(ScriptError::SpawnFailed(_)) => {}
            other => panic!("Expected SpawnFailed error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_concurrent_invocations_are_isolated() {
        let (manifest, dir) = test_manifest("cat");
        let args1 = json!({"invocation": 1, "data": "first"});
        let args2 = json!({"invocation": 2, "data": "second"});

        let manifest1 = manifest.clone();
        let dir1 = dir.path().to_path_buf();
        let args1_clone = args1.clone();
        let handle1 =
            tokio::spawn(async move { run_simple(&manifest1, &dir1, &args1_clone, 5).await });

        let manifest2 = manifest.clone();
        let dir2 = dir.path().to_path_buf();
        let args2_clone = args2.clone();
        let handle2 =
            tokio::spawn(async move { run_simple(&manifest2, &dir2, &args2_clone, 5).await });

        let result1 = handle1.await.unwrap().expect("First invocation failed");
        let result2 = handle2.await.unwrap().expect("Second invocation failed");

        assert_eq!(result1.output, args1, "First invocation output mismatch");
        assert_eq!(result2.output, args2, "Second invocation output mismatch");
    }
}
