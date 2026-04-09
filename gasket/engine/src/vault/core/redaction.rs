//! Log redaction utilities for sensitive data

/// Minimum length for a secret to be considered for redaction.
/// Secrets shorter than this threshold are skipped to avoid false positives
/// that would make logs unreadable (e.g., "id" or "a" matching everywhere).
const MIN_SECRET_LENGTH: usize = 4;

/// Replace sensitive values with [REDACTED]
///
/// This function is used to prevent sensitive data from appearing in logs.
///
/// # Security vs Usability Trade-off
///
/// We skip secrets shorter than MIN_SECRET_LENGTH (4 chars) because:
/// - A 1-3 character password like "a" or "id" would match in almost every log line
/// - This would render logs completely unreadable in production
/// - Short passwords are also weak passwords - users should use longer ones anyway
///
/// If you need to redact short secrets, consider using a different approach
/// like context-aware redaction or prefix/suffix matching.
///
/// # Substring Handling
///
/// Secrets are sorted by length in **descending order** before replacement.
/// This prevents short substrings from corrupting longer secrets.
/// Example: with ["pass", "password"], we replace "password" first,
/// avoiding the bug where "password" becomes "[REDACTED]word".
pub fn redact_secrets(text: &str, secrets: &[String]) -> String {
    let mut result = text.to_string();

    // Sort by length descending to prevent substring corruption
    // e.g., replace "password" before "pass" to avoid "[REDACTED]word"
    let mut sorted_secrets = secrets.to_vec();
    sorted_secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));

    for secret in sorted_secrets {
        // Skip empty or too-short secrets to prevent over-redaction
        // See MIN_SECRET_LENGTH comment for rationale
        if secret.len() < MIN_SECRET_LENGTH {
            continue;
        }
        if result.contains(&secret) {
            result = result.replace(&secret, "[REDACTED]");
        }
    }
    result
}

/// Check if text contains any sensitive values
pub fn contains_secrets(text: &str, secrets: &[String]) -> bool {
    secrets
        .iter()
        .any(|s| s.len() >= MIN_SECRET_LENGTH && text.contains(s))
}

