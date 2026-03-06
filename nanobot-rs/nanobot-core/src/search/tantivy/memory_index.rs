//! Memory index for `~/.nanobot/memory/*.md` files.
//!
//! Provides a read-write separated design:
//! - `MemoryIndexReader`: lock-free, `Clone + Send + Sync` — safe for concurrent search
//! - `MemoryIndexWriter`: requires `&mut self` — wrap in `Mutex` for shared access
//!
//! Both structs share the same underlying `IndexReader` via clone.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use tantivy::{
    collector::TopDocs,
    directory::MmapDirectory,
    query::{BooleanQuery, FuzzyTermQuery, Occur, Query, TermQuery},
    schema::{
        Field, IndexRecordOption, Schema, SchemaBuilder, TextFieldIndexing, TextOptions, Value,
        STORED, STRING,
    },
    snippet::SnippetGenerator,
    Index, IndexReader, IndexWriter, TantivyDocument, Term,
};
use tracing::{debug, info, warn};

use super::TantivyError;
use crate::search::{SearchQuery, SearchResult, SortOrder};

/// Memory document schema fields.
#[derive(Clone, Copy)]
struct MemorySchema {
    id: Field,
    content: Field,
    title: Field,
    tags: Field,
    file_path: Field,
    modified_at: Field,
    created_at: Field,
}

impl MemorySchema {
    fn build() -> (Schema, Self) {
        let mut builder = SchemaBuilder::new();

        let text_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer("default")
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();

        let id = builder.add_text_field("id", STRING | STORED);
        let content = builder.add_text_field("content", text_options.clone());
        let title = builder.add_text_field("title", text_options);
        let tags = builder.add_text_field("tags", STRING | STORED);
        let file_path = builder.add_text_field("file_path", STORED);
        let modified_at = builder.add_i64_field("modified_at", STORED);
        let created_at = builder.add_i64_field("created_at", STORED);

        (
            builder.build(),
            Self {
                id,
                content,
                title,
                tags,
                file_path,
                modified_at,
                created_at,
            },
        )
    }
}

/// Thread-safe, lock-free reader for the memory index.
///
/// Tantivy's `IndexReader` is `Clone + Send + Sync` (backed by `ArcSwap`),
/// so this struct needs no external synchronization.
pub struct MemoryIndexReader {
    fields: MemorySchema,
    reader: IndexReader,
    memory_dir: PathBuf,
}

impl MemoryIndexReader {
    /// Get the memory directory being indexed.
    pub fn memory_dir(&self) -> &Path {
        &self.memory_dir
    }

    /// Search the memory index.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, TantivyError> {
        let searcher = self.reader.searcher();
        let tantivy_query = self.build_query(query)?;

        let mut results = Vec::new();

        // Execute search
        let top_docs = searcher.search(&tantivy_query, &TopDocs::with_limit(query.limit * 2))?;

        // Build snippet generator for highlighting
        let snippet_generator = if let Some(ref _text) = query.text {
            Some(SnippetGenerator::create(
                &searcher,
                &tantivy_query,
                self.fields.content,
            )?)
        } else {
            None
        };

