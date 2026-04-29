//! Resource limits configuration and enforcement
//!
//! Two enforcement layers:
//! - **Inner**: `ulimit` prefix (fallback mode) or bwrap `--rlimit-*` flags (sandbox mode)
//! - **Outer**: tokio wall-clock timeout (always applied by executor)

use serde::{Deserialize, Serialize};

/// Resource limits to apply to a child process.
///
/// All size fields use user-friendly units (MB) for consistency.
/// Byte conversion happens in `to_ulimit_prefix()` and `to_bwrap_args()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum virtual memory in MB (default: 512)
    #[serde(default = "default_max_memory_mb")]
    pub max_memory_mb: u32,

    /// Maximum CPU time in seconds (default: 60)
    #[serde(default = "default_max_cpu_secs")]
    pub max_cpu_secs: u32,

    /// Maximum output size in bytes (default: 1 MB, applied after execution)
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,

    /// Maximum number of processes (default: 0 = unlimited)
    #[serde(default = "default_max_processes")]
    pub max_processes: u32,

    /// Maximum file size in MB (default: 0 = unlimited)
    #[serde(default)]
    pub max_file_size_mb: u32,

    /// Maximum number of open files (default: 64)
    #[serde(default = "default_max_open_files")]
    pub max_open_files: u32,
}

fn default_max_memory_mb() -> u32 {
    512
}

fn default_max_cpu_secs() -> u32 {
    60
}

fn default_max_output_bytes() -> usize {
    1_048_576 // 1 MB
}

fn default_max_processes() -> u32 {
    0 // 0 = unlimited; macOS has ~800+ processes at idle, low values cause fork failures
}

fn default_max_open_files() -> u32 {
    64
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_mb: default_max_memory_mb(),
            max_cpu_secs: default_max_cpu_secs(),
            max_output_bytes: default_max_output_bytes(),
            max_processes: default_max_processes(),
            max_file_size_mb: 0,
            max_open_files: default_max_open_files(),
        }
    }
}

impl ResourceLimits {
    /// Generate a `ulimit` prefix string for fallback (non-sandboxed) mode.
    pub fn to_ulimit_prefix(&self) -> String {
        let mut parts = vec![];

        #[cfg(not(target_os = "macos"))]
        {
            let mem_kb = u64::from(self.max_memory_mb) * 1024;
            parts.push(format!("ulimit -v {} -t {}", mem_kb, self.max_cpu_secs));
        }

        #[cfg(target_os = "macos")]
        {
            parts.push(format!("ulimit -t {}", self.max_cpu_secs));
        }

        if self.max_processes > 0 {
            parts.push(format!("-u {}", self.max_processes));
        }

        if self.max_open_files > 0 {
            parts.push(format!("-n {}", self.max_open_files));
        }

        if self.max_file_size_mb > 0 {
            let file_size_kb = u64::from(self.max_file_size_mb) * 1024;
            parts.push(format!("-f {}", file_size_kb));
        }

        format!("{} 2>/dev/null; ", parts.join(" "))
    }

    /// Generate bwrap `--rlimit-*` command-line arguments for sandbox mode.
    pub fn to_bwrap_args(&self) -> Vec<String> {
        let mem_bytes = u64::from(self.max_memory_mb) * 1024 * 1024;
        let mut args = vec![
            "--rlimit-as".to_string(),
            mem_bytes.to_string(),
            "--rlimit-cpu".to_string(),
            self.max_cpu_secs.to_string(),
        ];

        if self.max_processes > 0 {
            args.extend(["--rlimit-nproc".to_string(), self.max_processes.to_string()]);
        }

        if self.max_open_files > 0 {
            args.extend([
                "--rlimit-nofile".to_string(),
                self.max_open_files.to_string(),
            ]);
        }

        if self.max_file_size_mb > 0 {
            let fsize_bytes = u64::from(self.max_file_size_mb) * 1024 * 1024;
            args.extend(["--rlimit-fsize".to_string(), fsize_bytes.to_string()]);
        }

        args
    }

    /// Truncate output to `max_output_bytes`, appending a marker if truncated.
    pub fn truncate_output(&self, output: &str) -> String {
        if output.len() <= self.max_output_bytes {
            return output.to_string();
        }

        let end = output.floor_char_boundary(self.max_output_bytes);

        let mut truncated = output[..end].to_string();
        truncated.push_str(&format!(
            "\n\n[OUTPUT TRUNCATED: {} bytes exceeded limit of {} bytes]",
            output.len(),
            self.max_output_bytes
        ));
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ulimit_prefix() {
        let limits = ResourceLimits::default();
        let prefix = limits.to_ulimit_prefix();
        assert!(prefix.contains("ulimit"));
        assert!(prefix.contains("-t 60"));
        assert!(prefix.contains("2>/dev/null"));
    }

    #[test]
    fn test_bwrap_args() {
        let limits = ResourceLimits {
            max_memory_mb: 512,
            ..Default::default()
        };
        let args = limits.to_bwrap_args();
        assert!(args.contains(&"--rlimit-as".to_string()));
        assert!(args.contains(&"536870912".to_string())); // 512 * 1024 * 1024
        assert!(args.contains(&"--rlimit-cpu".to_string()));
        assert!(args.contains(&"60".to_string()));
    }

    #[test]
    fn test_truncate_output_within_limit() {
        let limits = ResourceLimits {
            max_output_bytes: 100,
            ..Default::default()
        };
        let output = "short output";
        assert_eq!(limits.truncate_output(output), output);
    }

    #[test]
    fn test_truncate_output_exceeds_limit() {
        let limits = ResourceLimits {
            max_output_bytes: 10,
            ..Default::default()
        };
        let output = "this is a long output that exceeds the limit";
        let result = limits.truncate_output(output);
        assert!(result.starts_with("this is a "));
        assert!(result.contains("[OUTPUT TRUNCATED"));
    }

    #[test]
    fn test_truncate_output_utf8_boundary_safe() {
        let limits = ResourceLimits {
            max_output_bytes: 5,
            ..Default::default()
        };
        let output = "中文字符";
        let result = limits.truncate_output(output);
        assert!(result.starts_with("中"));
        assert!(result.contains("[OUTPUT TRUNCATED"));
    }

    #[test]
    fn test_default_values() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.max_memory_mb, 512);
        assert_eq!(limits.max_cpu_secs, 60);
        assert_eq!(limits.max_output_bytes, 1_048_576);
        assert_eq!(limits.max_processes, 0);
        assert_eq!(limits.max_file_size_mb, 0);
        assert_eq!(limits.max_open_files, 64);
    }
}
