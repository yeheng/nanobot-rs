//! VaultInjector - Runtime injection of sensitive data
//!
//! Injects secrets at the last moment before sending to LLM.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tracing::{debug, warn};

use super::{replace_placeholders, scan_placeholders, VaultStore};
use crate::providers::ChatMessage;

/// Report of an injection operation
#[derive(Debug, Default)]
pub struct InjectionReport {
    /// Number of messages that were modified
    pub messages_modified: usize,
    /// Keys that were successfully injected
    pub keys_used: Vec<String>,
    /// Keys that were not found in the vault
    pub missing_keys: Vec<String>,
    /// All injected values (for log redaction)
    pub injected_values: Vec<String>,
}

/// Runtime injector for vault placeholders
///
/// Replaces {{vault:key}} placeholders with actual values
/// at the last moment before sending to LLM.
///
/// Uses `Arc<VaultStore>` instead of `Arc<Mutex<VaultStore>>` because
/// `VaultStore::get()` is now a read-only operation (doesn't update last_used).
/// This eliminates lock contention in high-concurrency scenarios.
pub struct VaultInjector {
    store: Arc<VaultStore>,
}

impl VaultInjector {
    /// Create a new injector with the given vault store
    pub fn new(store: Arc<VaultStore>) -> Self {
        Self { store }
    }

    /// Inject all placeholders in the messages
    ///
    /// Returns a report of what was injected.
    pub fn inject(&self, messages: &mut [ChatMessage]) -> InjectionReport {
        let mut report = InjectionReport::default();
        let mut all_keys = HashSet::new();

        // 1. Collect all keys needed
        for msg in messages.iter() {
            if let Some(ref content) = msg.content {
                for placeholder in scan_placeholders(content) {
                    all_keys.insert(placeholder.key);
                }
            }
        }

        if all_keys.is_empty() {
            return report;
        }

        debug!(
            "[VaultInjector] Found {} unique keys to inject",
            all_keys.len()
        );

        // Check if vault is locked upfront to provide better error message
        let vault_locked = self.store.is_locked();

        // 2. Build replacement map (no lock needed - get() is &self)
        let mut replacements = HashMap::new();
        for key in &all_keys {
            if let Some(value) = self.store.get(key) {
                replacements.insert(key.clone(), value.clone());
                report.keys_used.push(key.clone());
                report.injected_values.push(value);
            } else {
                report.missing_keys.push(key.clone());
                // Provide more specific error messages based on failure reason
                if vault_locked {
                    warn!(
                        "[VaultInjector] Cannot inject key '{}': vault is locked. Run 'nanobot vault unlock' first.",
                        key
                    );
                } else if self.store.exists(key) {
                    // Key exists but get() failed - decryption error already logged by VaultStore::get
                    warn!(
                        "[VaultInjector] Failed to decrypt key '{}'. Check if vault password is correct.",
                        key
                    );
                } else {
                    warn!("[VaultInjector] Key not found in vault: {}", key);
                }
            }
        }

        // 3. Perform replacements
        for msg in messages.iter_mut() {
            if let Some(ref content) = msg.content {
                let replaced = replace_placeholders(content, &replacements);
                if replaced != *content {
                    report.messages_modified += 1;
                    msg.content = Some(replaced);
                }
            }
        }

        debug!(
            "[VaultInjector] Injected {} values into {} messages",
            report.keys_used.len(),
            report.messages_modified
        );

        report
    }

    /// Scan messages for placeholders without injecting
    ///
    /// Returns the set of all keys found.
    pub fn scan(&self, messages: &[ChatMessage]) -> HashSet<String> {
        let mut keys = HashSet::new();
        for msg in messages {
            if let Some(ref content) = msg.content {
                for placeholder in scan_placeholders(content) {
                    keys.insert(placeholder.key);
                }
            }
        }
        keys
    }