        for (score, doc_address) in top_docs {
            if results.len() >= query.limit {
                break;
            }

            if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                let id = doc
                    .get_first(self.fields.id)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let content = doc
                    .get_first(self.fields.content)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let title = doc
                    .get_first(self.fields.title)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let tags: Vec<String> = doc
                    .get_all(self.fields.tags)
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();

                let modified_ts = doc
                    .get_first(self.fields.modified_at)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                let timestamp = DateTime::from_timestamp(modified_ts, 0)
                    .map(|dt| dt.with_timezone(&Utc).to_rfc3339())
                    .unwrap_or_default();

                // Apply tag filter if specified
                if !query.tags.is_empty() {
                    let has_all_tags = query.tags.iter().all(|t| tags.contains(t));
                    if !has_all_tags {
                        continue;
                    }
                }

                // Create highlighted snippet
                let highlight = if let Some(ref gen) = snippet_generator {
                    let snippet = gen.snippet(&content);
                    let highlighted = snippet.to_html();
                    if highlighted.contains("<b>") {
                        Some(crate::search::HighlightedText::with_marker(
                            highlighted,
                            "<b>",
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let display_content = if content.len() > 500 {
                    format!("{}...\n\n[Title: {}]", &content[..500], title)
                } else {
                    format!("{}\n\n[Title: {}]", content, title)
                };

                results.push(SearchResult {
                    id,
                    content: display_content,
                    score,
                    tags,
                    source: Some("memory".to_string()),
                    timestamp: Some(timestamp),
                    highlight,
                    metadata: {
                        let mut meta = serde_json::Map::new();
                        meta.insert("title".to_string(), serde_json::Value::String(title));
                        meta
                    },
                });
            }
        }

        // Sort by date if requested
        if query.sort == SortOrder::Date {
            results.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            results.truncate(query.limit);
        }

        Ok(results)
    }

    /// Get the number of documents in the index.
    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    /// Get all indexed document IDs with their modification times.
    pub fn get_indexed_documents(
        &self,
    ) -> Result<std::collections::HashMap<String, DateTime<Utc>>, TantivyError> {
        use tantivy::collector::DocSetCollector;

        let searcher = self.reader.searcher();
        let query = tantivy::query::AllQuery;

        let doc_addresses = searcher.search(&query, &DocSetCollector)?;
        let mut docs = std::collections::HashMap::new();

        for doc_address in doc_addresses {
            if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                let id = doc
                    .get_first(self.fields.id)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let modified_ts = doc
                    .get_first(self.fields.modified_at)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                let modified_at = DateTime::from_timestamp(modified_ts, 0)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(Utc::now);

                docs.insert(id, modified_at);
            }
        }

        Ok(docs)
    }

    /// Build a Tantivy query from SearchQuery.
    fn build_query(&self, query: &SearchQuery) -> Result<Box<dyn Query>, TantivyError> {
        let mut queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        // Text search
        if let Some(ref text) = query.text {
            let text_query: Box<dyn Query> = Box::new(BooleanQuery::new_multiterms_query(vec![
                Term::from_field_text(self.fields.content, text),
                Term::from_field_text(self.fields.title, text),
            ]));
            queries.push((Occur::Should, text_query));
        }

        // Boolean query
        if let Some(ref bq) = query.boolean {
            for term in &bq.must {
                let term_query: Box<dyn Query> = Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.content, term),
                    IndexRecordOption::WithFreqsAndPositions,
                ));
                queries.push((Occur::Must, term_query));
            }

            for term in &bq.should {
                let term_query: Box<dyn Query> = Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.content, term),
                    IndexRecordOption::WithFreqsAndPositions,
                ));
                queries.push((Occur::Should, term_query));
            }

            for term in &bq.not {
                let term_query: Box<dyn Query> = Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.content, term),
                    IndexRecordOption::WithFreqsAndPositions,
                ));
                queries.push((Occur::MustNot, term_query));
            }
        }

        // Fuzzy query
        if let Some(ref fq) = query.fuzzy {
            let term = Term::from_field_text(self.fields.content, &fq.text);
            let fuzzy_query: Box<dyn Query> =
                Box::new(FuzzyTermQuery::new(term, fq.distance, fq.prefix));
            queries.push((Occur::Should, fuzzy_query));
        }

        // If no queries, match all
        if queries.is_empty() {
            return Ok(Box::new(tantivy::query::AllQuery));
        }

        Ok(Box::new(BooleanQuery::new(queries)))
    }
}

/// Writer for the memory index — requires `&mut self` for all mutating operations.
///
/// Wrap in `tokio::sync::Mutex` (or similar) when shared across async tasks.
pub struct MemoryIndexWriter {
    fields: MemorySchema,
    index: Index,
    writer: Option<IndexWriter>,
    reader: IndexReader,
    memory_dir: PathBuf,
}

impl MemoryIndexWriter {
    /// Ensure writer is available.
    fn ensure_writer(&mut self) -> Result<&mut IndexWriter, TantivyError> {
        if self.writer.is_none() {
            let writer = self.index.writer(50_000_000)?;
            self.writer = Some(writer);
        }
        Ok(self.writer.as_mut().unwrap())
    }

