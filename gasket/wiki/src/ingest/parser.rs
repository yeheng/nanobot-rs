use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Supported source formats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceFormat {
    Markdown,
    Html,
    PlainText,
    Conversation,
}

/// Metadata extracted from a source document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceMetadata {
    /// Original filename or identifier.
    pub source_path: String,
    /// Format of the source.
    pub format: SourceFormat,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Title extracted from content (if available).
    pub title: Option<String>,
    /// Arbitrary key-value metadata from frontmatter/headers.
    pub extra: HashMap<String, String>,
}

/// Result of parsing a source document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedSource {
    /// Title of the source (from frontmatter, first heading, or filename).
    pub title: String,
    /// Full text content, cleaned and normalized.
    pub content: String,
    /// Metadata about the source.
    pub metadata: SourceMetadata,
}

/// Parser for a specific source format.
#[async_trait]
pub trait SourceParser: Send + Sync {
    /// The format this parser handles.
    fn format(&self) -> SourceFormat;

    /// Parse a source at the given path.
    async fn parse(&self, path: &Path) -> Result<ParsedSource>;

    /// Parse raw content string directly (no file I/O).
    fn parse_content(&self, content: &str, title: &str) -> Result<ParsedSource>;
}

/// Extract a title from a file path (stem of the filename).
pub fn title_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string()
}

/// Markdown parser with YAML frontmatter support.
#[derive(Debug, Clone)]
pub struct MarkdownParser;

impl MarkdownParser {
    pub fn new() -> Self {
        Self
    }

    /// Extract YAML frontmatter and body from markdown content.
    fn split_frontmatter(content: &str) -> Result<(Option<&str>, &str)> {
        let content = content.trim_start();
        if !content.starts_with("---") {
            return Ok((None, content));
        }

        let rest = &content[3..];
        let end = rest
            .find("\n---")
            .ok_or_else(|| anyhow::anyhow!("unclosed frontmatter delimiter"))?;
        let yaml = &rest[..end];
        let body = rest[end + 4..].trim_start_matches('\n');

        Ok((Some(yaml), body))
    }

    /// Extract title from frontmatter YAML.
    fn extract_title_from_frontmatter(yaml: &str) -> Option<String> {
        let yaml = yaml.trim();
        for line in yaml.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("title:") {
                let title = rest.trim().trim_start_matches('"').trim_start_matches('\'');
                let title = title.trim_end_matches('"').trim_end_matches('\'');
                return Some(title.to_string());
            }
        }
        None
    }

    /// Extract all metadata from frontmatter YAML.
    fn extract_frontmatter_metadata(yaml: &str) -> HashMap<String, String> {
        let mut meta = HashMap::new();
        for line in yaml.lines() {
            let line = line.trim();
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value
                    .trim()
                    .trim_start_matches('"')
                    .trim_start_matches('\'')
                    .trim_end_matches('"')
                    .trim_end_matches('\'')
                    .to_string();
                meta.insert(key, value);
            }
        }
        meta
    }

    /// Extract title from first heading in markdown body.
    fn extract_first_heading(body: &str) -> Option<String> {
        for line in body.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix('#') {
                let title = rest.trim();
                if !title.is_empty() {
                    return Some(title.to_string());
                }
            }
        }
        None
    }
}

impl Default for MarkdownParser {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SourceParser for MarkdownParser {
    fn format(&self) -> SourceFormat {
        SourceFormat::Markdown
    }

    async fn parse(&self, path: &Path) -> Result<ParsedSource> {
        let content = tokio::fs::read_to_string(path).await?;
        let metadata = tokio::fs::metadata(path).await?;
        let size_bytes = metadata.len();
        let source_path = path.to_string_lossy().to_string();

        let (frontmatter, body) = Self::split_frontmatter(&content)?;

        let title_from_fm = frontmatter.and_then(Self::extract_title_from_frontmatter);
        let title_from_heading = Self::extract_first_heading(body);
        let title_from_file = title_from_path(path);

        let title = title_from_fm
            .or(title_from_heading)
            .unwrap_or(title_from_file);

        let extra = frontmatter
            .map(Self::extract_frontmatter_metadata)
            .unwrap_or_default();

        let metadata = SourceMetadata {
            source_path,
            format: SourceFormat::Markdown,
            size_bytes,
            title: Some(title.clone()),
            extra,
        };

        Ok(ParsedSource {
            title,
            content: body.to_string(),
            metadata,
        })
    }

