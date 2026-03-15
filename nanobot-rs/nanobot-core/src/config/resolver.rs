//! Config vault placeholder resolver
//!
//! Provides functionality to resolving `{{vault:key}}` placeholders
//! in configuration values.
//!
//! # Usage
//!
//! ```ignore
//! use nanobot_core::config::resolver::VaultPlaceholderResolve;
//! use nanobot_core::vault::VaultStore;
//!
//! let mut config = Config::default();
//! // ... config has api_key: "{{vault:openai_key}}"
//! let store = VaultStore::new().unwrap();
//! store.unlock("password").unwrap();
//!
//! let unresolved = config.resolve_placeholders(&store);
//! // unresolved: ["api_key_that_references a vault key 'openai_key' but was not found"]
//! ```

use regex::Regex;
use std::collections::HashMap;

use crate::config::provider::ProviderConfig;
use crate::config::schema::Config;
use crate::vault::VaultStore;

/// Environment variable name for vault password
pub const VAULT_PASSWORD_ENV: &str = "NANOBOT_VAULT_PASSWORD";

/// Trait for resolving vault placeholders in configuration structures
pub trait VaultPlaceholderResolve {
    /// Resolve all `{{vault:key}}` placeholders in this structure
    ///
    /// Returns a list of vault keys that could not be resolved
    /// (either not found in vault or vault is locked)
    fn resolve_placeholders(&mut self, store: &VaultStore) -> Vec<String>;
}

impl VaultPlaceholderResolve for String {
    fn resolve_placeholders(&mut self, store: &VaultStore) -> Vec<String> {
        let (resolved, unresolved) = resolve_string_placeholders(self, store);
        if !unresolved.is_empty() {
            *self = resolved;
        }
        unresolved
    }
}

impl VaultPlaceholderResolve for Option<String> {
    fn resolve_placeholders(&mut self, store: &VaultStore) -> Vec<String> {
        let mut unresolved = Vec::new();

        if let Some(ref s) = self {
            let (resolved, errors) = resolve_string_placeholders(s, store);
            if errors.is_empty() {
                *self = Some(resolved);
            }
            unresolved.extend(errors);
        }

        unresolved
    }
}

impl VaultPlaceholderResolve for Option<HashMap<String, String>> {
    fn resolve_placeholders(&mut self, store: &VaultStore) -> Vec<String> {
        let mut unresolved = Vec::new();

        if let Some(ref mut map) = self {
            for value in map.values_mut() {
                let (resolved, errors) = resolve_string_placeholders(value, store);
                if errors.is_empty() {
                    *value = resolved;
                }
                unresolved.extend(errors);
            }
        }

        unresolved
    }
}

impl VaultPlaceholderResolve for ProviderConfig {
    fn resolve_placeholders(&mut self, store: &VaultStore) -> Vec<String> {
        let mut unresolved = Vec::new();

        // Resolve api_key
        if let Some(ref api_key) = self.api_key {
            let (resolved, errors) = resolve_string_placeholders(api_key, store);
            if errors.is_empty() {
                self.api_key = Some(resolved);
            }
            unresolved.extend(errors);
        }

        // Resolve api_base
        if let Some(ref api_base) = self.api_base {
            let (resolved, errors) = resolve_string_placeholders(api_base, store);
            if errors.is_empty() {
                self.api_base = Some(resolved);
            }
            unresolved.extend(errors);
        }

        // Resolve client_id
        if let Some(ref client_id) = self.client_id {
            let (resolved, errors) = resolve_string_placeholders(client_id, store);
            if errors.is_empty() {
                self.client_id = Some(resolved);
            }
            unresolved.extend(errors);
        }

        unresolved
    }
}

impl VaultPlaceholderResolve for Config {
    fn resolve_placeholders(&mut self, store: &VaultStore) -> Vec<String> {
        let mut unresolved = Vec::new();

        // Resolve all providers
        for provider in self.providers.values_mut() {
            let resolved = provider.resolve_placeholders(store);
            unresolved.extend(resolved);
        }

        unresolved
    }
}

