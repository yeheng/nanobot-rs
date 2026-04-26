//! Wiki search index trait and Tantivy implementation.
//!
//! Defines `PageSearchIndex` — an async trait for full-text search over wiki
//! pages. The Tantivy-based implementation lives here in storage; engine only
//! sees the trait.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
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

// ── Public types ───────────────────────────────────────────────

/// A search hit from the wiki index.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Wiki page path (e.g., "entities/projects/gasket").
    pub path: String,
    /// BM25 relevance score.
    pub score: f32,
    /// Page title (for display).
    pub title: String,
}

/// Minimal page data needed for search indexing.
///
/// Engine converts `WikiPage` to `IndexPage` before calling search methods.
/// This keeps the trait boundary clean — storage doesn't depend on engine types.
#[derive(Debug, Clone)]
pub struct IndexPage {
    pub path: String,
    pub title: String,
    pub content: String,
    /// Page type as string: "entity", "topic", "source", "sop".
    pub page_type: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub confidence: f64,
}

// ── Trait ──────────────────────────────────────────────────────

/// Async trait for wiki page search operations.
///
/// All methods are async and internally offload blocking Tantivy I/O to
/// `spawn_blocking`, so callers never block the Tokio runtime.
#[async_trait]
pub trait PageSearchIndex: Send + Sync {
    /// BM25 search across title and content fields.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>>;

    /// Search with page type filter.
    async fn search_by_type(&self, query: &str, page_type: &str, limit: usize)
        -> Result<Vec<SearchHit>>;

    /// Search by tags (exact match, OR semantics).
    async fn search_by_tags(&self, tags: &[String], limit: usize) -> Result<Vec<SearchHit>>;

    /// Upsert a page into the index.
    async fn upsert(&self, page: &IndexPage) -> Result<()>;

    /// Delete a page by path.
    async fn delete(&self, path: &str) -> Result<()>;

    /// Rebuild the index from a full set of pages.
    async fn rebuild(&self, pages: &[IndexPage]) -> Result<usize>;

    /// Get document count.
    fn doc_count(&self) -> u64;
}

// ── Tantivy schema fields ──────────────────────────────────────

/// Holds all Tantivy field handles for the wiki index.
#[derive(Clone)]
struct WikiFields {
    path: Field,
    title: Field,
    content: Field,
    page_type: Field,
    category: Field,
    tags: Field,
    confidence: Field,
}

impl WikiFields {
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

// ── Tantivy implementation ─────────────────────────────────────

/// Tantivy-backed full-text index for wiki pages.
///
/// Thread-safe: `Clone` shares the same underlying writer mutex.
#[derive(Clone)]
pub struct TantivyPageIndex {
    index: Index,
    reader: IndexReader,
    writer: Arc<Mutex<IndexWriter>>,
    fields: WikiFields,
}

impl TantivyPageIndex {
    /// Open or create the Tantivy index at the given directory.
    ///
    /// If the index is corrupted (e.g., truncated mmap files from unclean
    /// shutdown), the index directory is wiped and a fresh index is created.
    pub fn open(index_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&index_dir)?;

        match Self::try_open(&index_dir) {
            Ok(idx) => {
                info!("Opened Tantivy wiki index at {:?}", index_dir);
                Ok(idx)
            }
            Err(e) => {
                tracing::warn!(
                    "Tantivy index at {:?} is corrupted ({}), wiping and recreating",
                    index_dir,
                    e
                );
                // Wipe corrupted index files and retry.
                if index_dir.exists() {
                    std::fs::remove_dir_all(&index_dir)?;
                }
                std::fs::create_dir_all(&index_dir)?;

                let idx = Self::try_open(&index_dir)?;
                info!("Recreated Tantivy wiki index at {:?}", index_dir);
                Ok(idx)
            }
        }
    }

