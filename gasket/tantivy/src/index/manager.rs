//! Index manager for managing multiple indexes.
//!
//! ## Simplified CLI Architecture
//!
//! This module uses a simple synchronous design suitable for CLI tools:
//!
//! 1. **HashMap for index registry**: Uses `HashMap<String, IndexState>` since CLI
//!    processes one command at a time.
//!
//! 2. **File locking for process safety**: Uses `IndexLock` to prevent multiple
//!    CLI processes from accessing the same index simultaneously.
//!
//! 3. **Synchronous operations**: All operations execute synchronously and
//!    return `Result<()>` directly.

use std::collections::HashMap;
use std::ops::Bound;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tantivy::{
    directory::MmapDirectory,
    schema::{Field, Schema, SchemaBuilder, TextFieldIndexing, TextOptions, Value, STORED, STRING},
    Index, IndexReader, IndexWriter, TantivyDocument, Term,
};
use tracing::{debug, info, warn};

use super::document::{BatchDocumentInput, Document};
use super::lock::IndexLock;
use super::schema::{FieldDef, FieldType, IndexConfig, IndexSchema};
use super::search::{SearchQuery, SearchResult};
use crate::{Error, Result};

/// Internal index state.
///
/// Each index has its own state with all fields stored directly
/// (no locking needed for single-threaded CLI).
struct IndexState {
    // Immutable fields (set at creation, never changed)
    schema: IndexSchema,
    config: IndexConfig,
    _tantivy_schema: Schema,
    tantivy_index: Index,
    reader: IndexReader,
    field_map: HashMap<String, Field>,
    id_field: Field,
    expires_at_field: Field,

    // Mutable field - no lock needed for single-threaded CLI
    writer: Option<IndexWriter>,
}

impl IndexState {
    /// Ensure the writer is initialized.
    fn ensure_writer(&mut self) -> Result<()> {
        if self.writer.is_none() {
            self.writer = Some(self.tantivy_index.writer(50_000_000)?);
        }
        Ok(())
    }
}

/// Index manager handling multiple indexes.
///
/// Uses `HashMap` for simple single-threaded access.
/// Uses file locking for process-level safety.
pub struct IndexManager {
    base_path: PathBuf,
    /// Index registry (no concurrency primitives needed for CLI).
    indexes: HashMap<String, IndexState>,
}

impl IndexManager {
    /// Create a new index manager.
    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            indexes: HashMap::new(),
        }
    }

    /// Get the base path for index storage.
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }

    /// Get the path to a specific index directory.
    pub fn index_path(&self, name: &str) -> PathBuf {
        self.base_path.join("indexes").join(name)
    }

    /// Get the indexes directory path.
    pub fn index_dir(&self) -> PathBuf {
        self.base_path.join("indexes")
    }

    /// Acquire an exclusive lock for an index.
    pub fn acquire_index_lock(&self, index_name: &str) -> Result<IndexLock> {
        let index_path = self.index_path(index_name);
        IndexLock::acquire(&index_path)
    }

    /// Unload an index from memory.
    /// The index files remain on disk.
    pub fn unload_index(&mut self, name: &str) -> Result<()> {
        if self.indexes.remove(name).is_some() {
            info!("Unloaded index: {}", name);
        }
        Ok(())
    }

    /// Load an index by name (public wrapper for the private load_index).
    pub fn load_index_by_name(&mut self, name: &str) -> Result<()> {
        let metadata_path = self.index_path(name).join("metadata.json");
        if !metadata_path.exists() {
            return Err(Error::IndexNotFound(name.to_string()));
        }
        self.load_index(name, &metadata_path)
    }
}

/// Index statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    /// Index name.
    pub name: String,
    /// Total document count.
    pub doc_count: u64,
    /// Index size on disk in bytes.
    pub size_bytes: u64,
    /// Number of segments.
    pub segment_count: usize,
    /// Deleted documents count (not yet compacted).
    pub deleted_count: u64,
    /// Last modified timestamp.
    pub last_modified: Option<DateTime<Utc>>,
    /// Health status.
    pub health: IndexHealth,
}

/// Index health status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IndexHealth {
    Healthy,
    NeedsCompaction,
    Warning,
    Error,
}