/// Redact secrets in a ChatMessage content
pub fn redact_message_secrets(content: &str, secrets: &[String]) -> String {
    redact_secrets(content, secrets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_single() {
        let text = "密码是 secret123 请保密";
        let secrets = vec!["secret123".to_string()];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, "密码是 [REDACTED] 请保密");
    }

    #[test]
    fn test_redact_multiple() {
        let text = "key=sk-12345 pass=secret123";
        let secrets = vec!["sk-12345".to_string(), "secret123".to_string()];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, "key=[REDACTED] pass=[REDACTED]");
    }

    #[test]
    fn test_redact_no_secrets() {
        let text = "没有敏感数据";
        let secrets: Vec<String> = vec![];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, text);
    }

    #[test]
    fn test_redact_empty_secret() {
        let text = "文本内容";
        let secrets = vec!["".to_string()];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, text); // Empty secret should not redact
    }

    #[test]
    fn test_redact_partial_match() {
        let text = "sk-12345 is a key";
        let secrets = vec!["sk-12345".to_string()];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, "[REDACTED] is a key");
    }

    #[test]
    fn test_redact_multiple_occurrences() {
        let text = "key: secret, again: secret";
        let secrets = vec!["secret".to_string()];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, "key: [REDACTED], again: [REDACTED]");
    }

    #[test]
    fn test_contains_secrets_true() {
        let text = "密码是 secret123";
        let secrets = vec!["secret123".to_string()];
        assert!(contains_secrets(text, &secrets));
    }

    #[test]
    fn test_contains_secrets_false() {
        let text = "普通文本";
        let secrets = vec!["secret123".to_string()];
        assert!(!contains_secrets(text, &secrets));
    }

    #[test]
    fn test_redact_json_like() {
        let text = r#"{"api_key": "sk-12345", "other": "value"}"#;
        let secrets = vec!["sk-12345".to_string()];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, r#"{"api_key": "[REDACTED]", "other": "value"}"#);
    }

    #[test]
    fn test_redact_url_with_password() {
        let text = "postgresql://user:p@ss@localhost/db";
        let secrets = vec!["p@ss".to_string()];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, "postgresql://user:[REDACTED]@localhost/db");
    }

    #[test]
    fn test_redact_multiline() {
        let text = "Line 1\nsecret123\nLine 3";
        let secrets = vec!["secret123".to_string()];
        let result = redact_secrets(text, &secrets);
        assert_eq!(result, "Line 1\n[REDACTED]\nLine 3");
    }

    // ── MIN_SECRET_LENGTH tests ─────────────────────────────────────────────

    #[test]
    fn test_short_secret_not_redacted() {
        // Secrets shorter than MIN_SECRET_LENGTH (4) should NOT be redacted
        // to avoid making logs unreadable with false positives
        let text = "My id is 12345";
        let secrets = vec!["id".to_string()];
        let result = redact_secrets(text, &secrets);
        // "id" is too short, so it should NOT be redacted
        assert_eq!(result, "My id is 12345");
    }

    #[test]
    fn test_single_char_not_redacted() {
        let text = "a b c d e f g";
        let secrets = vec!["a".to_string(), "b".to_string()];
        let result = redact_secrets(text, &secrets);
        // Single chars should NOT be redacted
        assert_eq!(result, "a b c d e f g");
    }

    #[test]
    fn test_three_char_not_redacted() {
        let text = "The key is abc";
        let secrets = vec!["abc".to_string()];
        let result = redact_secrets(text, &secrets);
        // 3 chars is still too short
        assert_eq!(result, "The key is abc");
    }

    #[test]
    fn test_four_char_redacted() {
        let text = "The key is abcd";
        let secrets = vec!["abcd".to_string()];
        let result = redact_secrets(text, &secrets);
        // 4 chars meets the minimum threshold
        assert_eq!(result, "The key is [REDACTED]");
    }

    #[test]
    fn test_mixed_short_and_long_secrets() {
        // Test with both short (ignored) and long (redacted) secrets
        let text = "My id is super_secret_key";
        let secrets = vec!["id".to_string(), "super_secret_key".to_string()];
        let result = redact_secrets(text, &secrets);
        // "id" should NOT be redacted, "super_secret_key" should be
        assert_eq!(result, "My id is [REDACTED]");
    }

    #[test]
    fn test_contains_secrets_ignores_short() {
        let text = "My id is here";
        let secrets = vec!["id".to_string()];
        // contains_secrets should also respect the minimum length
        assert!(!contains_secrets(text, &secrets));
    }

    #[test]
    fn test_contains_secrets_with_long_secret() {
        let text = "The secret is mypassword";
        let secrets = vec!["mypassword".to_string()];
        assert!(contains_secrets(text, &secrets));
    }

    // ── Substring coverage tests (Task 2) ─────────────────────────────────────

    #[test]
    fn test_redact_substring_ordering() {
        // Critical test: ensure longer secrets are replaced first
        // to prevent substring corruption
        let text = "My pass is password123";
        let secrets = vec!["pass".to_string(), "password123".to_string()];
        let result = redact_secrets(text, &secrets);
        // Should be: "My [REDACTED] is [REDACTED]"
        // NOT: "My [REDACTED] is [REDACTED]word123"
        assert_eq!(result, "My [REDACTED] is [REDACTED]");
    }

    #[test]
    fn test_redact_overlapping_secrets() {
        // Another test case with overlapping secrets
        let text = "The api_key and api_key_secret are set";
        let secrets = vec!["api_key".to_string(), "api_key_secret".to_string()];
        let result = redact_secrets(text, &secrets);
        // Both should be redacted without corruption
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("api_key"));
    }

    #[test]
    fn test_redact_multiple_substrings() {
        // Test with multiple levels of substring overlap
        let text = "Values: a, ab, abc, abcd";
        let secrets = vec![
            "a".to_string(),
            "ab".to_string(),
            "abc".to_string(),
            "abcd".to_string(),
        ];
        let result = redact_secrets(text, &secrets);
        // "a" (1 char) is too short, so it won't be redacted
        // "ab" (2 chars) is too short
        // "abc" (3 chars) is too short
        // Only "abcd" (4 chars) meets MIN_SECRET_LENGTH
        assert_eq!(result, "Values: a, ab, abc, [REDACTED]");
    }

    #[test]
    fn test_redact_unsorted_input() {
        // Input secrets in random order should still work correctly
        let text = "password and pass are both secrets";
        // "pass" (4 chars) and "password" (8 chars) both meet MIN_SECRET_LENGTH
        let secrets = vec!["pass".to_string(), "password".to_string()];
        let result = redact_secrets(text, &secrets);
        // "password" should be replaced first, then "pass" if it still exists
        assert_eq!(result, "[REDACTED] and [REDACTED] are both secrets");
    }
}
