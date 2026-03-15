//! Index manager for managing multiple indexes.
//!
//! ## Background Job Architecture
//!
//! This module uses a **background job queue** pattern for all write operations:
//!
//! 1. **DashMap for index registry**: Uses `DashMap<String, Arc<IndexState>>` for
//!    concurrent read access to index metadata.
//!
//! 2. **Background Write Jobs**: All write operations (add_document, delete_document,
//!    commit, compact) are submitted as background jobs. The caller receives a JobId
//!    immediately and can query status via JobRegistry.
//!
//! 3. **RwLock for Writer**: A short-lived RwLock protects the IndexWriter during
//!    actual write operations. Since writes happen in background tasks, callers
//!    are never blocked.
//!
//! 4. **Job-based tracking**: Every write operation returns a JobId for status tracking.

use std::collections::HashMap;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tantivy::{
    directory::MmapDirectory,
    schema::{Field, Schema, SchemaBuilder, TextFieldIndexing, TextOptions, Value, STORED, STRING},
    Index, IndexReader, IndexWriter, TantivyDocument, Term,
};
use tracing::{debug, info, warn};

use super::document::{BatchDocumentInput, Document};
use super::schema::{FieldDef, FieldType, IndexConfig, IndexSchema};
use super::search::{SearchQuery, SearchResult};
use crate::maintenance::{JobId, JobRegistry, JobType};
use crate::{Error, Result};

/// Internal index state.
///
/// Each index has its own state with:
/// - Immutable data (schema, config, tantivy_index, reader, field_map)
/// - Mutable writer protected by RwLock
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

    // Mutable field (protected by RwLock)
    /// Index writer with lazy initialization.
    /// Uses RwLock to allow multiple readers (for read-heavy operations)
    /// while still allowing writer recreation during compact.
    writer: RwLock<Option<IndexWriter>>,
}

impl IndexState {
    /// Ensure the writer is initialized.
    fn ensure_writer(&self) -> Result<()> {
        let mut guard = self.writer.write();
        if guard.is_none() {
            *guard = Some(self.tantivy_index.writer(50_000_000)?);
        }
        Ok(())
    }
}

/// Index manager handling multiple indexes.
///
/// Uses `DashMap` for concurrent access to the index registry.
/// All write operations are background jobs tracked via JobRegistry.
#[derive(Clone)]
pub struct IndexManager {
    base_path: PathBuf,
    /// Concurrent index registry using DashMap.
    /// Each index is wrapped in Arc for shared ownership.
    indexes: DashMap<String, Arc<IndexState>>,
    /// Job registry for tracking background operations.
    job_registry: Arc<JobRegistry>,
}

impl IndexManager {
    /// Get the job registry.
    pub fn job_registry(&self) -> &Arc<JobRegistry> {
        &self.job_registry
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

    /// Unload an index from memory (remove from DashMap).
    /// The index files remain on disk.
    pub fn unload_index(&self, name: &str) -> Result<()> {
        if self.indexes.remove(name).is_some() {
            info!("Unloaded index: {}", name);
        }
        Ok(())
    }

    /// Load an index by name (public wrapper for the private load_index).
    pub fn load_index_by_name(&self, name: &str) -> Result<()> {
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
    /// Create a new index manager with a shared JobRegistry.
    pub fn new(base_path: impl AsRef<Path>, job_registry: Arc<JobRegistry>) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            indexes: DashMap::new(),
            job_registry,
        }
    }

    /// Create a new index.
    ///
    /// This operation atomically inserts the new index into the registry.
    /// If an index with the same name already exists, returns an error.
    pub fn create_index(
        &self,
        name: &str,
        fields: Vec<FieldDef>,
        config: Option<IndexConfig>,
    ) -> Result<IndexSchema> {
        // Quick check without holding any lock
        if self.indexes.contains_key(name) {
            return Err(Error::IndexAlreadyExists(name.to_string()));
        }

        // Build index state outside of any lock
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

        let state = Arc::new(IndexState {
            schema: schema.clone(),
            config,
            _tantivy_schema: tantivy_schema,
            tantivy_index,
            reader,
            writer: RwLock::new(None),
            field_map,
            id_field,
            expires_at_field,
        });

        // Atomic insert with race condition check
        if self.indexes.insert(name.to_string(), state).is_some() {
            // Race condition: another thread created the same index
            // Rollback by removing the directory we just created
            let _ = std::fs::remove_dir_all(&index_path);
            return Err(Error::IndexAlreadyExists(name.to_string()));
        }

        info!("Created index: {}", name);
        Ok(schema)
    }

