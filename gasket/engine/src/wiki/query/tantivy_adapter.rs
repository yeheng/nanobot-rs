//! Tantivy index adapter for wiki pages.
//!
//! Provides BM25 full-text search over wiki pages using Tantivy.
//! Index location: `~/.gasket/wiki/.tantivy/`
//!
//! Schema fields:
//! - path (STRING): document identity, exact match
//! - title (TEXT): BM25 tokenized search
//! - content (TEXT): BM25 tokenized search
//! - page_type (STRING): filter by Entity/Topic/Source
//! - category (STRING): optional category filter
//! - tags (STRING, multi-value): tag filter
//! - confidence (F64): relevance boosting

use std::path::PathBuf;

use anyhow::Result;
use parking_lot::Mutex;
use tantivy::{
    collector::TopDocs,
    directory::MmapDirectory,
    query::{BooleanQuery, Occur, QueryParser, TermQuery},
    schema::{
        Field, IndexRecordOption, Schema, SchemaBuilder, TextFieldIndexing, TextOptions, Value,
        STORED, STRING,
    },
    Index, IndexReader, IndexWriter, TantivyDocument, Term,
};
use tracing::{debug, info};

use super::super::page::{PageType, WikiPage};

// ── Schema Fields ──────────────────────────────────────────────────

/// Holds all Tantivy field handles for the wiki index.
pub struct WikiFields {
    pub path: Field,
    pub title: Field,
    pub content: Field,
    pub page_type: Field,
    pub category: Field,
    pub tags: Field,
    pub confidence: Field,
}

impl WikiFields {
    /// Build the Tantivy schema and return (schema, fields).
    fn build() -> (Schema, Self) {
        let mut builder = SchemaBuilder::new();

        let text_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer("default")
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();

        let path = builder.add_text_field("path", STRING | STORED);
        let title = builder.add_text_field("title", text_options.clone());
        let content = builder.add_text_field("content", text_options);
        let page_type = builder.add_text_field("page_type", STRING | STORED);
        let category = builder.add_text_field("category", STRING | STORED);
        let tags = builder.add_text_field("tags", STRING | STORED);
        let confidence = builder.add_f64_field("confidence", STORED);

        (
            builder.build(),
            Self {
                path,
                title,
                content,
                page_type,
                category,
                tags,
                confidence,
            },
        )
    }
}

// ── Search Result ──────────────────────────────────────────────────

/// A search hit from the Tantivy index.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Wiki page path (e.g., "entities/projects/gasket").
    pub path: String,
    /// BM25 relevance score.
    pub score: f32,
    /// Page title (for display).
    pub title: String,
}

// ── Tantivy Index ──────────────────────────────────────────────────

/// Tantivy-backed full-text index for wiki pages.
///
/// Thread-safe: uses `parking_lot::Mutex` for writer access.
/// Reader is always available (Tantivy's `IndexReader` is `Send + Sync`).
pub struct TantivyIndex {
    index: Index,
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
    fields: WikiFields,
}

impl TantivyIndex {
    /// Open or create the Tantivy index at the given directory.
    pub fn open(index_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&index_dir)?;

        let (schema, fields) = WikiFields::build();
        let directory = MmapDirectory::open(&index_dir)?;
        let index = Index::open_or_create(directory, schema)?;
        let reader = index.reader()?;
        let writer = index.writer(50_000_000)?; // 50MB heap

        info!("Opened Tantivy wiki index at {:?}", index_dir);

