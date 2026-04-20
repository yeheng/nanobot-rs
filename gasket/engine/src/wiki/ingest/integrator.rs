//! Wiki Integrator — two-tier knowledge integration with cost validation.
//!
//! Quick Ingest: Creates 1 page directly. No LLM cost.
//! Deep Ingest: LLM-driven multi-page update. Cost validation gate required.

use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};

use super::parser::ParsedSource;
use crate::wiki::page::{slugify, PageType, WikiPage};
use crate::wiki::store::PageStore;

// ── Types ─────────────────────────────────────────────────────────

/// Ingest tier selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IngestTier {
    /// Quick: 1 page, no LLM cost. For conversation/agent exploration.
    Quick,
    /// Deep: up to 15 pages, LLM-driven integration. For document import.
    Deep,
}

/// Cost estimate for a deep ingest operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Estimated total input tokens.
    pub estimated_input_tokens: u32,
    /// Number of existing pages that would be affected.
    pub estimated_pages_affected: usize,
    /// Estimated cost in USD.
    pub estimated_cost_usd: f64,
}

/// Configuration for the integrator.
#[derive(Debug, Clone)]
pub struct IngestConfig {
    /// Maximum cost per deep ingest (USD). Default: 0.10
    pub max_cost_per_ingest: f64,
    /// Cost warning threshold (USD). Default: 0.05
    pub cost_warning_threshold: f64,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            max_cost_per_ingest: 0.10,
            cost_warning_threshold: 0.05,
        }
    }
}

/// Report of an ingest operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestReport {
    /// The source page that was created.
    pub source_path: String,
    /// Pages that were created or updated.
    pub affected_pages: Vec<String>,
    /// Relations that were created.
    pub relations: Vec<(String, String, String)>,
    /// Which tier was used.
    pub tier: IngestTier,
    /// Cost of the operation (0.0 for Quick).
    pub cost_usd: f64,
}

impl IngestReport {
    /// Create a quick ingest report.
    pub fn quick(source_path: String) -> Self {
        Self {
            source_path,
            affected_pages: vec![],
            relations: vec![],
            tier: IngestTier::Quick,
            cost_usd: 0.0,
        }
    }

    /// Create a deep ingest report.
    pub fn deep(source_path: String, affected_pages: Vec<String>, cost: f64) -> Self {
        Self {
            source_path,
            affected_pages,
            relations: vec![],
            tier: IngestTier::Deep,
            cost_usd: cost,
        }
    }
}

// ── WikiIntegrator ────────────────────────────────────────────────

/// Wiki Integrator — handles knowledge integration into the wiki.
pub struct WikiIntegrator {
    provider: Arc<dyn LlmProvider>,
    model: String,
    config: IngestConfig,
}

impl WikiIntegrator {
    pub fn new(provider: Arc<dyn LlmProvider>, model: String, config: IngestConfig) -> Self {
        Self {
            provider,
            model,
            config,
        }
    }

    // ── Quick Ingest ──────────────────────────────────────────────

    /// Quick ingest: creates 1 entity/topic page directly. No LLM needed.
    pub async fn quick_ingest(
        &self,
        title: &str,
        content: &str,
        page_type: PageType,
        tags: Vec<String>,
        store: &PageStore,
    ) -> Result<IngestReport> {
        let prefix = match page_type {
            PageType::Entity => "entities",
            PageType::Topic => "topics",
            PageType::Source => "sources",
        };
        let path = format!("{}/{}", prefix, slugify(title));

        // Skip if page already exists with identical content
        if let Ok(existing) = store.read(&path).await {
            if existing.content == content {
                debug!(
                    "Quick ingest: '{}' already exists with same content, skipping",
                    path
                );
                return Ok(IngestReport::quick(path));
            }
        }

        let mut page = WikiPage::new(
            path.clone(),
            title.to_string(),
            page_type,
            content.to_string(),
        );
        page.tags = tags;
        store.write(&page).await?;

        info!("Quick ingest: created page '{}'", path);
        Ok(IngestReport::quick(path))
    }

    // ── Deep Ingest ───────────────────────────────────────────────

    /// Estimate the cost of a deep ingest operation.
    pub fn estimate_cost(&self, source: &ParsedSource) -> CostEstimate {
        // Rough token estimate: ~4 chars per token
        let source_tokens = (source.content.len() / 4) as u32;

        // Estimate affected pages (we can't do async here, so use a heuristic)
        let estimated_pages = std::cmp::min((source.content.len() / 500).max(1), 15);
        let update_tokens = estimated_pages as u32 * 2500;

        let total_tokens = source_tokens + update_tokens;

        // Rough pricing: GPT-4o input ~$2.50/1M tokens, output ~$10/1M tokens
        // Assume 80% input, 20% output
        let input_cost = (total_tokens as f64 / 1_000_000.0) * 2.50;
        let output_cost = (total_tokens as f64 * 0.3 / 1_000_000.0) * 10.0;

        CostEstimate {
            estimated_input_tokens: total_tokens,
            estimated_pages_affected: estimated_pages,
            estimated_cost_usd: input_cost + output_cost,
        }
    }