    /// Check if any messages contain placeholders
    pub fn has_placeholders(&self, messages: &[ChatMessage]) -> bool {
        messages.iter().any(|msg| {
            if let Some(ref content) = msg.content {
                !scan_placeholders(content).is_empty()
            } else {
                false
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_store() -> Arc<VaultStore> {
        let mut store = VaultStore::new_in_memory();
        store.unlock("test-password").unwrap();
        store
            .set("api_key", "sk-12345", Some("Test API key"))
            .unwrap();
        store.set("password", "secret123", None).unwrap();
        store
            .set(
                "db_conn",
                "postgresql://user:pass@localhost/db",
                Some("Database connection"),
            )
            .unwrap();
        Arc::new(store)
    }

    #[test]
    fn test_inject_single_key() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let mut messages = vec![ChatMessage::user("使用 {{vault:api_key}} 调用API")];

        let report = injector.inject(&mut messages);

        assert_eq!(report.messages_modified, 1);
        assert_eq!(report.keys_used, vec!["api_key"]);
        assert!(report.missing_keys.is_empty());
        assert_eq!(
            messages[0].content,
            Some("使用 sk-12345 调用API".to_string())
        );
    }

    #[test]
    fn test_inject_multiple_keys() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let mut messages = vec![ChatMessage::user(
            "用 {{vault:api_key}} 和 {{vault:password}}",
        )];

        let report = injector.inject(&mut messages);

        assert_eq!(report.keys_used.len(), 2);
        assert!(messages[0].content.as_ref().unwrap().contains("sk-12345"));
        assert!(messages[0].content.as_ref().unwrap().contains("secret123"));
    }

    #[test]
    fn test_inject_multiple_messages() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let mut messages = vec![
            ChatMessage::user("使用 {{vault:api_key}}"),
            ChatMessage::assistant("好的"),
            ChatMessage::user("再用 {{vault:password}}"),
        ];

        let report = injector.inject(&mut messages);

        assert_eq!(report.messages_modified, 2);
        assert_eq!(messages[0].content, Some("使用 sk-12345".to_string()));
        assert_eq!(messages[1].content, Some("好的".to_string())); // Unchanged
        assert_eq!(messages[2].content, Some("再用 secret123".to_string()));
    }

    #[test]
    fn test_inject_missing_key() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let mut messages = vec![ChatMessage::user("使用 {{vault:unknown_key}}")];

        let report = injector.inject(&mut messages);

        assert_eq!(report.missing_keys, vec!["unknown_key"]);
        // Unknown key should remain unchanged
        assert_eq!(
            messages[0].content,
            Some("使用 {{vault:unknown_key}}".to_string())
        );
    }

    #[test]
    fn test_inject_partial() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let mut messages = vec![ChatMessage::user("{{vault:api_key}} 和 {{vault:missing}}")];

        let report = injector.inject(&mut messages);

