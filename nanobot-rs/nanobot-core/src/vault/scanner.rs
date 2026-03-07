//! Placeholder scanner for detecting {{vault:key}} patterns

use regex::Regex;
use std::collections::HashSet;

/// Placeholder pattern: {{vault:key_name}}
pub const PLACEHOLDER_PATTERN: &str = r"\{\{vault:([a-zA-Z0-9_]+)\}\}";

/// A scanned placeholder
#[derive(Debug, Clone)]
pub struct Placeholder {
    /// The extracted key
    pub key: String,
    /// The full match (e.g., "{{vault:key}}")
    pub full_match: String,
    /// Start position in the text
    pub start: usize,
    /// End position in the text
    pub end: usize,
}

/// Scan text for vault placeholders
///
/// Returns a list of all placeholders found in the text.
pub fn scan_placeholders(text: &str) -> Vec<Placeholder> {
    let re = Regex::new(PLACEHOLDER_PATTERN).unwrap();

    re.captures_iter(text)
        .filter_map(|cap| {
            let full = cap.get(0)?;
            let key = cap.get(1)?;
            Some(Placeholder {
                key: key.as_str().to_string(),
                full_match: full.as_str().to_string(),
                start: full.start(),
                end: full.end(),
            })
        })
        .collect()
}

/// Extract all unique keys from text
pub fn extract_keys(text: &str) -> HashSet<String> {
    scan_placeholders(text)
        .into_iter()
        .map(|p| p.key)
        .collect()
}

/// Replace placeholders in text with their values
///
/// Placeholders with missing keys are left unchanged.
pub fn replace_placeholders(text: &str, replacements: &std::collections::HashMap<String, String>) -> String {
    let re = Regex::new(PLACEHOLDER_PATTERN).unwrap();

    re.replace_all(text, |cap: &regex::Captures| -> String {
        let key = &cap[1];
        replacements.get(key)
            .cloned()
            .unwrap_or_else(|| cap.get(0).unwrap().as_str().to_string())
    }).to_string()
}

/// Check if text contains any placeholders
pub fn contains_placeholders(text: &str) -> bool {
    let re = Regex::new(PLACEHOLDER_PATTERN).unwrap();
    re.is_match(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_single_placeholder() {
        let text = "使用 {{vault:db_password}} 连接数据库";
        let result = scan_placeholders(text);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].key, "db_password");
        assert_eq!(result[0].full_match, "{{vault:db_password}}");
    }

    #[test]
    fn test_scan_multiple_placeholders() {
        let text = "用 {{vault:key1}} 和 {{vault:key2}} 测试";
        let result = scan_placeholders(text);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].key, "key1");
        assert_eq!(result[1].key, "key2");
    }

    #[test]
    fn test_scan_no_placeholders() {
        let text = "没有placeholder的普通文本";
        let result = scan_placeholders(text);
        assert!(result.is_empty());
    }

    #[test]
    fn test_scan_adjacent_placeholders() {
        let text = "{{vault:a}}{{vault:b}}";
        let result = scan_placeholders(text);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].end, result[1].start);
    }

    #[test]
    fn test_extract_keys_unique() {
        let text = "{{vault:key1}} {{vault:key2}} {{vault:key1}}";
        let keys = extract_keys(text);
        assert_eq!(keys.len(), 2);
        assert!(keys.contains("key1"));
        assert!(keys.contains("key2"));
    }

    #[test]
    fn test_replace_single() {
        let mut replacements = std::collections::HashMap::new();
        replacements.insert("key".to_string(), "value".to_string());

        let text = "使用 {{vault:key}} 测试";
        let result = replace_placeholders(text, &replacements);
        assert_eq!(result, "使用 value 测试");
    }

    #[test]
    fn test_replace_multiple() {
        let mut replacements = std::collections::HashMap::new();
        replacements.insert("k1".to_string(), "v1".to_string());
        replacements.insert("k2".to_string(), "v2".to_string());

        let text = "{{vault:k1}} and {{vault:k2}}";
        let result = replace_placeholders(text, &replacements);
        assert_eq!(result, "v1 and v2");
    }

    #[test]
    fn test_replace_missing_key() {
        let replacements = std::collections::HashMap::new();
        let text = "使用 {{vault:missing}} 测试";
        let result = replace_placeholders(text, &replacements);
        assert_eq!(result, text); // Unchanged
    }

    #[test]
    fn test_replace_partial() {
        let mut replacements = std::collections::HashMap::new();
        replacements.insert("exists".to_string(), "value".to_string());

        let text = "{{vault:exists}} and {{vault:missing}}";
        let result = replace_placeholders(text, &replacements);
        assert_eq!(result, "value and {{vault:missing}}");
    }

    #[test]
    fn test_contains_placeholders() {
        assert!(contains_placeholders("{{vault:key}}"));
        assert!(!contains_placeholders("no placeholder"));
    }

    #[test]
    fn test_invalid_format_not_matched() {
        let text = "{{vault:}} {{vault:key-with-dash}} {{Vault:key}}";
        let result = scan_placeholders(text);
        // Empty key and dash should not match, case sensitive
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_valid_key_formats() {
        let text = "{{vault:api_key}} {{vault:API_KEY}} {{vault:key123}} {{vault:Key_123}}";
        let result = scan_placeholders(text);
        assert_eq!(result.len(), 4);
    }
}