    fn parse_content(&self, content: &str, title: &str) -> Result<ParsedSource> {
        let (frontmatter, body) = Self::split_frontmatter(content)?;

        let title_from_fm = frontmatter.and_then(Self::extract_title_from_frontmatter);
        let title_from_heading = Self::extract_first_heading(body);

        let title = title_from_fm
            .or(title_from_heading)
            .unwrap_or_else(|| title.to_string());

        let extra = frontmatter
            .map(Self::extract_frontmatter_metadata)
            .unwrap_or_default();

        let metadata = SourceMetadata {
            source_path: "<string>".to_string(),
            format: SourceFormat::Markdown,
            size_bytes: content.len() as u64,
            title: Some(title.clone()),
            extra,
        };

        Ok(ParsedSource {
            title,
            content: body.to_string(),
            metadata,
        })
    }
}

/// Plain text parser.
#[derive(Debug, Clone)]
pub struct PlainTextParser;

impl PlainTextParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PlainTextParser {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SourceParser for PlainTextParser {
    fn format(&self) -> SourceFormat {
        SourceFormat::PlainText
    }

    async fn parse(&self, path: &Path) -> Result<ParsedSource> {
        let content = tokio::fs::read_to_string(path).await?;
        let metadata = tokio::fs::metadata(path).await?;
        let size_bytes = metadata.len();
        let source_path = path.to_string_lossy().to_string();

        let title_from_content = content
            .lines()
            .next()
            .map(|l| l.trim().to_string())
            .filter(|s| !s.is_empty());
        let title_from_file = title_from_path(path);

        let title = title_from_content.unwrap_or(title_from_file);

        let metadata = SourceMetadata {
            source_path,
            format: SourceFormat::PlainText,
            size_bytes,
            title: Some(title.clone()),
            extra: HashMap::new(),
        };

        Ok(ParsedSource {
            title,
            content,
            metadata,
        })
    }

    fn parse_content(&self, content: &str, title: &str) -> Result<ParsedSource> {
        let title_from_content = content
            .lines()
            .next()
            .map(|l| l.trim().to_string())
            .filter(|s| !s.is_empty());

        let title = title_from_content.unwrap_or_else(|| title.to_string());

        let metadata = SourceMetadata {
            source_path: "<string>".to_string(),
            format: SourceFormat::PlainText,
            size_bytes: content.len() as u64,
            title: Some(title.clone()),
            extra: HashMap::new(),
        };

        Ok(ParsedSource {
            title,
            content: content.to_string(),
            metadata,
        })
    }
}

/// HTML parser with tag stripping and entity decoding.
#[derive(Debug, Clone)]
pub struct HtmlParser;

impl HtmlParser {
    pub fn new() -> Self {
        Self
    }

    /// Strip HTML tags from content.
    fn strip_tags(html: &str) -> String {
        let re = regex::Regex::new(r"<[^>]*>").unwrap();
        re.replace_all(html, "").to_string()
    }

    /// Decode common HTML entities.
    fn decode_entities(text: &str) -> String {
        text.replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&apos;", "'")
    }

    /// Extract title from HTML <title> or <h1> tag.
    fn extract_title(html: &str) -> Option<String> {
        // Try <title> tag first
        let title_re = regex::Regex::new(r"<title[^>]*>(.*?)</title>").unwrap();
        if let Some(caps) = title_re.captures(html) {
            if let Some(title) = caps.get(1) {
                let title = Self::decode_entities(title.as_str());
                let title = title.trim().to_string();
                if !title.is_empty() {
                    return Some(title);
                }
            }
        }

        // Try <h1> tag
        let h1_re = regex::Regex::new(r"<h1[^>]*>(.*?)</h1>").unwrap();
        if let Some(caps) = h1_re.captures(html) {
            if let Some(title) = caps.get(1) {
                let title = Self::strip_tags(title.as_str());
                let title = Self::decode_entities(&title);
                let title = title.trim().to_string();
                if !title.is_empty() {
                    return Some(title);
                }
            }
        }

        None
    }

