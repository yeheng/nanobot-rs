//! Resource limits configuration and enforcement
//!
//! Two enforcement layers:
//! - **Inner**: `ulimit` prefix (fallback mode) or bwrap `--rlimit-*` flags (sandbox mode)
//! - **Outer**: tokio wall-clock timeout (always applied by executor)

use serde::{Deserialize, Serialize};

/// Resource limits to apply to a child process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum virtual memory in bytes
    #[serde(default = "default_max_memory_bytes")]
    pub max_memory_bytes: u64,

    /// Maximum CPU time in seconds
    #[serde(default = "default_max_cpu_secs")]
    pub max_cpu_secs: u32,

    /// Maximum output size in bytes (applied after execution)
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,

    /// Maximum number of processes
    #[serde(default = "default_max_processes")]
    pub max_processes: u32,

    /// Maximum file size in bytes (0 = unlimited)
    #[serde(default)]
    pub max_file_size_bytes: u64,

    /// Maximum number of open files
    #[serde(default = "default_max_open_files")]
    pub max_open_files: u32,
}

fn default_max_memory_bytes() -> u64 {
    512 * 1024 * 1024 // 512 MB
}

fn default_max_cpu_secs() -> u32 {
    60
}

fn default_max_output_bytes() -> usize {
    1_048_576 // 1 MB
}

fn default_max_processes() -> u32 {
    10
}

fn default_max_open_files() -> u32 {
    64
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: default_max_memory_bytes(),
            max_cpu_secs: default_max_cpu_secs(),
            max_output_bytes: default_max_output_bytes(),
            max_processes: default_max_processes(),
            max_file_size_bytes: 0,
            max_open_files: default_max_open_files(),
        }
    }
}

impl ResourceLimits {
    /// Create from MB values for convenience
    pub fn from_mb(max_memory_mb: u32, max_cpu_secs: u32, max_output_bytes: usize) -> Self {
        Self {
            max_memory_bytes: u64::from(max_memory_mb) * 1024 * 1024,
            max_cpu_secs,
            max_output_bytes,
            ..Default::default()
        }
    }

    /// Generate a `ulimit` prefix string for fallback (non-sandboxed) mode.
    ///
    /// Example output: `ulimit -v 524288 -t 60; `
    ///
    /// `-v` sets virtual memory limit in KB, `-t` sets CPU time in seconds.
    pub fn to_ulimit_prefix(&self) -> String {
        let mem_kb = self.max_memory_bytes / 1024;
        let mut parts = vec![format!("ulimit -v {} -t {}", mem_kb, self.max_cpu_secs)];

        if self.max_processes > 0 {
            parts.push(format!("-u {}", self.max_processes));
        }

        if self.max_open_files > 0 {
            parts.push(format!("-n {}", self.max_open_files));
        }

        if self.max_file_size_bytes > 0 {
            let file_size_kb = self.max_file_size_bytes / 1024;
            parts.push(format!("-f {}", file_size_kb));
        }

        format!("{}; ", parts.join(" "))
    }

    /// Generate bwrap `--rlimit-*` command-line arguments for sandbox mode.
    pub fn to_bwrap_args(&self) -> Vec<String> {
        let mut args = vec![
            "--rlimit-as".to_string(),
            self.max_memory_bytes.to_string(),
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

        if self.max_file_size_bytes > 0 {
            args.extend([
                "--rlimit-fsize".to_string(),
                self.max_file_size_bytes.to_string(),
            ]);
        }

        args
    }

    /// Truncate output to `max_output_bytes`, appending a marker if truncated.
    ///
    /// SAFETY: This function correctly handles UTF-8 character boundaries.
    /// If `max_output_bytes` falls in the middle of a multi-byte character,
    /// we walk back to the nearest safe boundary.
    pub fn truncate_output(&self, output: &str) -> String {
        if output.len() <= self.max_output_bytes {
            return output.to_string();
        }

        // Find a safe UTF-8 boundary by walking backwards from max_output_bytes.
        // Rust strings are UTF-8 encoded, so slicing at arbitrary byte offsets
        // can panic if we split a multi-byte character.
        let mut end = self.max_output_bytes;
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }

        let mut truncated = output[..end].to_string();
        truncated.push_str(&format!(
            "\n\n[OUTPUT TRUNCATED: {} bytes exceeded limit of {} bytes]",
            output.len(),
            self.max_output_bytes
        ));
        truncated
    }
}

