//! LLM-based knowledge extraction from parsed sources.
//!
//! Takes a `ParsedSource` and uses an LLM to extract structured knowledge
//! items: entities, concepts, claims, and relationships.

use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};

use super::parser::ParsedSource;
use crate::wiki::page::slugify;

/// A single extracted knowledge item from a source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedItem {
    /// Title for this knowledge item.
    pub title: String,
    /// Type classification.
    #[serde(rename = "type")]
    pub item_type: ExtractedItemType,
    /// The knowledge content (markdown).
    pub content: String,
    /// Suggested wiki path (e.g., "entities/projects/gasket").
    #[serde(default)]
    pub suggested_path: Option<String>,
    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Confidence score 0.0-1.0.
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

fn default_confidence() -> f64 {
    0.8
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractedItemType {
    Entity,
    Topic,
    Claim,
}

/// Result of knowledge extraction.
#[derive(Debug, Clone)]
pub struct ExtractionResult {
    /// Extracted knowledge items.
    pub items: Vec<ExtractedItem>,
    /// Number of LLM tokens used (approximate).
    pub tokens_used: u32,
}

/// LLM-based knowledge extractor.
pub struct KnowledgeExtractor {
    provider: Arc<dyn LlmProvider>,
    model: String,
}

impl KnowledgeExtractor {
    pub fn new(provider: Arc<dyn LlmProvider>, model: String) -> Self {
        Self { provider, model }
    }

    /// Extract knowledge items from a parsed source.
    pub async fn extract(&self, source: &ParsedSource) -> Result<ExtractionResult> {
        // Build extraction prompt
        let system_prompt = EXTRACTION_SYSTEM_PROMPT;
        let user_prompt = self.build_user_prompt(source);

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system(system_prompt),
                ChatMessage::user(user_prompt),
            ],
            tools: None,
            temperature: Some(0.3),
            max_tokens: Some(4096),
            thinking: None,
        };

        let response = self
            .provider
            .chat(request)
            .await
            .map_err(|e| anyhow::anyhow!("Knowledge extraction LLM call failed: {}", e))?;

        let content = response.content.unwrap_or_default();
        let tokens_used = response
            .usage
            .as_ref()
            .map(|u| u.total_tokens as u32)
            .unwrap_or(0);

        // Parse the LLM response
        let items = self.parse_response(&content);

        if items.is_empty() {
            debug!(
                "KnowledgeExtractor: no items extracted from '{}'",
                source.title
            );
        } else {
            debug!(
                "KnowledgeExtractor: extracted {} item(s) from '{}'",
                items.len(),
                source.title
            );
        }

        Ok(ExtractionResult { items, tokens_used })
    }

    fn build_user_prompt(&self, source: &ParsedSource) -> String {
        // Truncate very long sources to avoid token overflow
        let max_chars = 12000;
        let content = if source.content.len() > max_chars {
            format!(
                "{}\n\n[... content truncated, showing first {} chars]\n{}",
                &source.content[..max_chars],
                max_chars,
                ""
            )
        } else {
            source.content.clone()
        };

        format!(
            "Source title: {}\nSource format: {:?}\n\nContent:\n{}\n\n\
             Extract structured knowledge items as a JSON array.",
            source.title, source.metadata.format, content
        )
    }

    /// Parse LLM response into ExtractedItems.
    /// Handles markdown code blocks and various JSON formats.
    fn parse_response(&self, text: &str) -> Vec<ExtractedItem> {
        let trimmed = text.trim();

        // Try direct parse
        if let Ok(items) = serde_json::from_str::<Vec<ExtractedItem>>(trimmed) {
            return self.normalize_items(items);
        }

        // Try stripping markdown code block
        let without_fences = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        if let Ok(items) = serde_json::from_str::<Vec<ExtractedItem>>(without_fences) {
            return self.normalize_items(items);
        }

        // Try finding JSON array boundaries
        if let Some(start) = trimmed.find('[') {
            if let Some(end) = trimmed.rfind(']') {
                if end > start {
                    let slice = &trimmed[start..=end];
                    if let Ok(items) = serde_json::from_str::<Vec<ExtractedItem>>(slice) {
                        return self.normalize_items(items);
                    }
                }
            }
        }

        warn!("KnowledgeExtractor: failed to parse LLM response as JSON array");
        vec![]
    }

    /// Normalize extracted items: ensure paths, fill defaults.
    fn normalize_items(&self, items: Vec<ExtractedItem>) -> Vec<ExtractedItem> {
        items
            .into_iter()
            .map(|mut item| {
                // Generate suggested_path if not provided
                if item.suggested_path.is_none() {
                    let prefix = match item.item_type {
                        ExtractedItemType::Entity => "entities",
                        ExtractedItemType::Topic => "topics",
                        ExtractedItemType::Claim => "topics",
                    };
                    item.suggested_path = Some(format!("{}/{}", prefix, slugify(&item.title)));
                }
                item
            })
            .collect()
    }
}

