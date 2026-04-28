//! Wiki ingest pipeline — converts raw information into structured wiki pages.

pub mod dedup;
pub mod extractor;
pub mod parser;

pub use parser::{
    ConversationParser, HtmlParser, MarkdownParser, ParsedSource, PlainTextParser, SourceFormat,
    SourceMetadata, SourceParser,
};

pub use extractor::{ExtractedItem, ExtractedItemType, ExtractionResult, KnowledgeExtractor};

pub use dedup::{DedupResult, SemanticDeduplicator};
