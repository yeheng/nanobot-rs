//! YAML frontmatter parsing and serialization for memory files.
//!
//! Memory files are stored as Markdown with YAML frontmatter:
//!
//! ```markdown
//! ---
//! id: 550e8400-e29b-41d4-a716-446655440000
//! title: My Memory
//! scenario: active
//! tags:
//!   - important
//!   - reference
//! ---
//!
//! # Content
//!
//! This is the body content.
//! ```

use super::types::MemoryMeta;
use anyhow::{Context, Result};

/// Extract raw frontmatter YAML and body from markdown content.
///
/// This is a generic parser that handles:
/// - Leading/trailing whitespace
/// - Windows line endings (\r\n)
/// - Content containing `---` after frontmatter
///
/// Returns (frontmatter_yaml, body) or error if format is invalid.
/// Unlike `parse_frontmatter`, this does not deserialize the YAML.
///
/// # Errors
///
/// - Returns error if content doesn't start with `---`
/// - Returns error if no closing `---` delimiter is found
///
/// # Example
///
/// ```ignore
/// let content = r#"---
/// name: Test
/// cron: "0 9 * * *"
/// ---
///
/// Body content
/// "#;
/// let (yaml, body) = extract_frontmatter_raw(content).unwrap();
/// ```
pub fn extract_frontmatter_raw(content: &str) -> Result<(String, String)> {
    let content = content.trim_start();

    if !content.starts_with("---") {
        anyhow::bail!("Invalid markdown format: missing frontmatter start delimiter '---'");
    }

    // Find the end of frontmatter (\n--- or \r\n---)
    // Skip the first "---"
    let after_start = &content[3..];

    // Find the next "\n---" which closes frontmatter
    // We need to handle both \n and \r\n
    let end_pos = after_start.find("\n---").ok_or_else(|| {
        anyhow::anyhow!("Invalid markdown format: missing frontmatter end delimiter '---'")
    })?;

    // Extract YAML (skip initial ---, take content until closing ---)
    let yaml_str = &after_start[..end_pos];
    // Normalize line endings for YAML parsing
    let yaml_str = yaml_str.replace("\r\n", "\n").replace('\r', "\n");

    // Extract body (skip past the closing ---)
    // Position after "\n---" is end_pos + 4 (for "\n---")
    let body_start = 3 + end_pos + 4;
    let body = if body_start < content.len() {
        content[body_start..].trim().to_string()
    } else {
        String::new()
    };

    Ok((yaml_str, body))
}

/// Parse YAML frontmatter from a .md file content.
///
/// Expects content to start with `---\n`. Returns error if delimiters are missing
/// or YAML is malformed.
///
/// # Errors
///
/// - Returns error if content doesn't start with `---`
/// - Returns error if no closing `---` delimiter is found
/// - Returns error if YAML cannot be parsed into MemoryMeta
///
/// # Example
///
/// ```ignore
/// let content = r#"---
/// id: 123
/// title: Test
/// scenario: active
/// ---
/// "#;
/// let meta = parse_frontmatter(content)?;
/// ```
pub fn parse_frontmatter(content: &str) -> Result<MemoryMeta> {
    let (yaml_str, _) = extract_frontmatter_raw(content)?;
    let meta: MemoryMeta =
        serde_yaml::from_str(&yaml_str).context("Failed to parse YAML frontmatter")?;
    Ok(meta)
}

/// Extract the body content (everything after the closing `---`).
///
/// If no frontmatter delimiters are found, returns the entire content.
/// Returns empty string if there's no content after the closing delimiter.
///
/// # Example
///
/// ```ignore
/// let content = r#"---
/// title: Test
/// ---
///
/// # Body content
/// "#;
/// let body = extract_body(content);
/// assert_eq!(body, "# Body content");
/// ```
pub fn extract_body(content: &str) -> &str {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return content;
    }
    if let Some(end) = content[3..].find("\n---") {
        // Skip `---\n` + `\n---\n` = 7 chars
        let body_start = end + 7;
        if body_start < content.len() {
            return content[body_start..].trim();
        }
    }
    ""
}