impl IndexManager {
    /// Create a new index.
    pub fn create_index(
        &mut self,
        name: &str,
        fields: Vec<FieldDef>,
        config: Option<IndexConfig>,
    ) -> Result<IndexSchema> {
        // Check if already exists
        if self.indexes.contains_key(name) {
            return Err(Error::IndexAlreadyExists(name.to_string()));
        }

        let index_path = self.base_path.join("indexes").join(name);
        std::fs::create_dir_all(&index_path)?;

        let schema = IndexSchema::new(name, fields);
        let config = config.unwrap_or_default();

        let (tantivy_schema, field_map, id_field, expires_at_field) =
            build_tantivy_schema(&schema)?;

        let directory = MmapDirectory::open(&index_path)
            .map_err(|e| Error::PathError(index_path.clone(), e.to_string()))?;
        let tantivy_index = Index::open_or_create(directory, tantivy_schema.clone())?;
        let reader = tantivy_index.reader()?;

        // Save metadata
        let metadata = IndexMetadata {
            schema: schema.clone(),
            config: config.clone(),
        };
        let metadata_path = index_path.join("metadata.json");
        let metadata_json = serde_json::to_string_pretty(&metadata)?;
        std::fs::write(&metadata_path, metadata_json)?;

        let state = IndexState {
            schema: schema.clone(),
            config,
            _tantivy_schema: tantivy_schema,
            tantivy_index,
            reader,
            writer: None,
            field_map,
            id_field,
            expires_at_field,
        };

        self.indexes.insert(name.to_string(), state);

        info!("Created index: {}", name);
        Ok(schema)
    }

    /// Drop an index.
    pub fn drop_index(&mut self, name: &str) -> Result<()> {
        let _lock = self.acquire_index_lock(name)?;

        // Remove from memory
        self.indexes
            .remove(name)
            .ok_or_else(|| Error::IndexNotFound(name.to_string()))?;

        // Delete index directory
        let index_path = self.index_path(name);
        if index_path.exists() {
            std::fs::remove_dir_all(&index_path)?;
        }

        info!("Dropped index: {}", name);
        Ok(())
    }

    /// List all indexes.
    pub fn list_indexes(&self) -> Vec<String> {
        self.indexes.keys().cloned().collect()
    }

    /// Get index schema.
    pub fn get_schema(&self, name: &str) -> Result<Option<IndexSchema>> {
        Ok(self.indexes.get(name).map(|s| s.schema.clone()))
    }

    /// Get index config.
    pub fn get_config(&self, name: &str) -> Result<Option<IndexConfig>> {
        Ok(self.indexes.get(name).map(|s| s.config.clone()))
    }