        Ok(Self {
            index,
            reader,
            writer: Mutex::new(writer),
            fields,
        })
    }

    /// Open an existing index (fails if not yet created).
    pub fn open_existing(index_dir: PathBuf) -> Result<Self> {
        let (_schema, fields) = WikiFields::build();
        let directory = MmapDirectory::open(&index_dir)?;
        let index = Index::open(directory)?;
        let reader = index.reader()?;
        let writer = index.writer(50_000_000)?;

        Ok(Self {
            index,
            reader,
            writer: Mutex::new(writer),
            fields,
        })
    }

    /// Upsert a wiki page into the index. Deletes any existing doc with same path first.
    pub fn upsert(&self, page: &WikiPage) -> Result<()> {
        let mut writer = self.writer.lock();

        // Delete existing document with same path
        let delete_term = Term::from_field_text(self.fields.path, &page.path);
        writer.delete_term(delete_term);

        // Build new document
        let mut doc = TantivyDocument::new();
        doc.add_text(self.fields.path, &page.path);
        doc.add_text(self.fields.title, &page.title);
        doc.add_text(self.fields.content, &page.content);
        doc.add_text(self.fields.page_type, page.page_type.as_str());

        if let Some(ref cat) = page.category {
            doc.add_text(self.fields.category, cat);
        }

        for tag in &page.tags {
            doc.add_text(self.fields.tags, tag);
        }

        doc.add_f64(self.fields.confidence, page.confidence);

        writer.add_document(doc)?;
        writer.commit()?;

        // Reload reader to see new documents
        self.reader.reload()?;

        debug!("Tantivy upsert: '{}'", page.path);
        Ok(())
    }

    /// Delete a page from the index by path.
    pub fn delete(&self, path: &str) -> Result<()> {
        let mut writer = self.writer.lock();
        let delete_term = Term::from_field_text(self.fields.path, path);
        writer.delete_term(delete_term);
        writer.commit()?;
        self.reader.reload()?;

        debug!("Tantivy delete: '{}'", path);
        Ok(())
    }

    /// BM25 search across title and content fields.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let searcher = self.reader.searcher();

        // Build query: search both title and content fields
        let title_parser = QueryParser::for_index(&self.index, vec![self.fields.title]);
        let content_parser = QueryParser::for_index(&self.index, vec![self.fields.content]);

        let mut sub_queries: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();

        // Title matches get higher boost
        match title_parser.parse_query(query) {
            Ok(q) => sub_queries.push((Occur::Should, q)),
            Err(e) => {
                debug!("Title query parse failed, using term fallback: {}", e);
                self.add_term_fallback(&mut sub_queries, query, self.fields.title);
            }
        }

        match content_parser.parse_query(query) {
            Ok(q) => sub_queries.push((Occur::Should, q)),
            Err(e) => {
                debug!("Content query parse failed, using term fallback: {}", e);
                self.add_term_fallback(&mut sub_queries, query, self.fields.content);
            }
        }

        if sub_queries.is_empty() {
            return Ok(vec![]);
        }

        let combined = BooleanQuery::new(sub_queries);
        let top_docs = searcher.search(&combined, &TopDocs::with_limit(limit))?;

        let mut hits = Vec::new();
        for (score, doc_address) in top_docs {
            if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                let path = doc
                    .get_first(self.fields.path)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let title = doc
                    .get_first(self.fields.title)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                hits.push(SearchHit {
                    path,
                    score,
                    title,
                });
            }
        }

        Ok(hits)
    }

    /// Search with page type filter.
    pub fn search_by_type(
        &self,
        query: &str,
        page_type: PageType,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let searcher = self.reader.searcher();

        // Type filter query
        let type_term = Term::from_field_text(self.fields.page_type, page_type.as_str());
        let type_query: Box<dyn tantivy::query::Query> = Box::new(TermQuery::new(
            type_term,
            IndexRecordOption::Basic,
        ));

        // Text search queries
        let text_parser = QueryParser::for_index(
            &self.index,
            vec![self.fields.title, self.fields.content],
        );

        let text_query: Box<dyn tantivy::query::Query> = match text_parser.parse_query(query) {
            Ok(q) => q,
            Err(_) => {
                // Fallback: simple term query on title
                let term = Term::from_field_text(self.fields.title, query);
                Box::new(TermQuery::new(term, IndexRecordOption::WithFreqsAndPositions))
            }
        };

        let combined = BooleanQuery::new(vec![
            (Occur::Must, type_query),
            (Occur::Must, text_query),
        ]);

        let top_docs = searcher.search(&combined, &TopDocs::with_limit(limit))?;

        let mut hits = Vec::new();
        for (score, doc_address) in top_docs {
            if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                let path = doc
                    .get_first(self.fields.path)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let title = doc
                    .get_first(self.fields.title)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                hits.push(SearchHit { path, score, title });
            }
        }

        Ok(hits)
    }

    /// Search by tags only (exact match on tag values).
    pub fn search_by_tags(&self, tags: &[String], limit: usize) -> Result<Vec<SearchHit>> {
        if tags.is_empty() {
            return Ok(vec![]);
        }

        let searcher = self.reader.searcher();

        let tag_queries: Vec<(Occur, Box<dyn tantivy::query::Query>)> = tags
            .iter()
            .map(|tag| {
                let term = Term::from_field_text(self.fields.tags, tag);
                let query: Box<dyn tantivy::query::Query> = Box::new(TermQuery::new(
                    term,
                    IndexRecordOption::Basic,
                ));
                (Occur::Should, query)
            })
            .collect();

        let combined = BooleanQuery::new(tag_queries);
        let top_docs = searcher.search(&combined, &TopDocs::with_limit(limit))?;

        let mut hits = Vec::new();
        for (score, doc_address) in top_docs {
            if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                let path = doc
                    .get_first(self.fields.path)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let title = doc
                    .get_first(self.fields.title)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                hits.push(SearchHit { path, score, title });
            }
        }

        Ok(hits)
    }

    /// Get the number of documents in the index.
    pub fn doc_count(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    /// Rebuild the index from a full set of wiki pages.
    /// Use when the index is corrupted or needs full refresh.
    pub fn rebuild(&self, pages: &[WikiPage]) -> Result<usize> {
        let mut writer = self.writer.lock();

        // Delete all existing documents
        writer.delete_all_documents()?;
        writer.commit()?;

        // Add all pages
        for page in pages {
            let mut doc = TantivyDocument::new();
            doc.add_text(self.fields.path, &page.path);
            doc.add_text(self.fields.title, &page.title);
            doc.add_text(self.fields.content, &page.content);
            doc.add_text(self.fields.page_type, page.page_type.as_str());

            if let Some(ref cat) = page.category {
                doc.add_text(self.fields.category, cat);
            }

            for tag in &page.tags {
                doc.add_text(self.fields.tags, tag);
            }

            doc.add_f64(self.fields.confidence, page.confidence);
            writer.add_document(doc)?;
        }

        writer.commit()?;
        self.reader.reload()?;

        info!("Rebuilt Tantivy index with {} pages", pages.len());
        Ok(pages.len())
    }

    // ── Helpers ────────────────────────────────────────────────────

    /// Add term-level fallback queries when QueryParser fails.
    fn add_term_fallback(
        &self,
        queries: &mut Vec<(Occur, Box<dyn tantivy::query::Query>)>,
        query_text: &str,
        field: Field,
    ) {
        for word in query_text.split_whitespace() {
            let term = Term::from_field_text(field, word);
            let term_query: Box<dyn tantivy::query::Query> = Box::new(TermQuery::new(
                term,
                IndexRecordOption::WithFreqsAndPositions,
            ));
            queries.push((Occur::Should, term_query));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_page(path: &str, title: &str, content: &str, tags: Vec<&str>) -> WikiPage {
        let mut page = WikiPage::new(
            path.to_string(),
            title.to_string(),
            PageType::Topic,
            content.to_string(),
        );
        page.tags = tags.into_iter().map(|s| s.to_string()).collect();
        page
    }

    #[test]
    fn test_open_or_create_index() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyIndex::open(dir.path().to_path_buf());
        assert!(idx.is_ok());
        assert_eq!(idx.unwrap().doc_count(), 0);
    }

    #[test]
    fn test_upsert_and_search() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyIndex::open(dir.path().to_path_buf()).unwrap();

        idx.upsert(&make_page(
            "topics/rust",
            "Rust Programming",
            "Rust is a systems programming language focused on safety.",
            vec!["rust", "systems"],
        ))
        .unwrap();

        idx.upsert(&make_page(
            "topics/python",
            "Python Programming",
            "Python is a high-level scripting language.",
            vec!["python", "scripting"],
        ))
        .unwrap();

        let hits = idx.search("rust programming", 10).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "topics/rust");
    }

    #[test]
    fn test_search_by_type() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyIndex::open(dir.path().to_path_buf()).unwrap();

        let page = WikiPage::new(
            "entities/gasket".to_string(),
            "Gasket Project".to_string(),
            PageType::Entity,
            "A Rust agent framework.".to_string(),
        );
        idx.upsert(&page).unwrap();

        idx.upsert(&make_page(
            "topics/rust",
            "Rust Topic",
            "About Rust programming.",
            vec![],
        ))
        .unwrap();

        let entity_hits = idx.search_by_type("rust", PageType::Entity, 10).unwrap();
        assert_eq!(entity_hits.len(), 1);
        assert_eq!(entity_hits[0].path, "entities/gasket");
    }

    #[test]
    fn test_search_by_tags() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyIndex::open(dir.path().to_path_buf()).unwrap();

        idx.upsert(&make_page(
            "topics/rust",
            "Rust",
            "Rust language.",
            vec!["rust", "systems", "async"],
        ))
        .unwrap();

        idx.upsert(&make_page(
            "topics/python",
            "Python",
            "Python language.",
            vec!["python", "scripting"],
        ))
        .unwrap();

        let hits = idx
            .search_by_tags(&["rust".to_string(), "systems".to_string()], 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "topics/rust");
    }

    #[test]
    fn test_delete() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyIndex::open(dir.path().to_path_buf()).unwrap();

        idx.upsert(&make_page("topics/rust", "Rust", "Rust language.", vec![]))
            .unwrap();
        assert_eq!(idx.doc_count(), 1);

        idx.delete("topics/rust").unwrap();
        assert_eq!(idx.doc_count(), 0);
    }

    #[test]
    fn test_upsert_replaces() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyIndex::open(dir.path().to_path_buf()).unwrap();

        idx.upsert(&make_page("topics/rust", "Rust Old", "Old content.", vec![]))
            .unwrap();
        idx.upsert(&make_page(
            "topics/rust",
            "Rust New",
            "New content about Rust async.",
            vec![],
        ))
        .unwrap();

        // Should have 1 doc, not 2
        assert_eq!(idx.doc_count(), 1);

        let hits = idx.search("async", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Rust New");
    }

    #[test]
    fn test_rebuild() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyIndex::open(dir.path().to_path_buf()).unwrap();

        idx.upsert(&make_page("topics/rust", "Rust", "Rust.", vec![]))
            .unwrap();

        let pages = vec![
            make_page("topics/a", "A", "Content A.", vec![]),
            make_page("topics/b", "B", "Content B.", vec![]),
        ];
        idx.rebuild(&pages).unwrap();

        assert_eq!(idx.doc_count(), 2);
        // Old page should be gone
        let hits = idx.search("rust", 10).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn test_search_empty_index() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyIndex::open(dir.path().to_path_buf()).unwrap();

        let hits = idx.search("anything", 10).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn test_search_multi_word() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyIndex::open(dir.path().to_path_buf()).unwrap();

        idx.upsert(&make_page(
            "topics/rust-async",
            "Rust Async",
            "How async/await works in Rust with tokio runtime.",
            vec!["rust", "async"],
        ))
        .unwrap();

        idx.upsert(&make_page(
            "topics/rust-ownership",
            "Rust Ownership",
            "Ownership and borrowing in Rust.",
            vec!["rust", "ownership"],
        ))
        .unwrap();

        let hits = idx.search("async tokio", 10).unwrap();
        assert!(!hits.is_empty());
        // Rust Async should rank higher
        assert_eq!(hits[0].path, "topics/rust-async");
    }
}
