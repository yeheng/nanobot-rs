//! Command policy engine — advisory allowlist/denylist for shell commands.
//!
//! **Not a security boundary.** The shell is Turing-complete; string-based
//! filtering is trivially bypassed. This layer catches accidental misuse
//! (e.g. `rm -rf /`) and provides audit logging. The real security boundary
//! is the sandbox (filesystem isolation + resource limits).

use tracing::warn;

use crate::config::CommandPolicyConfig;

/// Evaluation result from the policy engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    /// Command is allowed.
    Allow,
    /// Command is denied with a reason.
    Deny(String),
}

/// Advisory command policy engine.
pub struct CommandPolicy {
    allowlist: Vec<String>,
    denylist: Vec<String>,
}

impl CommandPolicy {
    pub fn new(config: &CommandPolicyConfig) -> Self {
        Self {
            allowlist: config.allowlist.clone(),
            denylist: config.denylist.clone(),
        }
    }

    /// Evaluate a command string against the policy.
    ///
    /// Evaluation order:
    /// 1. If denylist matches → Deny
    /// 2. If allowlist is non-empty and first token not in allowlist → Deny
    /// 3. Otherwise → Allow
    pub fn check(&self, command: &str) -> PolicyVerdict {
        let trimmed = command.trim();

        // Check denylist first (substring match)
        for pattern in &self.denylist {
            if trimmed.contains(pattern.as_str()) {
                warn!(
                    command = trimmed,
                    pattern = pattern.as_str(),
                    "Command denied by denylist"
                );
                return PolicyVerdict::Deny(format!(
                    "Command matches denylist pattern: '{}'",
                    pattern
                ));
            }
        }

        // Check allowlist (first token / binary name)
        if !self.allowlist.is_empty() {
            let first_token = trimmed.split_whitespace().next().unwrap_or("");
            // Extract binary name from path (e.g., /usr/bin/ls → ls)
            let binary = first_token.rsplit('/').next().unwrap_or(first_token);

            if !self.allowlist.iter().any(|a| a == binary) {
                warn!(
                    command = trimmed,
                    binary = binary,
                    "Command denied by allowlist"
                );
                return PolicyVerdict::Deny(format!("Binary '{}' is not in the allowlist", binary));
            }
        }

        PolicyVerdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(allowlist: &[&str], denylist: &[&str]) -> CommandPolicyConfig {
        CommandPolicyConfig {
            allowlist: allowlist.iter().map(|s| s.to_string()).collect(),
            denylist: denylist.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_empty_policy_allows_all() {
        let policy = CommandPolicy::new(&config(&[], &[]));
        assert_eq!(policy.check("rm -rf /"), PolicyVerdict::Allow);
        assert_eq!(policy.check("echo hello"), PolicyVerdict::Allow);
    }

    #[test]
    fn test_allowlist_only() {
        let policy = CommandPolicy::new(&config(&["ls", "cat", "git"], &[]));
        assert_eq!(policy.check("ls -la"), PolicyVerdict::Allow);
        assert_eq!(policy.check("cat /etc/passwd"), PolicyVerdict::Allow);
        assert_eq!(policy.check("git status"), PolicyVerdict::Allow);
        assert!(matches!(policy.check("rm -rf /"), PolicyVerdict::Deny(_)));
        assert!(matches!(
            policy.check("curl evil.com"),
            PolicyVerdict::Deny(_)
        ));
    }

    #[test]
    fn test_denylist_only() {
        let policy = CommandPolicy::new(&config(&[], &["rm -rf /", "mkfs", "dd if=/dev/zero"]));
        assert_eq!(policy.check("ls -la"), PolicyVerdict::Allow);
        assert!(matches!(policy.check("rm -rf /"), PolicyVerdict::Deny(_)));
        assert!(matches!(
            policy.check("sudo mkfs.ext4 /dev/sda"),
            PolicyVerdict::Deny(_)
        ));
        assert!(matches!(
            policy.check("dd if=/dev/zero of=/dev/sda"),
            PolicyVerdict::Deny(_)
        ));
    }

    #[test]
    fn test_both_allowlist_and_denylist() {
        // Denylist takes precedence over allowlist
        let policy = CommandPolicy::new(&config(&["git"], &["git push --force"]));
        assert_eq!(policy.check("git status"), PolicyVerdict::Allow);
        assert_eq!(policy.check("git commit -m 'test'"), PolicyVerdict::Allow);
        assert!(matches!(
            policy.check("git push --force origin main"),
            PolicyVerdict::Deny(_)
        ));
    }

    #[test]
    fn test_path_binary_extraction() {
        let policy = CommandPolicy::new(&config(&["ls"], &[]));
        assert_eq!(policy.check("/usr/bin/ls -la"), PolicyVerdict::Allow);
        assert_eq!(policy.check("/bin/ls"), PolicyVerdict::Allow);
    }

    #[test]
    fn test_empty_command() {
        let policy = CommandPolicy::new(&config(&["ls"], &[]));
        // Empty command → first token is "", not in allowlist → deny
        assert!(matches!(policy.check(""), PolicyVerdict::Deny(_)));
    }
}