    /// Add a document to an index.
    pub fn add_document(&mut self, index_name: &str, document: Document) -> Result<()> {
        let _lock = self.acquire_index_lock(index_name)?;

        let state = self
            .indexes
            .get_mut(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        let id = document.id.clone();
        let expires_at_ts = document
            .expires_at
            .map(|t| t.timestamp())
            .unwrap_or(i64::MAX);
        let field_values: Vec<(String, serde_json::Value)> = document.fields.into_iter().collect();

        state.ensure_writer()?;

        let writer = state.writer.as_mut().ok_or(Error::WriterNotInitialized)?;

        // Delete existing document with same ID
        let delete_term = Term::from_field_text(state.id_field, &id);
        writer.delete_term(delete_term);

        // Build new document
        let mut doc = TantivyDocument::new();
        doc.add_text(state.id_field, &id);
        doc.add_i64(state.expires_at_field, expires_at_ts);

        for (field_name, value) in field_values {
            if let Some(tantivy_field) = state.field_map.get(&field_name) {
                let _ = add_field_value(&mut doc, *tantivy_field, &value);
            }
        }

        writer.add_document(doc)?;
        debug!("Added document {} to index {}", id, index_name);

        Ok(())
    }

    /// Delete a document from an index.
    pub fn delete_document(&mut self, index_name: &str, doc_id: &str) -> Result<()> {
        let _lock = self.acquire_index_lock(index_name)?;

        let state = self
            .indexes
            .get_mut(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        state.ensure_writer()?;

        let writer = state.writer.as_mut().ok_or(Error::WriterNotInitialized)?;

        let delete_term = Term::from_field_text(state.id_field, doc_id);
        writer.delete_term(delete_term);
        debug!("Deleted document {} from index {}", doc_id, index_name);

        Ok(())
    }

    /// Commit changes to an index.
    pub fn commit(&mut self, index_name: &str) -> Result<()> {
        let _lock = self.acquire_index_lock(index_name)?;

        let state = self
            .indexes
            .get_mut(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        state.ensure_writer()?;

        let writer = state.writer.as_mut().ok_or(Error::WriterNotInitialized)?;

        writer.commit()?;
        state.reader.reload()?;
        info!("Committed index {}", index_name);

        Ok(())
    }

    /// Search an index.
    pub fn search(&self, index_name: &str, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let state = self
            .indexes
            .get(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        let searcher = state.reader.searcher();
        let tantivy_query = build_tantivy_query(state, query)?;

        let top_docs = searcher.search(
            &tantivy_query,
            &tantivy::collector::TopDocs::with_limit(query.limit + query.offset),
        )?;

        let mut results = Vec::new();
        let id_field = state.id_field;
        let field_map = &state.field_map;

        // Create snippet generator if highlighting is requested
        let snippet_generator = if let Some(ref highlight_config) = query.highlight {
            if let Some(ref query_text) = query.text {
                Some(create_snippet_generator(
                    state,
                    query_text,
                    highlight_config,
                )?)
            } else {
                None
            }
        } else {
            None
        };

        for (score, doc_address) in top_docs.into_iter().skip(query.offset) {
            if results.len() >= query.limit {
                break;
            }

            if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                let id = doc
                    .get_first(id_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let mut fields = serde_json::Map::new();
                for (field_name, tantivy_field) in field_map {
                    if let Some(value) = doc.get_first(*tantivy_field) {
                        let json_value = tantivy_value_to_json(value);
                        fields.insert(field_name.clone(), json_value);
                    }
                }

                // Generate highlights if requested
                let (highlights, legacy_highlight) = if let Some(ref gen) = snippet_generator {
                    generate_highlights(&doc, gen, query.highlight.as_ref().unwrap())
                } else {
                    (None, None)
                };

                results.push(SearchResult {
                    id,
                    fields,
                    score,
                    highlights,
                    highlight: legacy_highlight,
                });
            }
        }

        Ok(results)
    }

    /// Get index statistics.
    pub fn get_stats(&self, index_name: &str) -> Result<IndexStats> {
        let state = self
            .indexes
            .get(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        let searcher = state.reader.searcher();
        let segment_readers = searcher.segment_readers();

        let doc_count = searcher.num_docs();
        let segment_count = segment_readers.len();

        let deleted_count: u64 = segment_readers
            .iter()
            .map(|r| r.num_deleted_docs() as u64)
            .sum();

        let index_path = self.index_path(index_name);
        let size_bytes = calculate_dir_size(&index_path)?;

        let health = if deleted_count as f32 / (doc_count as f32 + 1.0) > 0.2 {
            IndexHealth::NeedsCompaction
        } else {
            IndexHealth::Healthy
        };

        Ok(IndexStats {
            name: index_name.to_string(),
            doc_count,
            size_bytes,
            segment_count,
            deleted_count,
            last_modified: None,
            health,
        })
    }

    /// Compact an index.
    ///
    /// This commits any pending changes, waits for merging threads, and recreates the writer.
    pub fn compact(&mut self, index_name: &str) -> Result<()> {
        let _lock = self.acquire_index_lock(index_name)?;

        let state = self
            .indexes
            .get_mut(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        // Commit and wait for merges
        {
            let writer = state.writer.as_mut();
            if let Some(writer) = writer {
                writer.commit()?;
            }
        }

        // Take and wait for merging threads
        if let Some(writer) = state.writer.take() {
            if let Err(e) = writer.wait_merging_threads() {
                warn!("Failed to wait for merging threads: {}", e);
            }
        }

        // Recreate writer
        state.writer = Some(state.tantivy_index.writer(50_000_000)?);
        state.reader.reload()?;

        info!("Compacted index {}", index_name);
        Ok(())
    }

    /// List all documents in an index (with pagination).
    pub fn list_documents(
        &self,
        index_name: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Document>> {
        let state = self
            .indexes
            .get(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        let searcher = state.reader.searcher();
        let tantivy_query = tantivy::query::AllQuery;

        let top_docs = searcher.search(
            &tantivy_query,
            &tantivy::collector::TopDocs::with_limit(limit + offset),
        )?;

        let mut documents = Vec::new();
        let id_field = state.id_field;
        let expires_at_field = state.expires_at_field;
        let field_map = &state.field_map;

        for (_, doc_address) in top_docs.into_iter().skip(offset) {
            if documents.len() >= limit {
                break;
            }

            if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                let id = doc
                    .get_first(id_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let expires_at = doc
                    .get_first(expires_at_field)
                    .and_then(|v| v.as_i64())
                    .map(|ts| chrono::DateTime::from_timestamp(ts, 0).unwrap_or_else(Utc::now));

                let mut fields = serde_json::Map::new();
                for (field_name, tantivy_field) in field_map {
                    if let Some(value) = doc.get_first(*tantivy_field) {
                        let json_value = tantivy_value_to_json(value);
                        fields.insert(field_name.clone(), json_value);
                    }
                }

                documents.push(Document {
                    id,
                    fields,
                    expires_at,
                });
            }
        }

        Ok(documents)
    }

    /// Load existing indexes from disk.
    pub fn load_indexes(&mut self) -> Result<()> {
        let indexes_path = self.base_path.join("indexes");
        if !indexes_path.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(&indexes_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    let metadata_path = path.join("metadata.json");
                    if metadata_path.exists() {
                        match self.load_index(name, &metadata_path) {
                            Ok(_) => info!("Loaded index: {}", name),
                            Err(e) => warn!("Failed to load index {}: {}", name, e),
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn load_index(&mut self, name: &str, metadata_path: &Path) -> Result<()> {
        let metadata_json = std::fs::read_to_string(metadata_path)?;
        let metadata: IndexMetadata = serde_json::from_str(&metadata_json)?;

        let index_path = self.index_path(name);
        let (tantivy_schema, field_map, id_field, expires_at_field) =
            build_tantivy_schema(&metadata.schema)?;

        let directory = MmapDirectory::open(&index_path)
            .map_err(|e| Error::PathError(index_path.clone(), e.to_string()))?;
        let tantivy_index = Index::open(directory)?;
        let reader = tantivy_index.reader()?;

        let state = IndexState {
            schema: metadata.schema,
            config: metadata.config,
            _tantivy_schema: tantivy_schema,
            tantivy_index,
            reader,
            writer: None,
            field_map,
            id_field,
            expires_at_field,
        };

        self.indexes.insert(name.to_string(), state);
        Ok(())
    }
}

/// Index metadata stored on disk.
#[derive(Debug, Serialize, Deserialize)]
struct IndexMetadata {
    schema: IndexSchema,
    config: IndexConfig,
}

/// Build Tantivy schema from our schema definition.
fn build_tantivy_schema(
    schema: &IndexSchema,
) -> Result<(Schema, HashMap<String, Field>, Field, Field)> {
    let mut builder = SchemaBuilder::new();
    let mut field_map = HashMap::new();

    let id_field = builder.add_text_field("_id", STRING | STORED);
    let expires_at_field = builder.add_i64_field("_expires_at", STORED);

    for field_def in &schema.fields {
        let tantivy_field = match field_def.field_type {
            FieldType::Text => {
                let options = TextOptions::default()
                    .set_indexing_options(
                        TextFieldIndexing::default()
                            .set_tokenizer("default")
                            .set_index_option(
                                tantivy::schema::IndexRecordOption::WithFreqsAndPositions,
                            ),
                    )
                    .set_stored();
                builder.add_text_field(&field_def.name, options)
            }
            FieldType::String => builder.add_text_field(&field_def.name, STRING | STORED),
            FieldType::I64 => builder.add_i64_field(&field_def.name, STORED),
            FieldType::F64 => builder.add_f64_field(&field_def.name, STORED),
            FieldType::DateTime => builder.add_i64_field(&field_def.name, STORED),
            FieldType::StringArray => builder.add_text_field(&field_def.name, STRING | STORED),
            FieldType::Json => builder.add_json_field(&field_def.name, STORED),
        };

        field_map.insert(field_def.name.clone(), tantivy_field);
    }

    Ok((builder.build(), field_map, id_field, expires_at_field))
}

/// Add a value to a Tantivy document.
fn add_field_value(
    doc: &mut TantivyDocument,
    field: Field,
    value: &serde_json::Value,
) -> Result<()> {
    match value {
        serde_json::Value::String(s) => {
            doc.add_text(field, s);
        }
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                doc.add_i64(field, i);
            } else if let Some(f) = n.as_f64() {
                doc.add_f64(field, f);
            }
        }
        serde_json::Value::Bool(b) => {
            doc.add_bool(field, *b);
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                if let serde_json::Value::String(s) = v {
                    doc.add_text(field, s);
                }
            }
        }
        serde_json::Value::Object(_) => {
            debug!("Skipping JSON object field");
        }
        serde_json::Value::Null => {}
    }
    Ok(())
}

/// Convert Tantivy value to JSON.
fn tantivy_value_to_json<'a>(value: impl tantivy::schema::Value<'a>) -> serde_json::Value {
    if let Some(s) = value.as_str() {
        serde_json::Value::String(s.to_string())
    } else if let Some(n) = value.as_i64() {
        serde_json::Value::Number(n.into())
    } else if let Some(n) = value.as_u64() {
        serde_json::Value::Number(n.into())
    } else if let Some(n) = value.as_f64() {
        serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
    } else if let Some(b) = value.as_bool() {
        serde_json::Value::Bool(b)
    } else {
        serde_json::Value::Null
    }
}

/// Build a Tantivy query from our SearchQuery.
fn build_tantivy_query(
    state: &IndexState,
    query: &SearchQuery,
) -> Result<Box<dyn tantivy::query::Query>> {
    use tantivy::query::{BooleanQuery, Occur, QueryParser};

    let mut queries: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();

    if let Some(ref text) = query.text {
        // Collect all indexed text fields
        let text_fields: Vec<Field> = state
            .field_map
            .iter()
            .filter_map(|(field_name, tantivy_field)| {
                if let Some(field_def) = state.schema.get_field(field_name) {
                    if field_def.field_type == FieldType::Text && field_def.indexed {
                        return Some(*tantivy_field);
                    }
                }
                None
            })
            .collect();

        if !text_fields.is_empty() {
            // Use QueryParser for proper full-text search with tokenization
            let query_parser = QueryParser::for_index(&state.tantivy_index, text_fields);

            match query_parser.parse_query(text) {
                Ok(parsed_query) => {
                    queries.push((Occur::Must, parsed_query));
                }
                Err(e) => {
                    // If parsing fails, fall back to term queries for each word
                    debug!("Query parse failed, falling back to term search: {}", e);
                    for word in text.split_whitespace() {
                        for (field_name, tantivy_field) in &state.field_map {
                            if let Some(field_def) = state.schema.get_field(field_name) {
                                if field_def.field_type == FieldType::Text && field_def.indexed {
                                    let term = tantivy::Term::from_field_text(*tantivy_field, word);
                                    let term_query: Box<dyn tantivy::query::Query> =
                                        Box::new(tantivy::query::TermQuery::new(
                                            term,
                                            tantivy::schema::IndexRecordOption::WithFreqsAndPositions,
                                        ));
                                    queries.push((Occur::Should, term_query));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    for filter in &query.filters {
        if let Some(tantivy_field) = state.field_map.get(&filter.field) {
            let filter_query = build_filter_query(*tantivy_field, &filter.op, &filter.value)?;
            queries.push((Occur::Must, filter_query));
        }
    }

    if queries.is_empty() {
        Ok(Box::new(tantivy::query::AllQuery))
    } else {
        Ok(Box::new(BooleanQuery::new(queries)))
    }
}

/// Build a filter query.
fn build_filter_query(
    field: Field,
    op: &super::search::FilterOp,
    value: &serde_json::Value,
) -> Result<Box<dyn tantivy::query::Query>> {
    use tantivy::query::{RangeQuery, TermQuery};
    use tantivy::schema::IndexRecordOption;

    match op {
        super::search::FilterOp::Eq => {
            let term = match value {
                serde_json::Value::String(s) => tantivy::Term::from_field_text(field, s),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        tantivy::Term::from_field_i64(field, i)
                    } else {
                        return Err(Error::InvalidFieldValue("Expected integer".to_string()));
                    }
                }
                _ => return Err(Error::InvalidFieldValue("Invalid filter value".to_string())),
            };
            Ok(Box::new(TermQuery::new(term, IndexRecordOption::Basic)))
        }
        super::search::FilterOp::Gte | super::search::FilterOp::Lte => {
            let num = match value {
                serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
                _ => return Err(Error::InvalidFieldValue("Expected number".to_string())),
            };

            Ok(Box::new(RangeQuery::new(
                Bound::Included(Term::from_field_i64(field, num)),
                Bound::Unbounded,
            )))
        }
        _ => {
            let term = match value {
                serde_json::Value::String(s) => tantivy::Term::from_field_text(field, s),
                _ => return Err(Error::InvalidFieldValue("Invalid filter value".to_string())),
            };
            Ok(Box::new(TermQuery::new(term, IndexRecordOption::Basic)))
        }
    }
}

/// Calculate directory size recursively.
fn calculate_dir_size(path: &Path) -> Result<u64> {
    let mut size = 0;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                size += calculate_dir_size(&path)?;
            } else {
                size += entry.metadata()?.len();
            }
        }
    }
    Ok(size)
}

/// Snippet generator for highlighting.
struct SnippetGeneratorEntry {
    field_name: String,
    field: Field,
    generator: tantivy::snippet::SnippetGenerator,
}

/// Create snippet generators for highlighted fields.
fn create_snippet_generator(
    state: &IndexState,
    query_text: &str,
    config: &super::search::HighlightConfig,
) -> Result<Vec<SnippetGeneratorEntry>> {
    use tantivy::query::QueryParser;
    use tantivy::snippet::SnippetGenerator;

    let mut generators = Vec::new();

    let fields_to_highlight = if config.fields.is_empty() {
        state.field_map.keys().cloned().collect::<Vec<_>>()
    } else {
        config.fields.clone()
    };

    let text_fields: Vec<Field> = fields_to_highlight
        .iter()
        .filter_map(|field_name| {
            if let Some(&tantivy_field) = state.field_map.get(field_name) {
                if let Some(field_def) = state.schema.get_field(field_name) {
                    if field_def.field_type == FieldType::Text {
                        return Some(tantivy_field);
                    }
                }
            }
            None
        })
        .collect();

    let query_parser = QueryParser::for_index(&state.tantivy_index, text_fields.clone());

    let query = query_parser
        .parse_query(query_text)
        .map_err(|e| Error::ParseError(format!("Query parse error: {}", e)))?;

    for field_name in fields_to_highlight {
        if let Some(&tantivy_field) = state.field_map.get(&field_name) {
            if let Some(field_def) = state.schema.get_field(&field_name) {
                if field_def.field_type == FieldType::Text {
                    let generator =
                        SnippetGenerator::create(&state.reader.searcher(), &query, tantivy_field)?;
                    generators.push(SnippetGeneratorEntry {
                        field_name: field_name.clone(),
                        field: tantivy_field,
                        generator,
                    });
                }
            }
        }
    }

    Ok(generators)
}

/// Generate highlights for a document.
fn generate_highlights(
    doc: &TantivyDocument,
    generators: &[SnippetGeneratorEntry],
    config: &super::search::HighlightConfig,
) -> (
    Option<serde_json::Map<String, serde_json::Value>>,
    Option<String>,
) {
    let mut highlights = serde_json::Map::new();
    let mut first_highlight: Option<String> = None;

    let open_tag = format!("<{}>", config.highlight_tag);
    let close_tag = format!("</{}>", config.highlight_tag);

    for entry in generators {
        if let Some(value) = doc.get_first(entry.field) {
            if let Some(text) = value.as_str() {
                let mut snippet = entry.generator.snippet(text);
                snippet.set_snippet_prefix_postfix(&open_tag, &close_tag);

                let highlighted = snippet.to_html();
                highlights.insert(
                    entry.field_name.clone(),
                    serde_json::Value::String(highlighted.clone()),
                );

                if first_highlight.is_none() {
                    first_highlight = Some(highlighted);
                }

                if highlights.len() >= config.num_snippets {
                    break;
                }
            }
        }
    }

    if highlights.is_empty() {
        (None, None)
    } else {
        (Some(highlights), first_highlight)
    }
}

impl IndexManager {
    /// Add multiple documents in batch.
    ///
    /// Returns a BatchResult with success/failure counts.
    pub fn add_documents_batch(
        &mut self,
        index_name: &str,
        documents: Vec<BatchDocumentInput>,
        default_ttl: Option<String>,
        _parallel: usize,
    ) -> crate::Result<super::document::BatchResult> {
        let total = documents.len();
        let mut errors = Vec::new();
        let mut success_count = 0usize;

        // Parse default TTL if provided
        let default_expires_at = if let Some(ref ttl_str) = default_ttl {
            Some(parse_ttl(ttl_str)?)
        } else {
            None
        };

        // Acquire lock once for the entire batch
        let _lock = self.acquire_index_lock(index_name)?;

        for doc_input in documents {
            let mut doc = Document::new(doc_input.id.clone(), doc_input.fields);

            // Apply TTL: document-specific first, then default
            let ttl = doc_input
                .ttl
                .or_else(|| default_expires_at.map(|_| String::new()));
            if let Some(ttl_str) = ttl {
                let duration = if ttl_str.is_empty() {
                    chrono::Duration::seconds(
                        default_expires_at.map(|d| d.num_seconds()).unwrap_or(0),
                    )
                } else {
                    match parse_ttl(&ttl_str) {
                        Ok(d) => d,
                        Err(e) => {
                            errors.push(super::document::BatchError {
                                id: doc_input.id.clone(),
                                error: format!("Invalid TTL: {}", e),
                            });
                            continue;
                        }
                    }
                };
                doc = doc.with_expiry(chrono::Utc::now() + duration);
            }

            // Add document directly (lock already held)
            match self.add_document_internal(index_name, doc) {
                Ok(_) => {
                    success_count += 1;
                }
                Err(e) => {
                    errors.push(super::document::BatchError {
                        id: doc_input.id.clone(),
                        error: e.to_string(),
                    });
                }
            }
        }

        Ok(super::document::BatchResult {
            total,
            success: success_count,
            failed: errors.len(),
            errors,
        })
    }

    /// Internal add document without lock (for batch operations).
    fn add_document_internal(&mut self, index_name: &str, document: Document) -> Result<()> {
        let state = self
            .indexes
            .get_mut(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        let id = document.id.clone();
        let expires_at_ts = document
            .expires_at
            .map(|t| t.timestamp())
            .unwrap_or(i64::MAX);
        let field_values: Vec<(String, serde_json::Value)> = document.fields.into_iter().collect();

        state.ensure_writer()?;

        let writer = state.writer.as_mut().ok_or(Error::WriterNotInitialized)?;

        // Delete existing document with same ID
        let delete_term = Term::from_field_text(state.id_field, &id);
        writer.delete_term(delete_term);

        // Build new document
        let mut doc = TantivyDocument::new();
        doc.add_text(state.id_field, &id);
        doc.add_i64(state.expires_at_field, expires_at_ts);

        for (field_name, value) in field_values {
            if let Some(tantivy_field) = state.field_map.get(&field_name) {
                let _ = add_field_value(&mut doc, *tantivy_field, &value);
            }
        }

        writer.add_document(doc)?;
        debug!("Added document {} to index {}", id, index_name);

        Ok(())
    }
}

/// Parse a TTL string into a duration.
fn parse_ttl(ttl: &str) -> crate::Result<chrono::Duration> {
    let ttl = ttl.trim();

    if ttl.is_empty() {
        return Err(crate::Error::ParseError("Empty TTL".to_string()));
    }

    let numeric_end = ttl.find(|c: char| !c.is_ascii_digit()).unwrap_or(ttl.len());

    if numeric_end == 0 {
        return Err(crate::Error::ParseError(format!("Invalid TTL: {}", ttl)));
    }

    let number: i64 = ttl[..numeric_end]
        .parse()
        .map_err(|_| crate::Error::ParseError(format!("Invalid TTL number: {}", ttl)))?;

    let unit = &ttl[numeric_end..];

    let duration = match unit {
        "s" | "sec" | "seconds" => chrono::Duration::seconds(number),
        "m" | "min" | "minutes" => chrono::Duration::minutes(number),
        "h" | "hour" | "hours" => chrono::Duration::hours(number),
        "d" | "day" | "days" => chrono::Duration::days(number),
        "w" | "week" | "weeks" => chrono::Duration::weeks(number),
        _ => {
            return Err(crate::Error::ParseError(format!(
                "Unknown TTL unit: {}",
                unit
            )))
        }
    };

    Ok(duration)
}