/// Serialize metadata back to a full .md file with frontmatter.
///
/// Creates a properly formatted Markdown file with YAML frontmatter
/// followed by the body content.
///
/// # Example
///
/// ```ignore
/// let meta = MemoryMeta::default();
/// let body = "# Content";
/// let file = serialize_memory_file(&meta, body);
/// assert!(file.starts_with("---\n"));
/// assert!(file.contains("\n---\n\n# Content"));
/// ```
pub fn serialize_memory_file(meta: &MemoryMeta, body: &str) -> String {
    let yaml = serde_yaml::to_string(meta).unwrap_or_default();
    format!("---\n{}\n---\n\n{}", yaml.trim_end(), body.trim())
}

/// Parse a complete memory file (frontmatter + body).
///
/// Returns both the parsed metadata and the extracted body content.
/// This is a convenience wrapper around `extract_frontmatter_raw`.
///
/// # Errors
///
/// Returns error if frontmatter parsing fails (see `extract_frontmatter_raw`).
///
/// # Example
///
/// ```ignore
/// let content = r#"---
/// title: Test
/// scenario: active
/// ---
///
/// # Body
/// "#;
/// let (meta, body) = parse_memory_file(content)?;
/// assert_eq!(meta.title, "Test");
/// assert_eq!(body, "# Body");
/// ```
pub fn parse_memory_file(content: &str) -> Result<(MemoryMeta, String)> {
    let (yaml_str, body) = extract_frontmatter_raw(content)?;
    let meta: MemoryMeta =
        serde_yaml::from_str(&yaml_str).context("Failed to parse YAML frontmatter")?;
    Ok((meta, body))
}

