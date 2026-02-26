//! Resource limits for shell command execution.
//!
//! Two enforcement layers:
//! - **Inner**: `ulimit` prefix (fallback mode) or bwrap `--rlimit-*` flags (sandbox mode)
//! - **Outer**: tokio wall-clock timeout (always applied by ExecTool)

use crate::config::ResourceLimitsConfig;

/// Resource limits to apply to a child process.
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Maximum virtual memory in bytes
    pub max_memory_bytes: u64,
    /// Maximum CPU time in seconds
    pub max_cpu_secs: u32,
    /// Maximum output size in bytes (applied after execution)
    pub max_output_bytes: usize,
}

impl ResourceLimits {
    pub fn from_config(config: &ResourceLimitsConfig) -> Self {
        Self {
            max_memory_bytes: u64::from(config.max_memory_mb) * 1024 * 1024,
            max_cpu_secs: config.max_cpu_secs,
            max_output_bytes: config.max_output_bytes,
        }
    }

    /// Generate a `ulimit` prefix string for fallback (non-sandboxed) mode.
    ///
    /// Example output: `ulimit -v 524288 -t 60; `
    ///
    /// `-v` sets virtual memory limit in KB, `-t` sets CPU time in seconds.
    pub fn to_ulimit_prefix(&self) -> String {
        let mem_kb = self.max_memory_bytes / 1024;
        format!("ulimit -v {} -t {}; ", mem_kb, self.max_cpu_secs)
    }

    /// Generate bwrap `--rlimit-*` command-line arguments for sandbox mode.
    pub fn to_bwrap_args(&self) -> Vec<String> {
        vec![
            "--rlimit-as".to_string(),
            self.max_memory_bytes.to_string(),
            "--rlimit-cpu".to_string(),
            self.max_cpu_secs.to_string(),
        ]
    }

    /// Truncate output to `max_output_bytes`, appending a marker if truncated.
    pub fn truncate_output(&self, output: &str) -> String {
        if output.len() <= self.max_output_bytes {
            return output.to_string();
        }
        let mut truncated = output[..self.max_output_bytes].to_string();
        truncated.push_str(&format!(
            "\n\n[OUTPUT TRUNCATED: {} bytes exceeded limit of {} bytes]",
            output.len(),
            self.max_output_bytes
        ));
        truncated
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self::from_config(&ResourceLimitsConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ulimit_prefix() {
        let limits = ResourceLimits {
            max_memory_bytes: 512 * 1024 * 1024, // 512 MB
            max_cpu_secs: 60,
            max_output_bytes: 1_048_576,
        };
        let prefix = limits.to_ulimit_prefix();
        assert_eq!(prefix, "ulimit -v 524288 -t 60; ");
    }

    #[test]
    fn test_bwrap_args() {
        let limits = ResourceLimits {
            max_memory_bytes: 512 * 1024 * 1024,
            max_cpu_secs: 60,
            max_output_bytes: 1_048_576,
        };
        let args = limits.to_bwrap_args();
        assert_eq!(
            args,
            vec![
                "--rlimit-as",
                "536870912",
                "--rlimit-cpu",
                "60",
            ]
        );
    }

    #[test]
    fn test_truncate_output_within_limit() {
        let limits = ResourceLimits {
            max_memory_bytes: 0,
            max_cpu_secs: 0,
            max_output_bytes: 100,
        };
        let output = "short output";
        assert_eq!(limits.truncate_output(output), output);
    }

    #[test]
    fn test_truncate_output_exceeds_limit() {
        let limits = ResourceLimits {
            max_memory_bytes: 0,
            max_cpu_secs: 0,
            max_output_bytes: 10,
        };
        let output = "this is a long output that exceeds the limit";
        let result = limits.truncate_output(output);
        assert!(result.starts_with("this is a "));
        assert!(result.contains("[OUTPUT TRUNCATED"));
    }

    #[test]
    fn test_from_config() {
        let config = ResourceLimitsConfig {
            max_memory_mb: 1024,
            max_cpu_secs: 30,
            max_output_bytes: 2_097_152,
        };
        let limits = ResourceLimits::from_config(&config);
        assert_eq!(limits.max_memory_bytes, 1024 * 1024 * 1024);
        assert_eq!(limits.max_cpu_secs, 30);
        assert_eq!(limits.max_output_bytes, 2_097_152);
    }
}