    /// Collapse multiple whitespace and newlines.
    fn normalize_whitespace(text: &str) -> String {
        let re = regex::Regex::new(r"\s+").unwrap();
        re.replace_all(text, " ").trim().to_string()
    }
}

impl Default for HtmlParser {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SourceParser for HtmlParser {
    fn format(&self) -> SourceFormat {
        SourceFormat::Html
    }

    async fn parse(&self, path: &Path) -> Result<ParsedSource> {
        let html = tokio::fs::read_to_string(path).await?;
        let metadata = tokio::fs::metadata(path).await?;
        let size_bytes = metadata.len();
        let source_path = path.to_string_lossy().to_string();

        let title = Self::extract_title(&html);
        let content = Self::strip_tags(&html);
        let content = Self::decode_entities(&content);
        let content = Self::normalize_whitespace(&content);

        let metadata = SourceMetadata {
            source_path,
            format: SourceFormat::Html,
            size_bytes,
            title: title.clone(),
            extra: HashMap::new(),
        };

        Ok(ParsedSource {
            title: title.unwrap_or_else(|| title_from_path(path)),
            content,
            metadata,
        })
    }

    fn parse_content(&self, content: &str, title: &str) -> Result<ParsedSource> {
        let title_from_html = Self::extract_title(content);
        let content = Self::strip_tags(content);
        let content = Self::decode_entities(&content);
        let content = Self::normalize_whitespace(&content);

        let metadata = SourceMetadata {
            source_path: "<string>".to_string(),
            format: SourceFormat::Html,
            size_bytes: content.len() as u64,
            title: title_from_html.clone(),
            extra: HashMap::new(),
        };

        Ok(ParsedSource {
            title: title_from_html.unwrap_or_else(|| title.to_string()),
            content,
            metadata,
        })
    }
}

/// Conversation parser for JSON chat transcripts.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConversationEvent {
    role: String,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConversationData {
    events: Vec<ConversationEvent>,
}

/// Conversation parser for JSON chat transcripts.
#[derive(Debug, Clone)]
pub struct ConversationParser;

impl ConversationParser {
    pub fn new() -> Self {
        Self
    }