const EXTRACTION_SYSTEM_PROMPT: &str = r#"You are a structured data extraction engine specializing in knowledge management.

Your task is to analyze source content and extract NEW, PERSISTENT knowledge items.

CRITICAL RULES:
1. Output ONLY a valid JSON array. No markdown, no explanations.
2. Each item must have: title, type (entity/topic/claim), content, suggested_path, tags, confidence.
3. DO NOT extract trivial or transient information.
4. Focus on concrete facts, definitions, relationships, architectural decisions.
5. If nothing valuable is found, return [].

Item types:
- entity: A named thing (person, project, tool, concept). Content should describe what it IS.
- topic: A synthesis or guide about a subject. Content should explain HOW or WHY.
- claim: A specific factual assertion that can be verified. Content should state WHAT.

Format:
[{"title": "...", "type": "entity|topic|claim", "content": "markdown body", "suggested_path": "entities/name or topics/name", "tags": ["tag1"], "confidence": 0.9}]"#;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use gasket_providers::{ChatResponse, ChatStream, ProviderError};

    /// Test-only noop provider for testing parse_response logic.
    struct NoopProvider;

    #[async_trait]
    impl LlmProvider for NoopProvider {
        fn name(&self) -> &str {
            "noop"
        }

        fn default_model(&self) -> &str {
            "noop"
        }

        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, ProviderError> {
            Ok(ChatResponse {
                content: Some("[]".to_string()),
                tool_calls: vec![],
                reasoning_content: None,
                usage: None,
            })
        }

        async fn chat_stream(&self, _request: ChatRequest) -> Result<ChatStream, ProviderError> {
            Err(ProviderError::Other(
                "noop does not support streaming".to_string(),
            ))
        }
    }

    impl KnowledgeExtractor {
        /// Create a test-only instance (no actual LLM calls).
        /// Only use for testing parse_response().
        #[cfg(test)]
        fn for_test() -> Self {
            Self {
                provider: Arc::new(NoopProvider),
                model: "test-model".to_string(),
            }
        }
    }

    #[test]
    fn test_parse_response_direct_json() {
        let extractor = KnowledgeExtractor::for_test();
        let json = r#"[{"title": "Rust", "type": "entity", "content": "A systems programming language.", "tags": ["programming"]}]"#;
        let items = extractor.parse_response(json);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Rust");
        assert_eq!(items[0].item_type, ExtractedItemType::Entity);
    }

    #[test]
    fn test_parse_response_markdown_fenced() {
        let extractor = KnowledgeExtractor::for_test();
        let response = "```json\n[{\"title\": \"Test\", \"type\": \"topic\", \"content\": \"Test content\", \"tags\": []}]\n```";
        let items = extractor.parse_response(response);
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn test_parse_response_with_surrounding_text() {
        let extractor = KnowledgeExtractor::for_test();
        let response = "Here are the extracted items:\n[{\"title\": \"X\", \"type\": \"claim\", \"content\": \"Y\", \"tags\": []}]\nDone.";
        let items = extractor.parse_response(response);
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn test_parse_response_empty_array() {
        let extractor = KnowledgeExtractor::for_test();
        let items = extractor.parse_response("[]");
        assert!(items.is_empty());
    }

    #[test]
    fn test_parse_response_invalid() {
        let extractor = KnowledgeExtractor::for_test();
        let items = extractor.parse_response("This is not JSON at all.");
        assert!(items.is_empty());
    }

    #[test]
    fn test_normalize_items_generates_path() {
        let extractor = KnowledgeExtractor::for_test();
        let items = extractor.normalize_items(vec![ExtractedItem {
            title: "My Project".to_string(),
            item_type: ExtractedItemType::Entity,
            content: "A cool project.".to_string(),
            suggested_path: None,
            tags: vec![],
            confidence: 0.9,
        }]);
        assert_eq!(
            items[0].suggested_path,
            Some("entities/my-project".to_string())
        );
    }
}