    fn try_open(index_dir: &PathBuf) -> Result<Self> {
        let (schema, fields) = WikiFields::build();

        let directory = MmapDirectory::open(index_dir)?;
        let index = Index::open_or_create(directory, schema)?;
        let reader = index.reader()?;
        let writer = index.writer(50_000_000)?;

        Ok(Self {
            index,
            reader,
            writer: Arc::new(Mutex::new(writer)),
            fields,
        })
    }

    // ── Sync operations (called via spawn_blocking) ──────────────

    fn upsert_sync(&self, page: &IndexPage) -> Result<()> {
        let mut writer = self.writer.lock().unwrap();

        let delete_term = Term::from_field_text(self.fields.path, &page.path);
        writer.delete_term(delete_term);

        let mut doc = TantivyDocument::new();
        doc.add_text(self.fields.path, &page.path);
        doc.add_text(self.fields.title, &page.title);
        doc.add_text(self.fields.content, &page.content);
        doc.add_text(self.fields.page_type, &page.page_type);

        if let Some(ref cat) = page.category {
            doc.add_text(self.fields.category, cat);
        }

        for tag in &page.tags {
            doc.add_text(self.fields.tags, tag);
        }

        doc.add_f64(self.fields.confidence, page.confidence);

        writer.add_document(doc)?;
        writer.commit()?;
        self.reader.reload()?;

        debug!("Tantivy upsert: '{}'", page.path);
        Ok(())
    }

    fn delete_sync(&self, path: &str) -> Result<()> {
        let mut writer = self.writer.lock().unwrap();
        let delete_term = Term::from_field_text(self.fields.path, path);
        writer.delete_term(delete_term);
        writer.commit()?;
        self.reader.reload()?;

        debug!("Tantivy delete: '{}'", path);
        Ok(())
    }

    fn search_sync(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let searcher = self.reader.searcher();

        let title_parser = QueryParser::for_index(&self.index, vec![self.fields.title]);
        let content_parser = QueryParser::for_index(&self.index, vec![self.fields.content]);

        let mut sub_queries: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();

        match title_parser.parse_query(query) {
            Ok(q) => sub_queries.push((Occur::Should, q)),
            Err(e) => {
                debug!("Title query parse failed, using term fallback: {}", e);
                Self::add_term_fallback(&self.fields, &mut sub_queries, query, self.fields.title);
            }
        }

        match content_parser.parse_query(query) {
            Ok(q) => sub_queries.push((Occur::Should, q)),
            Err(e) => {
                debug!("Content query parse failed, using term fallback: {}", e);
                Self::add_term_fallback(&self.fields, &mut sub_queries, query, self.fields.content);
            }
        }

        if sub_queries.is_empty() {
            return Ok(vec![]);
        }

        let combined = BooleanQuery::new(sub_queries);
        let top_docs = searcher.search(&combined, &TopDocs::with_limit(limit))?;

        Ok(Self::collect_hits(&searcher, &self.fields, &top_docs))
    }

    fn search_by_type_sync(
        &self,
        query: &str,
        page_type: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let searcher = self.reader.searcher();

        let type_term = Term::from_field_text(self.fields.page_type, page_type);
        let type_query: Box<dyn tantivy::query::Query> =
            Box::new(TermQuery::new(type_term, IndexRecordOption::Basic));

        let text_parser =
            QueryParser::for_index(&self.index, vec![self.fields.title, self.fields.content]);

        let text_query: Box<dyn tantivy::query::Query> = match text_parser.parse_query(query) {
            Ok(q) => q,
            Err(_) => {
                let term = Term::from_field_text(self.fields.title, query);
                Box::new(TermQuery::new(
                    term,
                    IndexRecordOption::WithFreqsAndPositions,
                ))
            }
        };

        let combined =
            BooleanQuery::new(vec![(Occur::Must, type_query), (Occur::Must, text_query)]);

        let top_docs = searcher.search(&combined, &TopDocs::with_limit(limit))?;
        Ok(Self::collect_hits(&searcher, &self.fields, &top_docs))
    }