    /// Add or update a memory document from file content.
    pub fn index_document(
        &mut self,
        id: &str,
        content: &str,
        title: &str,
        tags: &[String],
        file_path: &Path,
        modified_at: DateTime<Utc>,
    ) -> Result<(), TantivyError> {
        let id_field = self.fields.id;
        let content_field = self.fields.content;
        let title_field = self.fields.title;
        let tags_field = self.fields.tags;
        let file_path_field = self.fields.file_path;
        let modified_at_field = self.fields.modified_at;
        let created_at_field = self.fields.created_at;

        let writer = self.ensure_writer()?;

        // Delete existing document with same ID
        let delete_term = Term::from_field_text(id_field, id);
        writer.delete_term(delete_term);

        // Create new document
        let mut doc = TantivyDocument::new();
        doc.add_text(id_field, id);
        doc.add_text(content_field, content);
        doc.add_text(title_field, title);
        for tag in tags {
            doc.add_text(tags_field, tag);
        }
        doc.add_text(file_path_field, file_path.to_string_lossy());
        doc.add_i64(modified_at_field, modified_at.timestamp());
        doc.add_i64(created_at_field, modified_at.timestamp());

        writer.add_document(doc)?;
        debug!("Indexed memory document: {}", id);
        Ok(())
    }

    /// Delete a document by ID.
    pub fn delete_document(&mut self, id: &str) -> Result<(), TantivyError> {
        let id_field = self.fields.id;
        let writer = self.ensure_writer()?;
        let delete_term = Term::from_field_text(id_field, id);
        writer.delete_term(delete_term);
        debug!("Deleted memory document from index: {}", id);
        Ok(())
    }

    /// Commit pending changes and reload the reader.
    pub fn commit(&mut self) -> Result<(), TantivyError> {
        if let Some(writer) = &mut self.writer {
            writer.commit()?;
            self.reader.reload()?;
            info!("Memory index committed");
        }
        Ok(())
    }

    /// Rebuild the entire index from memory directory.
    pub async fn rebuild(&mut self) -> Result<usize, TantivyError> {
        use tokio::fs;

        if !self.memory_dir.exists() {
            warn!("Memory directory does not exist: {:?}", self.memory_dir);
            return Ok(0);
        }

        // Clear existing index
        let writer = self.ensure_writer()?;
        writer.delete_all_documents()?;

        let mut count = 0;
        let mut read_dir = fs::read_dir(&self.memory_dir).await.map_err(|e| {
            TantivyError::OperationError(format!("Failed to read memory directory: {}", e))
        })?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| TantivyError::OperationError(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                if let Ok(content) = fs::read_to_string(&path).await {
                    let id = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    let title = extract_title(&content).unwrap_or_else(|| id.clone());
                    let tags = extract_tags(&content);

                    let metadata = fs::metadata(&path).await.ok();
                    let modified_at = metadata
                        .and_then(|m| m.modified().ok())
                        .map(DateTime::<Utc>::from)
                        .unwrap_or_else(Utc::now);

                    self.index_document(&id, &content, &title, &tags, &path, modified_at)?;
                    count += 1;

                    if count % 100 == 0 {
                        info!("Rebuilding memory index: {} documents", count);
                    }
                }
            }
        }