/// Configuration for resource limits (for deserialization from config files)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimitsConfig {
    /// Maximum memory in MB (default: 512)
    #[serde(default = "default_max_memory_mb")]
    pub max_memory_mb: u32,

    /// Maximum CPU time in seconds (default: 60)
    #[serde(default = "default_max_cpu_secs")]
    pub max_cpu_secs: u32,

    /// Maximum output size in bytes (default: 1MB)
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,

    /// Maximum number of processes (default: 10)
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

impl Default for ResourceLimitsConfig {
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

impl From<&ResourceLimitsConfig> for ResourceLimits {
    fn from(config: &ResourceLimitsConfig) -> Self {
        Self {
            max_memory_bytes: u64::from(config.max_memory_mb) * 1024 * 1024,
            max_cpu_secs: config.max_cpu_secs,
            max_output_bytes: config.max_output_bytes,
            max_processes: config.max_processes,
            max_file_size_bytes: u64::from(config.max_file_size_mb) * 1024 * 1024,
            max_open_files: config.max_open_files,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ulimit_prefix() {
        let limits = ResourceLimits::from_mb(512, 60, 1_048_576);
        let prefix = limits.to_ulimit_prefix();
        assert!(prefix.contains("ulimit -v 524288"));
        assert!(prefix.contains("-t 60"));
    }

    #[test]
    fn test_bwrap_args() {
        let limits = ResourceLimits::from_mb(512, 60, 1_048_576);
        let args = limits.to_bwrap_args();
        assert!(args.contains(&"--rlimit-as".to_string()));
        assert!(args.contains(&"536870912".to_string()));
        assert!(args.contains(&"--rlimit-cpu".to_string()));
        assert!(args.contains(&"60".to_string()));
    }

    #[test]
    fn test_truncate_output_within_limit() {
        let limits = ResourceLimits::from_mb(0, 0, 100);
        let output = "short output";
        assert_eq!(limits.truncate_output(output), output);
    }

    #[test]
    fn test_truncate_output_exceeds_limit() {
        let limits = ResourceLimits::from_mb(0, 0, 10);
        let output = "this is a long output that exceeds the limit";
        let result = limits.truncate_output(output);
        assert!(result.starts_with("this is a "));
        assert!(result.contains("[OUTPUT TRUNCATED"));
    }

    #[test]
    fn test_truncate_output_utf8_boundary_safe() {
        // "中" is 3 bytes, "文" is 3 bytes, "字" is 3 bytes
        // "中文字" = 9 bytes total
        let limits = ResourceLimits::from_mb(0, 0, 5); // Cuts in the middle of "文"
        let output = "中文字符";
        let result = limits.truncate_output(output);
        // Should truncate to "中" (3 bytes) not panic
        assert!(result.starts_with("中"));
        assert!(result.contains("[OUTPUT TRUNCATED"));
    }

    #[test]
    fn test_from_config() {
        let config = ResourceLimitsConfig {
            max_memory_mb: 1024,
            max_cpu_secs: 30,
            max_output_bytes: 2_097_152,
            max_processes: 20,
            max_file_size_mb: 100,
            max_open_files: 128,
        };
        let limits = ResourceLimits::from(&config);
        assert_eq!(limits.max_memory_bytes, 1024 * 1024 * 1024);
        assert_eq!(limits.max_cpu_secs, 30);
        assert_eq!(limits.max_output_bytes, 2_097_152);
        assert_eq!(limits.max_processes, 20);
        assert_eq!(limits.max_file_size_bytes, 100 * 1024 * 1024);
        assert_eq!(limits.max_open_files, 128);
    }
}