    fn search_by_tags_sync(&self, tags: &[String], limit: usize) -> Result<Vec<SearchHit>> {
        if tags.is_empty() {
            return Ok(vec![]);
        }

        let searcher = self.reader.searcher();

        let tag_queries: Vec<(Occur, Box<dyn tantivy::query::Query>)> = tags
            .iter()
            .map(|tag| {
                let term = Term::from_field_text(self.fields.tags, tag);
                let query: Box<dyn tantivy::query::Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                (Occur::Should, query)
            })
            .collect();

        let combined = BooleanQuery::new(tag_queries);
        let top_docs = searcher.search(&combined, &TopDocs::with_limit(limit))?;
        Ok(Self::collect_hits(&searcher, &self.fields, &top_docs))
    }

    fn rebuild_sync(&self, pages: &[IndexPage]) -> Result<usize> {
        let mut writer = self.writer.lock().unwrap();

        writer.delete_all_documents()?;
        writer.commit()?;

        for page in pages {
            let mut doc = TantivyDocument::new();
            doc.add_text(self.fields.path, &page.path);
            doc.add_text(self.fields.title, &page.title);
            doc.add_text(self.fields.content, &page.content);
            doc.add_text(self.fields.page_type, &page.page_type);

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

    fn doc_count_sync(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    // ── Helpers ───────────────────────────────────────────────────

    fn collect_hits(
        searcher: &tantivy::Searcher,
        fields: &WikiFields,
        top_docs: &[(f32, tantivy::DocAddress)],
    ) -> Vec<SearchHit> {
        top_docs
            .iter()
            .filter_map(|&(score, doc_address)| {
                searcher
                    .doc::<TantivyDocument>(doc_address)
                    .ok()
                    .map(|doc| {
                        let path = doc
                            .get_first(fields.path)
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let title = doc
                            .get_first(fields.title)
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        SearchHit { path, score, title }
                    })
            })
            .collect()
    }

    fn add_term_fallback(
        _fields: &WikiFields,
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

// ── Async trait impl ───────────────────────────────────────────

/// Default timeout for blocking Tantivy operations.
const TANTIVY_TIMEOUT_SECS: u64 = 30;

/// Helper: run a closure in `spawn_blocking` with a timeout.
async fn with_timeout<F, T>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::time::timeout(
        std::time::Duration::from_secs(TANTIVY_TIMEOUT_SECS),
        tokio::task::spawn_blocking(f),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Tantivy operation timed out after {}s", TANTIVY_TIMEOUT_SECS))?
    .map_err(|e| anyhow::anyhow!("Tantivy spawn_blocking panicked: {}", e))?
}

#[async_trait]
impl PageSearchIndex for TantivyPageIndex {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let index = self.clone();
        let query = query.to_string();
        with_timeout(move || index.search_sync(&query, limit)).await
    }

    async fn search_by_type(
        &self,
        query: &str,
        page_type: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let index = self.clone();
        let query = query.to_string();
        let page_type = page_type.to_string();
        with_timeout(move || index.search_by_type_sync(&query, &page_type, limit)).await
    }

    async fn search_by_tags(&self, tags: &[String], limit: usize) -> Result<Vec<SearchHit>> {
        let index = self.clone();
        let tags = tags.to_vec();
        with_timeout(move || index.search_by_tags_sync(&tags, limit)).await
    }

    async fn upsert(&self, page: &IndexPage) -> Result<()> {
        let index = self.clone();
        let page = page.clone();
        with_timeout(move || index.upsert_sync(&page)).await
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let index = self.clone();
        let path = path.to_string();
        with_timeout(move || index.delete_sync(&path)).await
    }

    async fn rebuild(&self, pages: &[IndexPage]) -> Result<usize> {
        let index = self.clone();
        let pages = pages.to_vec();
        with_timeout(move || index.rebuild_sync(&pages)).await
    }

    fn doc_count(&self) -> u64 {
        self.doc_count_sync()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_page(path: &str, title: &str, content: &str, tags: Vec<&str>) -> IndexPage {
        IndexPage {
            path: path.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            page_type: "topic".to_string(),
            category: None,
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
            confidence: 1.0,
        }
    }

    #[tokio::test]
    async fn test_open_or_create_index() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyPageIndex::open(dir.path().to_path_buf()).unwrap();
        assert_eq!(idx.doc_count(), 0);
    }

    #[tokio::test]
    async fn test_upsert_and_search() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyPageIndex::open(dir.path().to_path_buf()).unwrap();

        idx.upsert(&make_page(
            "topics/rust",
            "Rust Programming",
            "Rust is a systems programming language focused on safety.",
            vec!["rust", "systems"],
        ))
        .await
        .unwrap();

        idx.upsert(&make_page(
            "topics/python",
            "Python Programming",
            "Python is a high-level scripting language.",
            vec!["python", "scripting"],
        ))
        .await
        .unwrap();

        let hits = idx.search("rust programming", 10).await.unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "topics/rust");
    }

    #[tokio::test]
    async fn test_search_by_type() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyPageIndex::open(dir.path().to_path_buf()).unwrap();

        let mut page = make_page("entities/gasket", "Gasket Project", "A Rust agent framework.", vec![]);
        page.page_type = "entity".to_string();
        idx.upsert(&page).await.unwrap();

        idx.upsert(&make_page("topics/rust", "Rust Topic", "About Rust programming.", vec![]))
            .await
            .unwrap();

        let entity_hits = idx.search_by_type("rust", "entity", 10).await.unwrap();
        assert_eq!(entity_hits.len(), 1);
        assert_eq!(entity_hits[0].path, "entities/gasket");
    }

    #[tokio::test]
    async fn test_search_by_tags() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyPageIndex::open(dir.path().to_path_buf()).unwrap();

        idx.upsert(&make_page("topics/rust", "Rust", "Rust language.", vec!["rust", "systems", "async"]))
            .await
            .unwrap();

        idx.upsert(&make_page("topics/python", "Python", "Python language.", vec!["python", "scripting"]))
            .await
            .unwrap();

        let hits = idx
            .search_by_tags(&["rust".to_string(), "systems".to_string()], 10)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "topics/rust");
    }

    #[tokio::test]
    async fn test_delete() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyPageIndex::open(dir.path().to_path_buf()).unwrap();

        idx.upsert(&make_page("topics/rust", "Rust", "Rust language.", vec![]))
            .await
            .unwrap();
        assert_eq!(idx.doc_count(), 1);

        idx.delete("topics/rust").await.unwrap();
        assert_eq!(idx.doc_count(), 0);
    }

    #[tokio::test]
    async fn test_upsert_replaces() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyPageIndex::open(dir.path().to_path_buf()).unwrap();

        idx.upsert(&make_page("topics/rust", "Rust Old", "Old content.", vec![]))
            .await
            .unwrap();
        idx.upsert(&make_page(
            "topics/rust",
            "Rust New",
            "New content about Rust async.",
            vec![],
        ))
        .await
        .unwrap();

        assert_eq!(idx.doc_count(), 1);

        let hits = idx.search("async", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Rust New");
    }

    #[tokio::test]
    async fn test_rebuild() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyPageIndex::open(dir.path().to_path_buf()).unwrap();

        idx.upsert(&make_page("topics/rust", "Rust", "Rust.", vec![]))
            .await
            .unwrap();

        let pages = vec![
            make_page("topics/a", "A", "Content A.", vec![]),
            make_page("topics/b", "B", "Content B.", vec![]),
        ];
        idx.rebuild(&pages).await.unwrap();

        assert_eq!(idx.doc_count(), 2);
        let hits = idx.search("rust", 10).await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn test_search_empty_index() {
        let dir = TempDir::new().unwrap();
        let idx = TantivyPageIndex::open(dir.path().to_path_buf()).unwrap();

        let hits = idx.search("anything", 10).await.unwrap();
        assert!(hits.is_empty());
    }
}