        self.commit()?;
        info!("Memory index rebuilt: {} documents", count);
        Ok(count)
    }

    /// Incremental update: only index new or modified files, remove deleted files.
    pub async fn incremental_update(&mut self) -> Result<IndexUpdateStats, TantivyError> {
        use tokio::fs;

        if !self.memory_dir.exists() {
            warn!("Memory directory does not exist: {:?}", self.memory_dir);
            return Ok(IndexUpdateStats::default());
        }

        let mut stats = IndexUpdateStats::default();

        // Collect all file IDs currently on disk with their modification times
        let mut disk_files: std::collections::HashMap<String, (PathBuf, DateTime<Utc>)> =
            std::collections::HashMap::new();

        let mut read_dir = fs::read_dir(&self.memory_dir).await.map_err(|e| {
            TantivyError::OperationError(format!("Failed to read memory directory: {}", e))
        })?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| TantivyError::OperationError(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                let metadata = fs::metadata(&path).await.ok();
                let modified_at = metadata
                    .and_then(|m| m.modified().ok())
                    .map(DateTime::<Utc>::from)
                    .unwrap_or_else(Utc::now);

                disk_files.insert(id, (path, modified_at));
            }
        }

        // Get indexed document IDs and their modification times
        let indexed_docs = self.get_indexed_documents()?;

        // Remove documents that no longer exist on disk
        for id in indexed_docs.keys() {
            if !disk_files.contains_key(id) {
                self.delete_document(id)?;
                stats.removed += 1;
                debug!("Removed deleted file from index: {}", id);
            }
        }

        // Add or update documents
        for (id, (path, modified_at)) in disk_files {
            let needs_update = match indexed_docs.get(&id) {
                Some(indexed_time) => modified_at > *indexed_time,
                None => true,
            };

            if needs_update {
                if let Ok(content) = fs::read_to_string(&path).await {
                    let title = extract_title(&content).unwrap_or_else(|| id.clone());
                    let tags = extract_tags(&content);
                    self.index_document(&id, &content, &title, &tags, &path, modified_at)?;

                    if indexed_docs.contains_key(&id) {
                        stats.updated += 1;
                        debug!("Updated file in index: {}", id);
                    } else {
                        stats.added += 1;
                        debug!("Added new file to index: {}", id);
                    }
                }
            }
        }

        if stats.added > 0 || stats.updated > 0 || stats.removed > 0 {
            self.commit()?;
        }

        info!(
            "Memory index incremental update: {} added, {} updated, {} removed",
            stats.added, stats.updated, stats.removed
        );
        Ok(stats)
    }

    /// Get all indexed document IDs with their modification times.
    fn get_indexed_documents(
        &self,
    ) -> Result<std::collections::HashMap<String, DateTime<Utc>>, TantivyError> {
        use tantivy::collector::DocSetCollector;

        let searcher = self.reader.searcher();
        let query = tantivy::query::AllQuery;

        let doc_addresses = searcher.search(&query, &DocSetCollector)?;
        let mut docs = std::collections::HashMap::new();

        for doc_address in doc_addresses {
            if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                let id = doc
                    .get_first(self.fields.id)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let modified_ts = doc
                    .get_first(self.fields.modified_at)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                let modified_at = DateTime::from_timestamp(modified_ts, 0)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(Utc::now);

                docs.insert(id, modified_at);
            }
        }

        Ok(docs)
    }
}

/// Open (or create) a memory index, returning a paired reader and writer.
///
/// Both share the same underlying `IndexReader`. When the writer commits,
/// Tantivy's `OnCommitWithDelay` policy auto-reloads all clones.
pub fn open_memory_index(
    index_path: impl AsRef<Path>,
    memory_dir: impl AsRef<Path>,
) -> Result<(MemoryIndexReader, MemoryIndexWriter), TantivyError> {
    let index_path = index_path.as_ref().to_path_buf();
    let memory_dir = memory_dir.as_ref().to_path_buf();
    let (schema, fields) = MemorySchema::build();

    if !index_path.exists() {
        std::fs::create_dir_all(&index_path)?;
        debug!("Created memory index directory: {:?}", index_path);
    }

    let directory = MmapDirectory::open(&index_path)
        .map_err(|e| TantivyError::OpenError(format!("Failed to open directory: {}", e)))?;

    let index = Index::open_or_create(directory, schema)?;
    let reader = index.reader()?;

    let reader_clone = reader.clone();

    Ok((
        MemoryIndexReader {
            fields,
            reader: reader_clone,
            memory_dir: memory_dir.clone(),
        },
        MemoryIndexWriter {
            fields,
            index,
            writer: None,
            reader,
            memory_dir,
        },
    ))
}

/// Statistics for index update operations.
#[derive(Debug, Default, Clone, Copy)]
pub struct IndexUpdateStats {
    /// Number of new documents added.
    pub added: usize,
    /// Number of existing documents updated.
    pub updated: usize,
    /// Number of documents removed.
    pub removed: usize,
}

/// Extract title from markdown content (first heading).
fn extract_title(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(stripped) = trimmed.strip_prefix("# ") {
            return Some(stripped.to_string());
        }
    }
    None
}

/// Extract tags from markdown content (#tag patterns).
fn extract_tags(content: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for line in content.lines() {
        for word in line.split_whitespace() {
            if word.starts_with('#') && word.len() > 1 {
                let tag = word[1..]
                    .trim_end_matches(|c: char| !c.is_alphanumeric())
                    .to_string();
                if !tag.is_empty() && !tags.contains(&tag) {
                    tags.push(tag);
                }
            }
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_title() {
        let content = "# My Project\n\nSome content";
        assert_eq!(extract_title(content), Some("My Project".to_string()));
    }

    #[test]
    fn test_extract_tags() {
        let content = "This has #rust and #python tags";
        let tags = extract_tags(content);
        assert_eq!(tags, vec!["rust", "python"]);
    }
}
