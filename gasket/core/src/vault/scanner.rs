//! Placeholder scanner for detecting {{vault:key}} patterns
//!
//! Uses a lightweight byte-slice cursor instead of regex — the pattern
//! has a fixed prefix (`{{vault:`) and suffix (`}}`), making a full regex
//! engine unnecessary.

use std::collections::HashSet;

/// Fixed prefix for vault placeholders
const PREFIX: &str = "{{vault:";
/// Fixed suffix for vault placeholders
const SUFFIX: &str = "}}";

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
    let mut results = Vec::new();
    let mut cursor = 0;
    let bytes = text.as_bytes();

    while cursor < bytes.len() {
        // Jump to next potential match
        let remaining = &text[cursor..];
        let Some(offset) = remaining.find(PREFIX) else {
            break;
        };
        let start = cursor + offset;
        let key_start = start + PREFIX.len();

        // Scan key characters: [a-zA-Z0-9_]+
        let mut key_end = key_start;
        while key_end < bytes.len() {
            let b = bytes[key_end];
            if b.is_ascii_alphanumeric() || b == b'_' {
                key_end += 1;
            } else {
                break;
            }
        }

        // Key must be non-empty and followed by "}}"
        if key_end > key_start && text[key_end..].starts_with(SUFFIX) {
            let end = key_end + SUFFIX.len();
            results.push(Placeholder {
                key: text[key_start..key_end].to_string(),
                full_match: text[start..end].to_string(),
                start,
                end,
            });
            cursor = end;
        } else {
            // Not a valid match — advance past the prefix to avoid infinite loop
            cursor = key_start;
        }
    }

    results
}

/// Extract all unique keys from text
pub fn extract_keys(text: &str) -> HashSet<String> {
    scan_placeholders(text).into_iter().map(|p| p.key).collect()
}

/// Replace placeholders in text with their values
///
/// Placeholders with missing keys are left unchanged.
pub fn replace_placeholders(
    text: &str,
    replacements: &std::collections::HashMap<String, String>,
) -> String {
    let placeholders = scan_placeholders(text);
    if placeholders.is_empty() {
        return text.to_string();
    }

    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    for p in &placeholders {
        // Copy text before this placeholder
        result.push_str(&text[last_end..p.start]);
        // Replace or keep original
        if let Some(value) = replacements.get(&p.key) {
            result.push_str(value);
        } else {
            result.push_str(&p.full_match);
        }
        last_end = p.end;
    }
    // Copy remaining text
    result.push_str(&text[last_end..]);

    result
}

/// Check if text contains any placeholders
pub fn contains_placeholders(text: &str) -> bool {
    let mut cursor = 0;
    let bytes = text.as_bytes();

    while cursor < bytes.len() {
        let remaining = &text[cursor..];
        let Some(offset) = remaining.find(PREFIX) else {
            return false;
        };
        let key_start = cursor + offset + PREFIX.len();

        // Check for at least one valid key character
        let mut key_end = key_start;
        while key_end < bytes.len() {
            let b = bytes[key_end];
            if b.is_ascii_alphanumeric() || b == b'_' {
                key_end += 1;
            } else {
                break;
            }
        }

        if key_end > key_start && text[key_end..].starts_with(SUFFIX) {
            return true;
        }
        cursor = key_start;
    }

    false
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
