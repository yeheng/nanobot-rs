//! Log redaction utilities for sensitive data

/// Replace sensitive values with [REDACTED]
///
/// This function is used to prevent sensitive data from appearing in logs.
pub fn redact_secrets(text: &str, secrets: &[String]) -> String {
    let mut result = text.to_string();
    for secret in secrets {
        if !secret.is_empty() && text.contains(secret) {
            result = result.replace(secret, "[REDACTED]");
        }
    }
    result
}

/// Check if text contains any sensitive values
pub fn contains_secrets(text: &str, secrets: &[String]) -> bool {
    secrets.iter().any(|s| !s.is_empty() && text.contains(s))
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
}