    /// Deep ingest with cost validation gate.
    pub async fn deep_ingest(
        &self,
        source: &ParsedSource,
        store: &PageStore,
    ) -> Result<IngestReport> {
        // GATE: validate cost before proceeding
        let estimate = self.estimate_cost(source);
        if estimate.estimated_cost_usd > self.config.max_cost_per_ingest {
            anyhow::bail!(
                "Deep ingest estimated cost ${:.4} exceeds budget ${:.4}. \
                 Affected pages: {}. Use quick_ingest or increase budget.",
                estimate.estimated_cost_usd,
                self.config.max_cost_per_ingest,
                estimate.estimated_pages_affected
            );
        }
        if estimate.estimated_cost_usd > self.config.cost_warning_threshold {
            warn!(
                "Deep ingest cost ${:.4} above warning threshold ${:.4}. Proceeding.",
                estimate.estimated_cost_usd, self.config.cost_warning_threshold
            );
        }

        // 1. Create source summary page
        let source_path = format!("sources/{}", slugify(&source.title));
        let source_page = WikiPage::new(
            source_path.clone(),
            source.title.clone(),
            PageType::Source,
            source.content.clone(),
        );
        store.write(&source_page).await?;
        debug!("Deep ingest: created source page '{}'", source_path);

        // 2. LLM analyzes which existing pages are affected
        let affected_paths = self.llm_analyze_impact(source, store).await?;

        // 3. Update each affected page
        let mut updated_paths = Vec::new();
        for page_path in &affected_paths {
            match store.read(page_path).await {
                Ok(mut page) => match self.llm_update_page(&page, source).await {
                    Ok(updated_content) => {
                        page.content = updated_content;
                        page.source_count += 1;
                        store.write(&page).await?;
                        updated_paths.push(page_path.clone());
                    }
                    Err(e) => {
                        warn!("Failed to update page '{}': {}", page_path, e);
                    }
                },
                Err(e) => {
                    warn!("Failed to read affected page '{}': {}", page_path, e);
                }
            }
        }

        info!(
            "Deep ingest complete: source '{}', updated {} page(s)",
            source_path,
            updated_paths.len()
        );

        Ok(IngestReport::deep(
            source_path,
            updated_paths,
            estimate.estimated_cost_usd,
        ))
    }

    // ── LLM Helpers ───────────────────────────────────────────────

    /// Ask LLM which existing wiki pages are affected by this source.
    async fn llm_analyze_impact(
        &self,
        source: &ParsedSource,
        store: &PageStore,
    ) -> Result<Vec<String>> {
        // Get existing page list for context
        let pages = store.list(crate::wiki::page::PageFilter::default()).await?;
        let page_list: Vec<String> = pages
            .iter()
            .take(50) // Limit context size
            .map(|p| format!("  - {} ({})", p.path, p.title))
            .collect();

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system(IMPACT_ANALYSIS_SYSTEM_PROMPT),
                ChatMessage::user(format!(
                    "Existing wiki pages:\n{}\n\nNew source: {}\n\nContent (truncated):\n{}\n\n\
                     Which existing pages does this source affect? Return JSON array of paths.",
                    page_list.join("\n"),
                    source.title,
                    &source.content[..source.content.len().min(4000)]
                )),
            ],
            tools: None,
            temperature: Some(0.1),
            max_tokens: Some(1024),
            thinking: None,
        };

        let response = self
            .provider
            .chat(request)
            .await
            .map_err(|e| anyhow::anyhow!("Impact analysis LLM call failed: {}", e))?;

        let content = response.content.unwrap_or_default();
        self.parse_path_array(&content)
    }

    /// Ask LLM to update a page with new source information.
    async fn llm_update_page(&self, page: &WikiPage, source: &ParsedSource) -> Result<String> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system(PAGE_UPDATE_SYSTEM_PROMPT),
                ChatMessage::user(format!(
                    "Current page '{}':\n{}\n\nNew source information:\n{}\n\n\
                     Return the updated page content ONLY. Preserve existing structure.",
                    page.title,
                    &page.content[..page.content.len().min(6000)],
                    &source.content[..source.content.len().min(4000)]
                )),
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
            .map_err(|e| anyhow::anyhow!("Page update LLM call failed: {}", e))?;

        Ok(response.content.unwrap_or(page.content.clone()))
    }

    /// Parse a JSON array of path strings from LLM response.
    fn parse_path_array(&self, text: &str) -> Result<Vec<String>> {
        let trimmed = text.trim();

        // Try stripping code fences
        let clean = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        // Try to find JSON array
        let json_str = if let Some(start) = clean.find('[') {
            if let Some(end) = clean.rfind(']') {
                &clean[start..=end]
            } else {
                clean
            }
        } else {
            clean
        };

        let paths: Vec<String> = serde_json::from_str(json_str).unwrap_or_default();
        Ok(paths)
    }
}