    /// Drop an index.
    ///
    /// Atomically removes the index from the registry and deletes its directory.
    pub fn drop_index(&self, name: &str) -> Result<()> {
        // Atomically remove and get the old value
        let state = self
            .indexes
            .remove(name)
            .ok_or_else(|| Error::IndexNotFound(name.to_string()))?;

        // Drop the state (this will wait for any in-progress operations)
        drop(state);

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
        self.indexes
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Get index schema.
    pub fn get_schema(&self, name: &str) -> Result<Option<IndexSchema>> {
        Ok(self.indexes.get(name).map(|entry| entry.schema.clone()))
    }

    /// Get index config.
    pub fn get_config(&self, name: &str) -> Result<Option<IndexConfig>> {
        Ok(self.indexes.get(name).map(|entry| entry.config.clone()))
    }

    /// Add a document to an index as a background job.
    ///
    /// Returns a JobId for tracking the operation.
    /// Use `job_registry().get_job(job_id)` to check status.
    pub fn add_document(&self, index_name: &str, document: Document) -> Result<JobId> {
        // Create job entry for API compatibility
        let job_id = self
            .job_registry
            .create_job(JobType::BulkImport, Some(index_name.to_string()));

        // Execute synchronously - CLI doesn't need async job queue
        self.job_registry.start_job(&job_id);

        let id = document.id.clone();
        match self.add_document_sync(index_name, document) {
            Ok(_) => {
                debug!("Added document {} to index {}", id, index_name);
                self.job_registry.complete_job(
                    &job_id,
                    format!("Document '{}' added successfully", id),
                );
            }
            Err(e) => {
                self.job_registry.fail_job(&job_id, e.to_string());
            }
        }

        Ok(job_id)
    }

    /// Add a document synchronously (for batch operations).
    ///
    /// This is a synchronous version for use in batch operations.
    /// Returns Ok(()) if the document was added successfully.
    pub fn add_document_sync(&self, index_name: &str, document: Document) -> Result<()> {
        let state = self.get_index(index_name)?;

        let id = document.id.clone();
        let expires_at_ts = document
            .expires_at
            .map(|t| t.timestamp())
            .unwrap_or(i64::MAX);
        let field_values: Vec<(String, serde_json::Value)> =
            document.fields.into_iter().collect();

        // Acquire write lock for the operation
        state.ensure_writer()?;

        let mut guard = state.writer.write();
        let writer = match guard.as_mut() {
            Some(w) => w,
            None => {
                return Err(Error::WriterNotInitialized);
            }
        };

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

    /// Delete a document from an index (synchronous execution).
    ///
    /// Returns a JobId for API compatibility, but executes synchronously.
    pub fn delete_document(&self, index_name: &str, doc_id: &str) -> Result<JobId> {
        let job_id = self.job_registry.create_job(
            JobType::Custom("delete_document".to_string()),
            Some(index_name.to_string()),
        );

        self.job_registry.start_job(&job_id);

        match self.delete_document_sync(index_name, doc_id) {
            Ok(_) => {
                self.job_registry.complete_job(
                    &job_id,
                    format!("Document '{}' deleted successfully", doc_id),
                );
            }
            Err(e) => {
                self.job_registry.fail_job(&job_id, e.to_string());
            }
        }

        Ok(job_id)
    }

    /// Delete a document synchronously (internal implementation).
    fn delete_document_sync(&self, index_name: &str, doc_id: &str) -> Result<()> {
        let state = self.get_index(index_name)?;

        state.ensure_writer()?;

        let mut guard = state.writer.write();
        let writer = guard
            .as_mut()
            .ok_or(Error::WriterNotInitialized)?;

        let delete_term = Term::from_field_text(state.id_field, doc_id);
        writer.delete_term(delete_term);
        debug!("Deleted document {} from index {}", doc_id, index_name);

        Ok(())
    }

    /// Commit changes to an index (synchronous execution).
    ///
    /// Returns a JobId for API compatibility, but executes synchronously.
    pub fn commit(&self, index_name: &str) -> Result<JobId> {
        let job_id = self.job_registry.create_job(
            JobType::Custom("commit".to_string()),
            Some(index_name.to_string()),
        );

        self.job_registry.start_job(&job_id);

        match self.commit_sync(index_name) {
            Ok(_) => {
                self.job_registry.complete_job(
                    &job_id,
                    format!("Index '{}' committed successfully", index_name),
                );
            }
            Err(e) => {
                self.job_registry.fail_job(&job_id, e.to_string());
            }
        }

        Ok(job_id)
    }

    /// Commit changes synchronously (internal implementation).
    fn commit_sync(&self, index_name: &str) -> Result<()> {
        let state = self.get_index(index_name)?;

        state.ensure_writer()?;

        let mut guard = state.writer.write();
        let writer = guard
            .as_mut()
            .ok_or(Error::WriterNotInitialized)?;

        writer.commit()?;
        state.reader.reload()?;
        info!("Committed index {}", index_name);

        Ok(())
    }

    /// Search an index.
    ///
    /// This is a read-only operation and doesn't acquire the writer lock.
    pub fn search(&self, index_name: &str, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let state = self.get_index(index_name)?;

        let searcher = state.reader.searcher();
        let tantivy_query = build_tantivy_query(&state, query)?;

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
                    &state,
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
        let state = self.get_index(index_name)?;

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

    /// Compact an index as a background job.
    ///
    /// This commits any pending changes, waits for merging threads, and recreates the writer.
    /// Returns a JobId for tracking the operation.
    pub fn compact(&self, index_name: &str) -> Result<JobId> {
        let state = self.get_index(index_name)?;
        let index_name_owned = index_name.to_string();

        let job_id = self
            .job_registry
            .create_job(JobType::IndexCompact, Some(index_name_owned.clone()));

        let rt_handle = tokio::runtime::Handle::current();
        let job_id_clone = job_id.clone();
        let job_registry = self.job_registry.clone();

        rt_handle.spawn(async move {
            job_registry.start_job(&job_id_clone);
            job_registry.update_progress(&job_id_clone, 10, "Starting compaction".to_string());

            // Commit and wait for merges
            {
                let mut guard = state.writer.write();
                if let Some(writer) = guard.as_mut() {
                    if let Err(e) = writer.commit() {
                        job_registry.fail_job(&job_id_clone, format!("Commit failed: {}", e));
                        return;
                    }
                }
                job_registry.update_progress(&job_id_clone, 30, "Commit completed".to_string());

                if let Some(writer) = guard.take() {
                    if let Err(e) = writer.wait_merging_threads() {
                        warn!("Failed to wait for merging threads: {}", e);
                    }
                }
            }

            job_registry.update_progress(
                &job_id_clone,
                60,
                "Merging threads completed".to_string(),
            );

            // Recreate writer
            {
                let mut guard = state.writer.write();
                match state.tantivy_index.writer(50_000_000) {
                    Ok(new_writer) => {
                        *guard = Some(new_writer);
                    }
                    Err(e) => {
                        job_registry
                            .fail_job(&job_id_clone, format!("Failed to recreate writer: {}", e));
                        return;
                    }
                }
            }

            job_registry.update_progress(&job_id_clone, 80, "Reloading reader".to_string());

            if let Err(e) = state.reader.reload() {
                job_registry.fail_job(&job_id_clone, format!("Failed to reload reader: {}", e));
                return;
            }

            info!("Compacted index {}", index_name_owned);
            job_registry.complete_job(
                &job_id_clone,
                format!("Index '{}' compacted successfully", index_name_owned),
            );
        });

        Ok(job_id)
    }

    /// List all documents in an index (with pagination).
    pub fn list_documents(
        &self,
        index_name: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Document>> {
        let state = self.get_index(index_name)?;

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
    pub fn load_indexes(&self) -> Result<()> {
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

    fn load_index(&self, name: &str, metadata_path: &Path) -> Result<()> {
        let metadata_json = std::fs::read_to_string(metadata_path)?;
        let metadata: IndexMetadata = serde_json::from_str(&metadata_json)?;

        let index_path = self.index_path(name);
        let (tantivy_schema, field_map, id_field, expires_at_field) =
            build_tantivy_schema(&metadata.schema)?;

        let directory = MmapDirectory::open(&index_path)
            .map_err(|e| Error::PathError(index_path.clone(), e.to_string()))?;
        let tantivy_index = Index::open(directory)?;
        let reader = tantivy_index.reader()?;

        let state = Arc::new(IndexState {
            schema: metadata.schema,
            config: metadata.config,
            _tantivy_schema: tantivy_schema,
            tantivy_index,
            reader,
            writer: RwLock::new(None),
            field_map,
            id_field,
            expires_at_field,
        });

        // Insert into DashMap
        self.indexes.insert(name.to_string(), state);
        Ok(())
    }

    /// Get a reference to an index state.
    fn get_index(&self, name: &str) -> Result<Arc<IndexState>> {
        self.indexes
            .get(name)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| Error::IndexNotFound(name.to_string()))
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
    use tantivy::query::{BooleanQuery, Occur, TermQuery};
    use tantivy::schema::IndexRecordOption;

    let mut queries: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();

    if let Some(ref text) = query.text {
        for (field_name, tantivy_field) in &state.field_map {
            if let Some(field_def) = state.schema.get_field(field_name) {
                if field_def.field_type == FieldType::Text && field_def.indexed {
                    let term = tantivy::Term::from_field_text(*tantivy_field, text);
                    let term_query: Box<dyn tantivy::query::Query> = Box::new(TermQuery::new(
                        term,
                        IndexRecordOption::WithFreqsAndPositions,
                    ));
                    queries.push((Occur::Should, term_query));
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
        &self,
        index_name: &str,
        documents: Vec<BatchDocumentInput>,
        default_ttl: Option<String>,
        parallel: usize,
    ) -> crate::Result<super::document::BatchResult> {
        use std::sync::{Arc, Mutex};

        let total = documents.len();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let success_count = Arc::new(Mutex::new(0usize));

        // Parse default TTL if provided
        let default_expires_at = if let Some(ref ttl_str) = default_ttl {
            Some(parse_ttl(ttl_str)?)
        } else {
            None
        };

        // Process documents in parallel batches
        let chunk_size = (documents.len() + parallel - 1) / parallel;
        let chunks: Vec<Vec<BatchDocumentInput>> = documents
            .chunks(chunk_size.max(1))
            .map(|c| c.to_vec())
            .collect();

        let mut handles = Vec::new();

        for chunk in chunks {
            let index_name = index_name.to_string();
            let manager = self.clone();
            let errors = Arc::clone(&errors);
            let success_count = Arc::clone(&success_count);
            let default_expires_at = default_expires_at.map(|d| d.num_seconds());

            let handle = std::thread::spawn(move || {
                for doc_input in chunk {
                    let mut doc = Document::new(doc_input.id.clone(), doc_input.fields);

                    // Apply TTL: document-specific first, then default
                    let ttl = doc_input.ttl.or_else(|| default_expires_at.map(|_| String::new()));
                    if let Some(ttl_str) = ttl {
                        let duration = if ttl_str.is_empty() {
                            chrono::Duration::seconds(default_expires_at.unwrap_or(0))
                        } else {
                            match parse_ttl(&ttl_str) {
                                Ok(d) => d,
                                Err(e) => {
                                    errors
                                        .lock()
                                        .unwrap()
                                        .push(super::document::BatchError {
                                            id: doc_input.id.clone(),
                                            error: format!("Invalid TTL: {}", e),
                                        });
                                    continue;
                                }
                            }
                        };
                        doc = doc.with_expiry(chrono::Utc::now() + duration);
                    }

                    // Add document synchronously
                    match manager.add_document_sync(&index_name, doc) {
                        Ok(_) => {
                            *success_count.lock().unwrap() += 1;
                        }
                        Err(e) => {
                            errors.lock().unwrap().push(super::document::BatchError {
                                id: doc_input.id.clone(),
                                error: e.to_string(),
                            });
                        }
                    }
                }
            });

            handles.push(handle);
        }

        // Wait for all workers to complete
        for handle in handles {
            handle.join().expect("Worker thread panicked");
        }

        let errors_vec = std::mem::take(&mut *errors.lock().unwrap());
        let success = *success_count.lock().unwrap();

        Ok(super::document::BatchResult {
            total,
            success,
            failed: errors_vec.len(),
            errors: errors_vec,
        })
    }
}

/// Parse a TTL string into a duration (moved from main.rs for reuse).
fn parse_ttl(ttl: &str) -> crate::Result<chrono::Duration> {
    let ttl = ttl.trim();

    if ttl.is_empty() {
        return Err(crate::Error::ParseError("Empty TTL".to_string()));
    }

    let numeric_end = ttl.find(|c: char| !c.is_ascii_digit()).unwrap_or(ttl.len());

    if numeric_end == 0 {
        return Err(crate::Error::ParseError(format!(
            "Invalid TTL: {}",
            ttl
        )));
    }

    let number: i64 = ttl[..numeric_end]
        .parse()
        .map_err(|_| {
            crate::Error::ParseError(format!("Invalid TTL number: {}", ttl))
        })?;

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