/// Count approximate tokens (~4 chars per token for mixed content).
///
/// This is a rough estimate suitable for budget calculations. For more
/// accurate counting, use tiktoken-rs with the appropriate model.
///
/// # Example
///
/// ```ignore
/// let text = "This is a sample text with some words.";
/// let tokens = estimate_tokens(text);
/// assert!(tokens > 0);
/// ```
pub fn estimate_tokens(text: &str) -> u32 {
    (text.len() as u32) / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Frequency, Scenario};
    use uuid::Uuid;

    fn make_test_meta() -> MemoryMeta {
        MemoryMeta {
            id: Uuid::nil().to_string(),
            title: "Test Memory".to_string(),
            r#type: "note".to_string(),
            scenario: Scenario::Knowledge,
            tags: vec!["important".to_string(), "reference".to_string()],
            frequency: Frequency::Hot,
            access_count: 5,
            created: "2024-01-01T00:00:00+00:00".to_string(),
            updated: "2024-01-02T00:00:00+00:00".to_string(),
            last_accessed: "2024-01-02T00:00:00+00:00".to_string(),
            auto_expire: false,
            expires: None,
            tokens: 100,
            superseded_by: None,
            index: true,
        }
    }

    #[test]
    fn parse_valid_frontmatter() {
        let content = r#"---
id: 00000000-0000-0000-0000-000000000000
title: Test Memory
type: note
scenario: knowledge
tags:
  - important
  - reference
frequency: hot
access_count: 5
created: 2024-01-01T00:00:00+00:00
updated: 2024-01-02T00:00:00+00:00
last_accessed: 2024-01-02T00:00:00+00:00
auto_expire: false
tokens: 100
---
"#;

        let meta = parse_frontmatter(content).unwrap();

        assert_eq!(meta.id, Uuid::nil().to_string());
        assert_eq!(meta.title, "Test Memory");
        assert_eq!(meta.r#type, "note");
        assert_eq!(meta.scenario, Scenario::Knowledge);
        assert_eq!(
            meta.tags,
            vec!["important".to_string(), "reference".to_string()]
        );
        assert_eq!(meta.frequency, Frequency::Hot);
        assert_eq!(meta.access_count, 5);
        assert_eq!(meta.created, "2024-01-01T00:00:00+00:00");
        assert_eq!(meta.updated, "2024-01-02T00:00:00+00:00");
        assert_eq!(meta.last_accessed, "2024-01-02T00:00:00+00:00");
        assert_eq!(meta.auto_expire, false);
        assert_eq!(meta.expires, None);
        assert_eq!(meta.tokens, 100);
    }

    #[test]
    fn extract_body_content() {
        let content = r#"---
title: Test
scenario: active
---

# Body Content

This is the body.
"#;

        let body = extract_body(content);
        assert_eq!(body, "# Body Content\n\nThis is the body.");
    }

    #[test]
    fn extract_body_no_frontmatter() {
        let content = "# Just body content\n\nNo frontmatter here.";
        let body = extract_body(content);
        assert_eq!(body, content);
    }

    #[test]
    fn extract_body_empty_after_frontmatter() {
        let content = "---\ntitle: Test\n---\n\n";
        let body = extract_body(content);
        assert_eq!(body, "");
    }

    #[test]
    fn parse_memory_file_returns_both() {
        let content = r#"---
id: 00000000-0000-0000-0000-000000000000
title: Test Memory
scenario: knowledge
created: 2024-01-01T00:00:00+00:00
updated: 2024-01-01T00:00:00+00:00
---

# Content

This is the body.
"#;

        let (meta, body) = parse_memory_file(content).unwrap();

        assert_eq!(meta.id, Uuid::nil().to_string());
        assert_eq!(meta.title, "Test Memory");
        assert_eq!(meta.scenario, Scenario::Knowledge);
        assert_eq!(body, "# Content\n\nThis is the body.");
    }

    #[test]
    fn serialize_roundtrip() {
        let meta = make_test_meta();
        let body = "# Content\n\nThis is test content.";

        let serialized = serialize_memory_file(&meta, body);

        // Check structure
        assert!(serialized.starts_with("---\n"));
        // Check for closing delimiter followed by blank line
        assert!(serialized.contains("---\n\n"));

        // Parse it back
        let (parsed_meta, parsed_body) = parse_memory_file(&serialized).unwrap();

        // Verify metadata matches
        assert_eq!(parsed_meta.id, meta.id);
        assert_eq!(parsed_meta.title, meta.title);
        assert_eq!(parsed_meta.scenario, meta.scenario);
        assert_eq!(parsed_meta.tags, meta.tags);
        assert_eq!(parsed_meta.frequency, meta.frequency);

        // Verify body matches (trimmed)
        assert_eq!(parsed_body, body.trim());
    }

    #[test]
    fn parse_missing_delimiter_fails() {
        let content = "Just plain text without any delimiters.";
        let result = parse_frontmatter(content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing frontmatter start delimiter"));
    }

    #[test]
    fn parse_missing_closing_delimiter_fails() {
        let content = "---\ntitle: Test\nscenario: active\n";
        let result = parse_frontmatter(content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing frontmatter end delimiter"));
    }

    #[test]
    fn parse_invalid_yaml_fails() {
        let content = "---\ninvalid: yaml: content:\n---\n";
        let result = parse_frontmatter(content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Failed to parse YAML frontmatter"));
    }

    #[test]
    fn estimate_tokens_reasonable() {
        let text = "This is a sample text with some words for testing token estimation.";
        let tokens = estimate_tokens(text);

        // Should be roughly character count / 4
        let expected = (text.len() / 4) as u32;
        assert_eq!(tokens, expected);

        // Should be non-zero for non-empty text
        assert!(tokens > 0);

        // Empty text should return 0
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn serialize_with_empty_body() {
        let meta = make_test_meta();
        let serialized = serialize_memory_file(&meta, "");

        assert!(serialized.starts_with("---\n"));
        // Should end with closing delimiter and blank line (even with empty body)
        assert!(serialized.ends_with("---\n\n"));
    }

    #[test]
    fn serialize_preserves_all_fields() {
        let meta = make_test_meta();
        let serialized = serialize_memory_file(&meta, "body");

        // Verify key fields are in the serialized output
        assert!(serialized.contains(&format!("id: {}", meta.id)));
        assert!(serialized.contains(&format!("title: {}", meta.title)));
        assert!(serialized.contains(&format!("scenario: {}", meta.scenario)));
        assert!(serialized.contains("important"));
        assert!(serialized.contains("reference"));
        assert!(serialized.contains("frequency: hot"));
        assert!(serialized.contains("access_count: 5"));
    }

    #[test]
    fn parse_with_optional_fields_missing() {
        let content = r#"---
id: 00000000-0000-0000-0000-000000000000
title: Test
scenario: active
created: 2024-01-01T00:00:00+00:00
updated: 2024-01-01T00:00:00+00:00
---"#;

        let meta = parse_frontmatter(content).unwrap();

        // Required fields
        assert_eq!(meta.title, "Test");
        assert_eq!(meta.scenario, Scenario::Active);

        // Optional fields should have defaults
        assert_eq!(meta.tags, Vec::<String>::new());
        assert_eq!(meta.frequency, Frequency::Archived); // default
        assert_eq!(meta.access_count, 0);
        assert_eq!(meta.auto_expire, false);
        assert_eq!(meta.tokens, 0);
    }
}
