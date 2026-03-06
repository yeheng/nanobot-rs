//! History index for session messages stored in SQLite.
//!
//! Provides a read-write separated design:
//! - `HistoryIndexReader`: lock-free, `Clone + Send + Sync` — safe for concurrent search
//! - `HistoryIndexWriter`: requires `&mut self` — wrap in `Mutex` for shared access
//!
//! Both structs share the same underlying `IndexReader` via clone. When the writer
//! commits, Tantivy's `OnCommitWithDelay` reload policy automatically makes new
//! data visible to all reader clones.

use std::path::PathBuf;

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
use tracing::{debug, info};

use super::TantivyError;
use crate::search::{SearchQuery, SearchResult, SortOrder};

/// History document schema fields.
#[derive(Clone, Copy)]
struct HistorySchema {
    id: Field,
    content: Field,
    role: Field,
    session_key: Field,
    timestamp: Field,
    tools: Field,
}

impl HistorySchema {
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
        let content = builder.add_text_field("content", text_options);
        let role = builder.add_text_field("role", STRING | STORED);
        let session_key = builder.add_text_field("session_key", STRING | STORED);
        let timestamp = builder.add_i64_field("timestamp", STORED);
        let tools = builder.add_text_field("tools", STORED);

        (
            builder.build(),
            Self {
                id,
                content,
                role,
                session_key,
                timestamp,
                tools,
            },
        )
    }
}

/// Thread-safe, lock-free reader for the history index.
///
/// Tantivy's `IndexReader` is `Clone + Send + Sync` (backed by `ArcSwap`),
/// so this struct needs no external synchronization. Wrap in `Arc` for sharing.
pub struct HistoryIndexReader {
    fields: HistorySchema,
    reader: IndexReader,
}

impl HistoryIndexReader {
    /// Search the history index.
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

                let role = doc
                    .get_first(self.fields.role)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let doc_session_key = doc
                    .get_first(self.fields.session_key)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let ts = doc
                    .get_first(self.fields.timestamp)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                let tools_json = doc
                    .get_first(self.fields.tools)
                    .and_then(|v| v.as_str())
                    .unwrap_or("[]");

                let tools: Vec<String> = serde_json::from_str(tools_json).unwrap_or_default();

                let timestamp = DateTime::from_timestamp(ts, 0)
                    .map(|dt| dt.with_timezone(&Utc).to_rfc3339())
                    .unwrap_or_default();

                // Apply role filter if specified
                if let Some(ref role_filter) = query.role {
                    if &role != role_filter {
                        continue;
                    }
                }

                // Apply session_key filter if specified
                if let Some(ref sk_filter) = query.session_key {
                    if &doc_session_key != sk_filter {
                        continue;
                    }
                }

