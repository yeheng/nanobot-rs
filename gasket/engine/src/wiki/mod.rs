//! Wiki knowledge management — compute and orchestration layer.
//!
//! This module owns the "business logic" half of the wiki system:
//! - Ingest pipeline (parsing, LLM extraction, dedup)
//! - Hybrid query engine (BM25 + vector + RRF)
//! - Structural lint and auto-fix
//! - Background indexing service (broker consumer)
//! - Operation log
//!
//! Data types live in `gasket_types::wiki`.
//! Persistence (PageStore, PageIndex) lives in `gasket_storage::wiki`.

pub mod indexing_service;
pub mod ingest;
pub mod lint;
pub mod log;
pub mod query;

// ── Re-exports from gasket-types (data structures) ──────────
pub use gasket_types::wiki::{Frequency, PageFilter, PageSummary, PageType, WikiPage, slugify};

// ── Re-exports from gasket-storage (persistence) ─────────────
pub use gasket_storage::wiki::{PageIndex, PageStore, create_wiki_tables};

// ── Re-exports from internal compute modules ─────────────────
pub use indexing_service::{
    WikiEmbeddingProvider, WikiIndexingService, WikiVectorHit, WikiVectorStore,
};
pub use ingest::{
    ConversationParser, DedupResult, ExtractedItem, ExtractedItemType, ExtractionResult,
    HtmlParser, KnowledgeExtractor, MarkdownParser, ParsedSource, PlainTextParser,
    SemanticDeduplicator, SourceFormat, SourceMetadata, SourceParser,
};
pub use lint::{
    extract_page_references, FixReport, LintReport, Severity, StructuralIssue,
    StructuralIssueType, StructuralLintConfig, WikiLinter,
};
pub use log::{LogEntry, WikiLog};
pub use query::{QueryResult, TokenBudget, WikiQueryEngine};