/// Resolve vault placeholders in a string
///
/// Returns a tuple of (resolved_string, unresolved_errors)
pub fn resolve_string_placeholders(s: &str, store: &VaultStore) -> (String, Vec<String>) {
    let re = Regex::new(r"\{\{vault:([a-zA-Z0-9_]+)\}\}").unwrap();
    let mut unresolved = Vec::new();
    let mut result = String::new();
    let mut last_end = 0;

    for cap in re.captures_iter(s) {
        if let Some(m) = cap.get(0) {
            // Add text before this match
            result.push_str(&s[last_end..m.start()]);

            // Extract the key from capture group 1
            if let Some(key_match) = cap.get(1) {
                let key = key_match.as_str();
                if let Some(value) = store.get(key) {
                    result.push_str(&value);
                } else {
                    // Keep the placeholder if not found
                    result.push_str(m.as_str());
                    unresolved.push(format!("Vault key '{}' not found", key));
                }
            }

            last_end = m.end();
        }
    }

    // Add remaining text after the last match
    result.push_str(&s[last_end..]);

    (result, unresolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_string_placeholders() {
        let mut store = VaultStore::new_in_memory();
        store.unlock("password").unwrap();
        store.set("api_key", "sk-test-123", None).unwrap();

        let (resolved, unresolved) = resolve_string_placeholders("key: {{vault:api_key}}", &store);
        assert!(unresolved.is_empty());
        assert_eq!(resolved, "key: sk-test-123");
    }

    #[test]
    fn test_resolve_string_multiple_placeholders() {
        let mut store = VaultStore::new_in_memory();
        store.unlock("password").unwrap();
        store.set("key1", "value1", None).unwrap();
        store.set("key2", "value2", None).unwrap();

        let (resolved, unresolved) = resolve_string_placeholders(
            "prefix {{vault:key1}} middle {{vault:key2}} suffix",
            &store,
        );
        assert!(unresolved.is_empty());
        assert_eq!(resolved, "prefix value1 middle value2 suffix");
    }

    #[test]
    fn test_resolve_string_missing_key() {
        let store = VaultStore::new_in_memory();

        let (resolved, unresolved) = resolve_string_placeholders("{{vault:nonexistent}}", &store);
        assert_eq!(unresolved.len(), 1);
        assert!(unresolved[0].contains("nonexistent"));
        // Placeholder should remain in the output
        assert_eq!(resolved, "{{vault:nonexistent}}");
    }

    #[test]
    fn test_resolve_string_no_placeholders() {
        let store = VaultStore::new_in_memory();
        let (resolved, unresolved) = resolve_string_placeholders("no placeholders here", &store);
        assert!(unresolved.is_empty());
        assert_eq!(resolved, "no placeholders here");
    }

    #[test]
    fn test_resolve_string_mixed_found_and_missing() {
        let mut store = VaultStore::new_in_memory();
        store.unlock("password").unwrap();
        store.set("found_key", "found_value", None).unwrap();

        let (resolved, unresolved) =
            resolve_string_placeholders("{{vault:found_key}} and {{vault:missing_key}}", &store);
        assert_eq!(unresolved.len(), 1);
        assert!(unresolved[0].contains("missing_key"));
        assert_eq!(resolved, "found_value and {{vault:missing_key}}");
    }

    #[test]
    fn test_resolve_config() {
        let mut store = VaultStore::new_in_memory();
        store.unlock("password").unwrap();
        store.set("openai_key", "sk-test-123", None).unwrap();
        store.set("anthropic_key", "sk-ant-456", None).unwrap();

        let mut config = Config::default();
        config.providers.insert(
            "openai".to_string(),
            crate::config::provider::ProviderConfig {
                api_key: Some("{{vault:openai_key}}".to_string()),
                ..Default::default()
            },
        );
        config.providers.insert(
            "anthropic".to_string(),
            crate::config::provider::ProviderConfig {
                api_key: Some("{{vault:anthropic_key}}".to_string()),
                ..Default::default()
            },
        );

        let unresolved = config.resolve_placeholders(&store);
        assert!(unresolved.is_empty());

        // Verify values were resolved
        assert_eq!(
            config.providers.get("openai").unwrap().api_key,
            Some("sk-test-123".to_string())
        );
        assert_eq!(
            config.providers.get("anthropic").unwrap().api_key,
            Some("sk-ant-456".to_string())
        );
    }
}