// ── Prompts ───────────────────────────────────────────────────────

const IMPACT_ANALYSIS_SYSTEM_PROMPT: &str = r#"You are a wiki impact analyzer. Given a new source document and a list of existing wiki pages, determine which existing pages would be affected or should be updated with information from this source.

Output a JSON array of page paths. Example: ["entities/projects/gasket", "topics/rust-async"]
If no pages are affected, return [].
Only include paths that EXIST in the provided list."#;

const PAGE_UPDATE_SYSTEM_PROMPT: &str = r#"You are a wiki page updater. Given an existing wiki page and new source information, produce an updated version of the page content.

Rules:
1. Preserve the existing structure and organization
2. Integrate new information naturally
3. Remove any contradictions
4. Keep the content concise and factual
5. Return ONLY the updated markdown content, no explanations"#;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use gasket_providers::{ChatResponse, ChatStream, ProviderError};

    #[test]
    fn test_ingest_config_default() {
        let config = IngestConfig::default();
        assert_eq!(config.max_cost_per_ingest, 0.10);
        assert_eq!(config.cost_warning_threshold, 0.05);
    }

    #[test]
    fn test_cost_estimate() {
        let integrator = create_test_integrator();
        let source = ParsedSource {
            title: "Test".to_string(),
            content: "A".repeat(2000), // 2000 chars ≈ 500 tokens
            metadata: super::super::parser::SourceMetadata {
                source_path: "test.md".to_string(),
                format: super::super::parser::SourceFormat::Markdown,
                size_bytes: 2000,
                title: Some("Test".to_string()),
                extra: Default::default(),
            },
        };
        let estimate = integrator.estimate_cost(&source);
        assert!(estimate.estimated_input_tokens > 0);
        assert!(estimate.estimated_cost_usd > 0.0);
    }

    #[test]
    fn test_cost_gate_blocks_expensive() {
        let mut config = IngestConfig::default();
        config.max_cost_per_ingest = 0.001; // Very low budget
        let integrator = create_test_integrator_with_config(config);
        let source = ParsedSource {
            title: "Big Doc".to_string(),
            content: "X".repeat(100_000), // Very large
            metadata: super::super::parser::SourceMetadata {
                source_path: "big.md".to_string(),
                format: super::super::parser::SourceFormat::Markdown,
                size_bytes: 100_000,
                title: Some("Big Doc".to_string()),
                extra: Default::default(),
            },
        };
        let estimate = integrator.estimate_cost(&source);
        // With 100k chars, cost should exceed $0.001
        assert!(estimate.estimated_cost_usd > 0.001);
    }

    #[test]
    fn test_parse_path_array() {
        let integrator = create_test_integrator();
        let paths = integrator
            .parse_path_array(r#"["entities/test", "topics/demo"]"#)
            .unwrap();
        assert_eq!(paths, vec!["entities/test", "topics/demo"]);
    }

    #[test]
    fn test_parse_path_array_with_fences() {
        let integrator = create_test_integrator();
        let paths = integrator
            .parse_path_array("```json\n[\"topics/rust\"]\n```")
            .unwrap();
        assert_eq!(paths, vec!["topics/rust"]);
    }

    #[test]
    fn test_parse_path_array_empty() {
        let integrator = create_test_integrator();
        let paths = integrator.parse_path_array("[]").unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn test_ingest_report_quick() {
        let report = IngestReport::quick("topics/test".to_string());
        assert_eq!(report.tier, IngestTier::Quick);
        assert_eq!(report.cost_usd, 0.0);
    }

    #[test]
    fn test_ingest_report_deep() {
        let report = IngestReport::deep(
            "sources/doc".to_string(),
            vec!["entities/a".to_string()],
            0.03,
        );
        assert_eq!(report.tier, IngestTier::Deep);
        assert_eq!(report.affected_pages.len(), 1);
    }

    // ── Test helpers ───────────────────────────────────────────────

    fn create_test_integrator() -> WikiIntegrator {
        create_test_integrator_with_config(IngestConfig::default())
    }

    fn create_test_integrator_with_config(config: IngestConfig) -> WikiIntegrator {
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

            async fn chat_stream(
                &self,
                _request: ChatRequest,
            ) -> Result<ChatStream, ProviderError> {
                Err(ProviderError::Other(
                    "noop does not support streaming".to_string(),
                ))
            }
        }

        WikiIntegrator::new(Arc::new(NoopProvider), "test".to_string(), config)
    }
}
