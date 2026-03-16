use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Skill metadata extracted from YAML frontmatter
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillMetadata {
    /// Unique skill name
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// Whether to always load the full skill content
    #[serde(default)]
    pub always: bool,

    /// Required binary commands (e.g., ["gh", "git"])
    #[serde(default)]
    pub bins: Vec<String>,

    /// Required environment variables (e.g., ["GITHUB_TOKEN"])
    #[serde(default)]
    pub env_vars: Vec<String>,

    /// Additional custom metadata
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

impl SkillMetadata {
    /// Check if all dependencies are available
    pub fn check_dependencies(&self) -> Result<(), Vec<String>> {
        let mut missing = Vec::new();

        // Check binary dependencies
        for bin in &self.bins {
            if which::which(bin).is_err() {
                missing.push(format!("binary '{}' not found in PATH", bin));
            }
        }

        // Check environment variable dependencies
        for env_var in &self.env_vars {
            if std::env::var(env_var).is_err() {
                missing.push(format!("environment variable '{}' not set", env_var));
            }
        }

        if missing.is_empty() {
            Ok(())
        } else {
            Err(missing)
        }
    }

    /// Check if the skill is available (all dependencies satisfied)
    pub fn is_available(&self) -> bool {
        self.check_dependencies().is_ok()
    }

    /// Get list of missing dependencies
    pub fn missing_dependencies(&self) -> Vec<String> {
        self.check_dependencies().err().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_default_metadata() {
        let meta = SkillMetadata::default();
        assert!(!meta.always);
        assert!(meta.bins.is_empty());
        assert!(meta.env_vars.is_empty());
    }

    #[test]
    fn test_check_dependencies_env_var() {
        let meta = SkillMetadata {
            env_vars: vec!["GASKET_TEST_VAR_12345".to_string()],
            ..Default::default()
        };

        // Should fail - env var not set
        assert!(meta.check_dependencies().is_err());

        // Set the env var
        env::set_var("GASKET_TEST_VAR_12345", "test");

        // Should pass now
        assert!(meta.check_dependencies().is_ok());

        // Clean up
        env::remove_var("GASKET_TEST_VAR_12345");
    }

    #[test]
    fn test_is_available() {
        let mut meta = SkillMetadata {
            name: "test-skill".to_string(),
            description: "Test skill".to_string(),
            ..Default::default()
        };

        assert!(meta.is_available());

        // Add non-existent binary
        meta.bins.push("nonexistent-binary-xyz".to_string());
        assert!(!meta.is_available());
    }
}