                // Apply date range filter if specified
                if let Some(ref date_range) = query.date_range {
                    if let Some(ref from) = date_range.from {
                        if let Ok(from_dt) = DateTime::parse_from_rfc3339(from) {
                            if ts < from_dt.timestamp() {
                                continue;
                            }
                        }
                    }
                    if let Some(ref to) = date_range.to {
                        if let Ok(to_dt) = DateTime::parse_from_rfc3339(to) {
                            if ts > to_dt.timestamp() {
                                continue;
                            }
                        }
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
                    format!("{}...", &content[..500])
                } else {
                    content
                };

                results.push(SearchResult {
                    id,
                    content: display_content,
                    score,
                    tags: tools,
                    source: Some(role),
                    timestamp: Some(timestamp),
                    highlight,
                    metadata: {
                        let mut meta = serde_json::Map::new();
                        meta.insert(
                            "session_key".to_string(),
                            serde_json::Value::String(doc_session_key),
                        );
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

    /// Get all indexed document IDs.
    pub fn get_indexed_documents(&self) -> Result<std::collections::HashSet<String>, TantivyError> {
        use tantivy::collector::DocSetCollector;

        let searcher = self.reader.searcher();
        let query = tantivy::query::AllQuery;

        let doc_addresses = searcher.search(&query, &DocSetCollector)?;
        let mut docs = std::collections::HashSet::new();

        for doc_address in doc_addresses {
            if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                let id = doc
                    .get_first(self.fields.id)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                docs.insert(id);
            }
        }

        Ok(docs)
    }

    /// Build a Tantivy query from SearchQuery.
    fn build_query(&self, query: &SearchQuery) -> Result<Box<dyn Query>, TantivyError> {
        let mut queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        // Text search
        if let Some(ref text) = query.text {
            let text_query: Box<dyn Query> = Box::new(TermQuery::new(
                Term::from_field_text(self.fields.content, text),
                IndexRecordOption::WithFreqsAndPositions,
            ));
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

        // Role filter
        if let Some(ref role) = query.role {
            let role_query: Box<dyn Query> = Box::new(TermQuery::new(
                Term::from_field_text(self.fields.role, role),
                IndexRecordOption::Basic,
            ));
            queries.push((Occur::Must, role_query));
        }

        // Session key filter
        if let Some(ref session_key) = query.session_key {
            let sk_query: Box<dyn Query> = Box::new(TermQuery::new(
                Term::from_field_text(self.fields.session_key, session_key),
                IndexRecordOption::Basic,
            ));
            queries.push((Occur::Must, sk_query));
        }

        // If no queries, match all
        if queries.is_empty() {
            return Ok(Box::new(tantivy::query::AllQuery));
        }

        Ok(Box::new(BooleanQuery::new(queries)))
    }
}

/// Writer for the history index — requires `&mut self` for all mutating operations.
///
/// Wrap in `tokio::sync::Mutex` (or similar) when shared across async tasks.
pub struct HistoryIndexWriter {
    fields: HistorySchema,
    index: Index,
    writer: Option<IndexWriter>,
    reader: IndexReader,
}

impl HistoryIndexWriter {
    /// Ensure writer is available.
    fn ensure_writer(&mut self) -> Result<&mut IndexWriter, TantivyError> {
        if self.writer.is_none() {
            let writer = self.index.writer(50_000_000)?;
            self.writer = Some(writer);
        }
        Ok(self.writer.as_mut().unwrap())
    }

    /// Add or update a history document.
    pub fn index_document(
        &mut self,
        id: &str,
        content: &str,
        role: &str,
        session_key: &str,
        timestamp: DateTime<Utc>,
        tools: Option<&[String]>,
    ) -> Result<(), TantivyError> {
        let id_field = self.fields.id;
        let content_field = self.fields.content;
        let role_field = self.fields.role;
        let session_key_field = self.fields.session_key;
        let timestamp_field = self.fields.timestamp;
        let tools_field = self.fields.tools;

        let writer = self.ensure_writer()?;

        // Delete existing document with same ID
        let delete_term = Term::from_field_text(id_field, id);
        writer.delete_term(delete_term);

        // Create new document
        let mut doc = TantivyDocument::new();
        doc.add_text(id_field, id);
        doc.add_text(content_field, content);
        doc.add_text(role_field, role);
        doc.add_text(session_key_field, session_key);
        doc.add_i64(timestamp_field, timestamp.timestamp());

        if let Some(tools_list) = tools {
            let tools_json = serde_json::to_string(tools_list).unwrap_or_else(|_| "[]".to_string());
            doc.add_text(tools_field, &tools_json);
        }

        writer.add_document(doc)?;
        debug!(
            "Indexed history document: {} (session: {})",
            id, session_key
        );
        Ok(())
    }

    /// Delete a document by ID.
    pub fn delete_document(&mut self, id: &str) -> Result<(), TantivyError> {
        let id_field = self.fields.id;
        let writer = self.ensure_writer()?;
        let delete_term = Term::from_field_text(id_field, id);
        writer.delete_term(delete_term);
        debug!("Deleted history document from index: {}", id);
        Ok(())
    }

    /// Delete all documents for a session.
    pub fn delete_session(&mut self, session_key: &str) -> Result<(), TantivyError> {
        let session_key_field = self.fields.session_key;
        let writer = self.ensure_writer()?;
        let delete_term = Term::from_field_text(session_key_field, session_key);
        writer.delete_term(delete_term);
        debug!("Deleted all history documents for session: {}", session_key);
        Ok(())
    }

    /// Commit pending changes and reload the reader.
    pub fn commit(&mut self) -> Result<(), TantivyError> {
        if let Some(writer) = &mut self.writer {
            writer.commit()?;
            self.reader.reload()?;
            info!("History index committed");
        }
        Ok(())
    }

    /// Rebuild the entire index from SQLite database.
    ///
    /// This clears the existing index and re-indexes all session messages
    /// from the database.
    pub async fn rebuild_from_db(
        &mut self,
        db: &crate::memory::SqliteStore,
    ) -> Result<usize, TantivyError> {
        use sqlx::Row;

        // Clear existing index
        let writer = self.ensure_writer()?;
        writer.delete_all_documents()?;

        // Query all messages with their IDs
        let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
            "SELECT id, session_key, role, content, timestamp, tools_used FROM session_messages ORDER BY id ASC",
        )
        .fetch_all(&db.pool)
        .await
        .map_err(|e| TantivyError::OperationError(format!("Failed to query messages: {}", e)))?;

        let mut count = 0;
        for row in rows {
            let id: i64 = row.get("id");
            let session_key: String = row.get("session_key");
            let role: String = row.get("role");
            let content: String = row.get("content");
            let timestamp_str: String = row.get("timestamp");
            let tools_json: Option<String> = row.get("tools_used");

            let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            let tools: Option<Vec<String>> =
                tools_json.and_then(|json| serde_json::from_str(&json).ok());

            // Use database ID as the index ID
            let doc_id = format!("{}:{}", session_key, id);

            self.index_document(
                &doc_id,
                &content,
                &role,
                &session_key,
                timestamp,
                tools.as_deref(),
            )?;
            count += 1;

            if count % 100 == 0 {
                info!("Rebuilding history index: {} documents", count);
            }
        }

        self.commit()?;
        info!("History index rebuilt: {} documents", count);
        Ok(count)
    }

    /// Incremental update: sync with database.
    ///
    /// This method compares the indexed documents with the database messages
    /// and adds new messages, removes deleted ones.
    ///
    /// Returns update statistics.
    pub async fn incremental_update(
        &mut self,
        db: &crate::memory::SqliteStore,
    ) -> Result<super::IndexUpdateStats, TantivyError> {
        use sqlx::Row;

        let mut stats = super::IndexUpdateStats::default();

        // Get all indexed document IDs
        let indexed_docs = self.get_indexed_documents()?;

        // Query all messages from database
        let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
            "SELECT id, session_key, role, content, timestamp, tools_used FROM session_messages ORDER BY id ASC",
        )
        .fetch_all(&db.pool)
        .await
        .map_err(|e| TantivyError::OperationError(format!("Failed to query messages: {}", e)))?;

        /// Helper struct to hold message data during sync.
        struct MessageData {
            doc_id: String,
            session_key: String,
            role: String,
            content: String,
            timestamp: DateTime<Utc>,
            tools: Option<Vec<String>>,
        }

        // Build a set of database message IDs
        let mut db_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut new_messages: Vec<MessageData> = Vec::new();

        for row in rows {
            let id: i64 = row.get("id");
            let session_key: String = row.get("session_key");
            let role: String = row.get("role");
            let content: String = row.get("content");
            let timestamp_str: String = row.get("timestamp");
            let tools_json: Option<String> = row.get("tools_used");

            let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            let tools: Option<Vec<String>> =
                tools_json.and_then(|json| serde_json::from_str(&json).ok());

            let doc_id = format!("{}:{}", session_key, id);
            db_ids.insert(doc_id.clone());

            // Check if this message is new (not in index)
            if !indexed_docs.contains(&doc_id) {
                new_messages.push(MessageData {
                    doc_id,
                    session_key,
                    role,
                    content,
                    timestamp,
                    tools,
                });
            }
        }

        // Remove documents that no longer exist in database
        for id in &indexed_docs {
            if !db_ids.contains(id) {
                self.delete_document(id)?;
                stats.removed += 1;
                debug!("Removed deleted message from history index: {}", id);
            }
        }

        // Add new messages
        for msg in new_messages {
            self.index_document(
                &msg.doc_id,
                &msg.content,
                &msg.role,
                &msg.session_key,
                msg.timestamp,
                msg.tools.as_deref(),
            )?;
            stats.added += 1;
            debug!("Added new message to history index: {}", msg.doc_id);
        }

        if stats.added > 0 || stats.removed > 0 {
            self.commit()?;
        }

        info!(
            "History index incremental update: {} added, {} removed",
            stats.added, stats.removed
        );
        Ok(stats)
    }

    /// Get all indexed document IDs.
    fn get_indexed_documents(&self) -> Result<std::collections::HashSet<String>, TantivyError> {
        use tantivy::collector::DocSetCollector;

        let searcher = self.reader.searcher();
        let query = tantivy::query::AllQuery;

        let doc_addresses = searcher.search(&query, &DocSetCollector)?;
        let mut docs = std::collections::HashSet::new();

        for doc_address in doc_addresses {
            if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                let id = doc
                    .get_first(self.fields.id)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                docs.insert(id);
            }
        }

        Ok(docs)
    }
}

/// Open (or create) a history index, returning a paired reader and writer.
///
/// Both share the same underlying `IndexReader`. When the writer commits,
/// Tantivy's `OnCommitWithDelay` policy auto-reloads all clones.
pub fn open_history_index(
    index_path: impl Into<PathBuf>,
) -> Result<(HistoryIndexReader, HistoryIndexWriter), TantivyError> {
    let index_path = index_path.into();
    let (schema, fields) = HistorySchema::build();

    if !index_path.exists() {
        std::fs::create_dir_all(&index_path)?;
        debug!("Created history index directory: {:?}", index_path);
    }

    let directory = MmapDirectory::open(&index_path)
        .map_err(|e| TantivyError::OpenError(format!("Failed to open directory: {}", e)))?;

    let index = Index::open_or_create(directory, schema)?;
    let reader = index.reader()?;

    let reader_clone = reader.clone();

    Ok((
        HistoryIndexReader {
            fields,
            reader: reader_clone,
        },
        HistoryIndexWriter {
            fields,
            index,
            writer: None,
            reader,
        },
    ))
}