        assert_eq!(report.keys_used, vec!["api_key"]);
        assert_eq!(report.missing_keys, vec!["missing"]);
        assert_eq!(
            messages[0].content,
            Some("sk-12345 和 {{vault:missing}}".to_string())
        );
    }

    #[test]
    fn test_inject_no_placeholders() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let mut messages = vec![ChatMessage::user("没有placeholder")];

        let report = injector.inject(&mut messages);

        assert_eq!(report.messages_modified, 0);
        assert!(report.keys_used.is_empty());
        assert_eq!(messages[0].content, Some("没有placeholder".to_string()));
    }

    #[test]
    fn test_scan_messages() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let messages = vec![
            ChatMessage::user("{{vault:key1}} 和 {{vault:key2}}"),
            ChatMessage::assistant("回复"),
            ChatMessage::user("{{vault:key1}} 再次"),
        ];

        let keys = injector.scan(&messages);

        assert_eq!(keys.len(), 2);
        assert!(keys.contains("key1"));
        assert!(keys.contains("key2"));
    }

    #[test]
    fn test_has_placeholders() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let with_placeholders = vec![ChatMessage::user("{{vault:key}}")];
        let without_placeholders = vec![ChatMessage::user("普通文本")];

        assert!(injector.has_placeholders(&with_placeholders));
        assert!(!injector.has_placeholders(&without_placeholders));
    }

    #[test]
    fn test_injected_values_for_redaction() {
        let store = create_test_store();
        let injector = VaultInjector::new(store);

        let mut messages = vec![ChatMessage::user("使用 {{vault:api_key}}")];

        let report = injector.inject(&mut messages);

        assert_eq!(report.injected_values, vec!["sk-12345"]);
    }

    #[test]
    fn test_complex_value_with_special_chars() {
        let mut store = VaultStore::new_in_memory();
        store.unlock("password").unwrap();
        store
            .set(
                "conn",
                "postgresql://user:p@ss!word@localhost:5432/db",
                None,
            )
            .unwrap();
        let injector = VaultInjector::new(Arc::new(store));

        let mut messages = vec![ChatMessage::user("连接: {{vault:conn}}")];

        let report = injector.inject(&mut messages);

        assert_eq!(report.messages_modified, 1);
        assert!(messages[0].content.as_ref().unwrap().contains("p@ss!word"));
    }

    #[test]
    fn test_inject_empty_messages() {
        let mut store = VaultStore::new_in_memory();
        store.unlock("password").unwrap();
        let injector = VaultInjector::new(Arc::new(store));

        let mut messages: Vec<ChatMessage> = vec![];
        let report = injector.inject(&mut messages);

        assert_eq!(report.messages_modified, 0);
        assert!(report.keys_used.is_empty());
        assert!(report.missing_keys.is_empty());
    }

    #[test]
    fn test_inject_message_with_none_content() {
        let mut store = VaultStore::new_in_memory();
        store.unlock("password").unwrap();
        store.set("key", "value", None).unwrap();
        let injector = VaultInjector::new(Arc::new(store));

        // Create a message with None content
        let mut messages = vec![ChatMessage {
            role: crate::providers::MessageRole::User,
            content: None,
            name: None,
            tool_call_id: None,
            tool_calls: None,
        }];

        let report = injector.inject(&mut messages);

        assert_eq!(report.messages_modified, 0);
    }

    #[test]
    fn test_inject_same_key_multiple_times() {
        let mut store = VaultStore::new_in_memory();
        store.unlock("password").unwrap();
        store.set("key", "value", None).unwrap();
        let injector = VaultInjector::new(Arc::new(store));

        let mut messages = vec![ChatMessage::user(
            "{{vault:key}} {{vault:key}} {{vault:key}}",
        )];

        let report = injector.inject(&mut messages);

        assert_eq!(report.keys_used.len(), 1);
        assert_eq!(report.keys_used[0], "key");
        assert_eq!(messages[0].content, Some("value value value".to_string()));
    }

    #[test]
    fn test_inject_unicode_placeholder() {
        let mut store = VaultStore::new_in_memory();
        store.unlock("password").unwrap();
        store.set("api_key", "sk-12345", None).unwrap();
        let injector = VaultInjector::new(Arc::new(store));

        let mut messages = vec![ChatMessage::user("使用 {{vault:api_key}} 进行认证 🔐")];

        let report = injector.inject(&mut messages);

        assert_eq!(report.messages_modified, 1);
        assert!(messages[0].content.as_ref().unwrap().contains("sk-12345"));
        assert!(messages[0].content.as_ref().unwrap().contains("🔐"));
    }

    /// Test sequential access to VaultInjector (Task 1 validation)
    /// Verifies that injection can happen without lock contention.
    /// Note: VaultStore doesn't implement Sync (due to VaultCrypto's ZeroizeOnDrop),
    /// so we test sequential injection speed instead of true concurrency.
    #[test]
    fn test_sequential_injection_no_lock() {
        let mut store = VaultStore::new_in_memory();
        store.unlock("password").unwrap();
        store.set("api_key", "sk-concurrent-test", None).unwrap();
        let store = Arc::new(store);
        let injector = VaultInjector::new(store);

        // Test that sequential injections work without any lock-related issues
        for i in 0..10 {
            let mut messages = vec![ChatMessage::user(&format!(
                "Request {}: {{{{vault:api_key}}}}",
                i
            ))];
            let report = injector.inject(&mut messages);
            assert_eq!(report.messages_modified, 1);
            assert!(messages[0]
                .content
                .as_ref()
                .unwrap()
                .contains("sk-concurrent-test"));
        }
    }
}
