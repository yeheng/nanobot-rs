pub mod index;
pub mod indexing_service;
pub mod ingest;
pub mod lifecycle;
pub mod lint;
pub mod log;
pub mod page;
pub mod query;
pub mod store;

// Re-exports
pub use index::PageIndex;
pub use indexing_service::{
    WikiEmbeddingProvider, WikiIndexingService, WikiVectorHit, WikiVectorStore,
};
pub use ingest::{
    ConversationParser, DedupResult, ExtractedItem, ExtractedItemType, ExtractionResult,
    HtmlParser, KnowledgeExtractor, MarkdownParser, ParsedSource, PlainTextParser,
    SemanticDeduplicator, SourceFormat, SourceMetadata, SourceParser,
};
pub use lifecycle::{DecayReport, FrequencyManager};
pub use lint::{
    extract_page_references, FixReport, LintReport, Severity, StructuralIssue,
    StructuralIssueType, StructuralLintConfig, WikiLinter,
};
pub use log::{LogEntry, WikiLog};
pub use page::{slugify, PageFilter, PageSummary, PageType, WikiPage};
pub use query::{QueryResult, TokenBudget, WikiQueryEngine};
pub use store::PageStore;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_new_entity() {
        let page = WikiPage::new(
            "entities/projects/gasket".to_string(),
            "Gasket Project".to_string(),
            PageType::Entity,
            "A Rust agent framework.".to_string(),
        );
        assert_eq!(page.path, "entities/projects/gasket");
        assert_eq!(page.page_type, PageType::Entity);
        assert_eq!(page.confidence, 1.0);
        assert!(page.tags.is_empty());
    }

    #[test]
    fn test_page_new_topic() {
        let page = WikiPage::new(
            "topics/rust-async".to_string(),
            "Rust Async".to_string(),
            PageType::Topic,
            "How async works in Rust.".to_string(),
        );
        assert_eq!(page.page_type, PageType::Topic);
    }

    #[test]
    fn test_page_markdown_roundtrip() {
        let mut page = WikiPage::new(
            "topics/test".to_string(),
            "Test Topic".to_string(),
            PageType::Topic,
            "Some content here.".to_string(),
        );
        page.tags = vec!["test".to_string()];
        let md = page.to_markdown();
        let parsed = WikiPage::from_markdown("topics/test".to_string(), &md).unwrap();
        assert_eq!(parsed.title, "Test Topic");
        assert_eq!(parsed.content, "Some content here.");
        assert_eq!(parsed.tags, vec!["test"]);
    }

    #[test]
    fn test_page_markdown_roundtrip_with_summary() {
        let mut page = WikiPage::new(
            "topics/rust-async".to_string(),
            "Rust Async".to_string(),
            PageType::Topic,
            "Detailed content about async programming.".to_string(),
        );
        page.summary = Some("Rust async programming overview".to_string());
        let md = page.to_markdown();
        assert!(md.contains("summary: Rust async programming overview"));
        let parsed = WikiPage::from_markdown("topics/rust-async".to_string(), &md).unwrap();
        assert_eq!(parsed.summary, Some("Rust async programming overview".to_string()));
        assert_eq!(parsed.content, "Detailed content about async programming.");
    }

    #[test]
    fn test_page_fallback_summary_from_content() {
        let md = "---\ntitle: No Summary\ntype: topic\n---\n\nThis is the body content that should become the fallback summary.";
        let parsed = WikiPage::from_markdown("topics/no-summary".to_string(), md).unwrap();
        assert!(parsed.summary.is_some());
        let s = parsed.summary.unwrap();
        assert!(s.starts_with("This is the body content"));
        assert!(s.len() <= 104); // 100 chars + "..."
    }

    #[test]
    fn test_make_path() {
        assert_eq!(
            WikiPage::make_path(&["entities", "projects", "gasket"]),
            "entities/projects/gasket"
        );
        assert_eq!(
            WikiPage::make_path(&["topics", "rust-async"]),
            "topics/rust-async"
        );
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("Rust & LLM"), "rust-llm");
        assert_eq!(slugify("  spaces  "), "spaces");
    }

    #[test]
    fn test_page_type_roundtrip() {
        assert_eq!("entity".parse(), Ok(PageType::Entity));
        assert_eq!("topic".parse(), Ok(PageType::Topic));
        assert_eq!("source".parse(), Ok(PageType::Source));
        assert_eq!("sop".parse(), Ok(PageType::Sop));
        assert!("unknown".parse::<PageType>().is_err());
    }

    #[test]
    fn test_page_type_sop_directory() {
        assert_eq!(PageType::Sop.as_str(), "sop");
        assert_eq!(PageType::Sop.directory(), "sops");
    }

    #[test]
    fn test_sop_page_roundtrip() {
        let page = WikiPage::new(
            "sops/docker-build".to_string(),
            "Docker Build SOP".to_string(),
            PageType::Sop,
            "1. Check Dockerfile exists\n2. Run docker build".to_string(),
        );
        let md = page.to_markdown();
        let parsed = WikiPage::from_markdown("sops/docker-build".to_string(), &md).unwrap();
        assert_eq!(parsed.page_type, PageType::Sop);
        assert_eq!(parsed.title, "Docker Build SOP");
    }
}