    /// Convert conversation events to readable transcript.
    fn format_transcript(events: &[ConversationEvent]) -> String {
        events
            .iter()
            .map(|event| {
                let role = if event.role.to_lowercase() == "user" {
                    "User"
                } else if event.role.to_lowercase() == "assistant" {
                    "Assistant"
                } else {
                    &event.role
                };
                format!("{}: {}", role, event.content)
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Extract title from first user message.
    fn extract_title(events: &[ConversationEvent]) -> Option<String> {
        events
            .iter()
            .find(|e| e.role.to_lowercase() == "user")
            .map(|e| {
                let mut title = e.content.clone();
                let char_count = title.chars().count();
                if char_count > 80 {
                    let end = title.floor_char_boundary(77);
                    title.truncate(end);
                    title.push_str("...");
                }
                title
            })
    }
}

impl Default for ConversationParser {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SourceParser for ConversationParser {
    fn format(&self) -> SourceFormat {
        SourceFormat::Conversation
    }

    async fn parse(&self, path: &Path) -> Result<ParsedSource> {
        let json = tokio::fs::read_to_string(path).await?;
        let metadata = tokio::fs::metadata(path).await?;
        let size_bytes = metadata.len();
        let source_path = path.to_string_lossy().to_string();

        let data: ConversationData = serde_json::from_str(&json)?;

        let title = Self::extract_title(&data.events).unwrap_or_else(|| title_from_path(path));
        let content = Self::format_transcript(&data.events);

        let metadata = SourceMetadata {
            source_path,
            format: SourceFormat::Conversation,
            size_bytes,
            title: Some(title.clone()),
            extra: HashMap::new(),
        };

        Ok(ParsedSource {
            title,
            content,
            metadata,
        })
    }

    fn parse_content(&self, content: &str, title: &str) -> Result<ParsedSource> {
        let data: ConversationData = serde_json::from_str(content)?;

        let title_from_conv = Self::extract_title(&data.events);
        let title = title_from_conv.unwrap_or_else(|| title.to_string());
        let content = Self::format_transcript(&data.events);

        let metadata = SourceMetadata {
            source_path: "<string>".to_string(),
            format: SourceFormat::Conversation,
            size_bytes: content.len() as u64,
            title: Some(title.clone()),
            extra: HashMap::new(),
        };

        Ok(ParsedSource {
            title,
            content,
            metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_markdown_parser_with_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.md");
        let content = r#"---
title: "Test Document"
tags: [test, markdown]
---

# This is ignored

Content body here."#;
        tokio::fs::write(&file, content).await.unwrap();

        let parser = MarkdownParser::new();
        let result = parser.parse(&file).await.unwrap();

        assert_eq!(result.title, "Test Document");
        assert_eq!(
            result.content.trim(),
            "# This is ignored\n\nContent body here."
        );
        assert_eq!(result.metadata.format, SourceFormat::Markdown);
        assert_eq!(
            result.metadata.extra.get("tags"),
            Some(&"[test, markdown]".to_string())
        );
    }

    #[tokio::test]
    async fn test_markdown_parser_without_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("my-doc.md");
        let content = "# Just a heading

Some content here.";
        tokio::fs::write(&file, content).await.unwrap();

        let parser = MarkdownParser::new();
        let result = parser.parse(&file).await.unwrap();

        assert_eq!(result.title, "Just a heading");
        assert_eq!(
            result.content.trim(),
            "# Just a heading\n\nSome content here."
        );
    }

    #[tokio::test]
    async fn test_markdown_parser_first_heading_as_title() {
        let parser = MarkdownParser::new();
        let content = r"# Main Heading

Content here.";
        let result = parser.parse_content(content, "fallback").unwrap();

        assert_eq!(result.title, "Main Heading");
        assert_eq!(result.content.trim(), "# Main Heading\n\nContent here.");
    }

    #[tokio::test]
    async fn test_plaintext_parser() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("note.txt");
        let content = "First line as title\n\nSecond line content.";
        tokio::fs::write(&file, content).await.unwrap();

        let parser = PlainTextParser::new();
        let result = parser.parse(&file).await.unwrap();

        assert_eq!(result.title, "First line as title");
        assert_eq!(result.content, content);
        assert_eq!(result.metadata.format, SourceFormat::PlainText);
    }

    #[tokio::test]
    async fn test_html_parser_strips_tags() {
        let parser = HtmlParser::new();
        let content = r"<p>Hello <strong>world</strong>!</p>";
        let result = parser.parse_content(content, "fallback").unwrap();

        assert_eq!(result.content, "Hello world!");
    }

    #[tokio::test]
    async fn test_html_parser_extracts_title() {
        let parser = HtmlParser::new();
        let content = r"<html><head><title>My Page</title></head><body>Content</body></html>";
        let result = parser.parse_content(content, "fallback").unwrap();

        assert_eq!(result.title, "My Page");
    }

    #[tokio::test]
    async fn test_html_parser_decodes_entities() {
        let parser = HtmlParser::new();
        let content = r"<p>&lt;tag&gt; &amp; &quot;quoted&quot;</p>";
        let result = parser.parse_content(content, "fallback").unwrap();

        assert_eq!(result.content, "<tag> & \"quoted\"");
    }

    #[tokio::test]
    async fn test_conversation_parser() {
        let parser = ConversationParser::new();
        let content = r#"{
            "events": [
                {"role": "user", "content": "Hello, how are you?"},
                {"role": "assistant", "content": "I'm doing well, thanks!"}
            ]
        }"#;
        let result = parser.parse_content(content, "fallback").unwrap();

        assert_eq!(result.title, "Hello, how are you?");
        assert!(result.content.contains("User: Hello, how are you?"));
        assert!(result
            .content
            .contains("Assistant: I'm doing well, thanks!"));
    }

    #[test]
    fn test_title_from_path() {
        assert_eq!(
            title_from_path(Path::new("/path/to/my-document.md")),
            "my-document"
        );
        assert_eq!(title_from_path(Path::new("simple.txt")), "simple");
        assert_eq!(title_from_path(Path::new("/no/extension/file")), "file");
        assert_eq!(title_from_path(Path::new("/")), "untitled");
    }
}
