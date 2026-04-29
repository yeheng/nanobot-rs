//! LanceDB-backed vector store with optional ANN index.
//!
//! Uses the `lancedb` crate for embedded vector search. By default queries
//! use brute-force cosine distance over Arrow batches; once the table grows
//! past `ANN_THRESHOLD` rows, callers should invoke
//! [`LanceVectorStore::ensure_vector_index`] to build an IVF-PQ index for
//! sub-linear search.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::index::{vector::IvfPqIndexBuilder, Index};
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::DistanceType;

use crate::vector_store::{SearchResult, StoredEmbedding, VectorRecord, VectorStore};

/// Approximate row count above which an IVF-PQ index is worth building.
/// Below this, brute force scan is fast enough and IVF-PQ training is
/// unstable.
pub const ANN_THRESHOLD: i64 = 1024;

/// LanceDB-backed vector store.
pub struct LanceVectorStore {
    table: lancedb::Table,
    dim: usize,
}

impl LanceVectorStore {
    /// Open (or create) a LanceDB table at the given path.
    pub async fn open(db_path: &str, table_name: &str, dim: usize) -> Result<Self> {
        let db = lancedb::connect(db_path)
            .execute()
            .await
            .map_err(|e| anyhow!("failed to open LanceDB at {db_path}: {e}"))?;

        let table = match db.open_table(table_name).execute().await {
            Ok(t) => t,
            Err(lancedb::Error::TableNotFound { .. }) => {
                let schema = Self::schema(dim);
                let empty_batch = RecordBatch::new_empty(schema);
                db.create_table(table_name, vec![empty_batch])
                    .execute()
                    .await
                    .map_err(|e| anyhow!("failed to create LanceDB table '{table_name}': {e}"))?
            }
            Err(e) => return Err(anyhow!("failed to open LanceDB table '{table_name}': {e}")),
        };

        Ok(Self { table, dim })
    }

    /// Build (or replace) an IVF-PQ cosine index on the `vector` column when
    /// the table has at least [`ANN_THRESHOLD`] rows. No-op otherwise.
    ///
    /// Safe to call repeatedly; uses `replace(true)` so re-indexing on
    /// growth is allowed.
    pub async fn ensure_vector_index(&self) -> Result<bool> {
        let count = self
            .table
            .count_rows(None)
            .await
            .map_err(|e| anyhow!("count_rows failed: {e}"))? as i64;
        if count < ANN_THRESHOLD {
            return Ok(false);
        }
        let builder = IvfPqIndexBuilder::default().distance_type(DistanceType::Cosine);
        self.table
            .create_index(&["vector"], Index::IvfPq(builder))
            .replace(true)
            .execute()
            .await
            .map_err(|e| anyhow!("create_index failed: {e}"))?;
        Ok(true)
    }

    fn schema(dim: usize) -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("event_id", DataType::Utf8, false),
            Field::new("session_key", DataType::Utf8, false),
            Field::new("event_type", DataType::Utf8, false),
            Field::new("content_hash", DataType::Utf8, false),
            Field::new("created_at", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    dim as i32,
                ),
                true,
            ),
        ]))
    }

    /// Build a RecordBatch from a slice of VectorRecords.
    fn records_to_batch(records: &[VectorRecord], dim: usize) -> Result<RecordBatch> {
        let n = records.len();
        let event_ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        let session_keys: Vec<&str> = records.iter().map(|r| r.session_key.as_str()).collect();
        let event_types: Vec<&str> = records.iter().map(|r| r.event_type.as_str()).collect();
        let content_hashes: Vec<&str> = records.iter().map(|r| r.content_hash.as_str()).collect();
        let now = chrono::Utc::now().to_rfc3339();
        let created_ats: Vec<&str> = (0..n).map(|_| now.as_str()).collect();

        for r in records {
            if r.vector.len() != dim {
                return Err(anyhow!(
                    "vector length {} does not match table dim {}",
                    r.vector.len(),
                    dim
                ));
            }
        }

        let mut flat_values = Vec::with_capacity(n * dim);
        for r in records {
            flat_values.extend_from_slice(&r.vector);
        }
        let values: Arc<dyn Array> = Arc::new(Float32Array::from(flat_values));
        let item_field = Arc::new(Field::new("item", DataType::Float32, true));
        let vector_array = FixedSizeListArray::try_new(item_field, dim as i32, values, None)
            .map_err(|e| anyhow!("failed to build FixedSizeListArray: {e}"))?;

        let schema = Self::schema(dim);
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(event_ids)),
                Arc::new(StringArray::from(session_keys)),
                Arc::new(StringArray::from(event_types)),
                Arc::new(StringArray::from(content_hashes)),
                Arc::new(StringArray::from(created_ats)),
                Arc::new(vector_array),
            ],
        )
        .map_err(|e| anyhow!("failed to build RecordBatch: {e}"))?;

        Ok(batch)
    }

    /// Extract StoredEmbedding records from a RecordBatch.
    fn batch_to_stored(batch: &RecordBatch) -> Result<Vec<StoredEmbedding>> {
        let event_ids = batch
            .column_by_name("event_id")
            .ok_or_else(|| anyhow!("missing event_id column"))?
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| anyhow!("event_id is not StringArray"))?;

        let session_keys = batch
            .column_by_name("session_key")
            .ok_or_else(|| anyhow!("missing session_key column"))?
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| anyhow!("session_key is not StringArray"))?;

        let event_types = batch
            .column_by_name("event_type")
            .ok_or_else(|| anyhow!("missing event_type column"))?
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| anyhow!("event_type is not StringArray"))?;

        let created_ats = batch
            .column_by_name("created_at")
            .ok_or_else(|| anyhow!("missing created_at column"))?
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| anyhow!("created_at is not StringArray"))?;

        let vectors = batch
            .column_by_name("vector")
            .ok_or_else(|| anyhow!("missing vector column"))?
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .ok_or_else(|| anyhow!("vector is not FixedSizeListArray"))?;

        let mut results = Vec::with_capacity(batch.num_rows());
        for i in 0..batch.num_rows() {
            let list = vectors.value(i);
            let floats = list.as_any().downcast_ref::<Float32Array>().unwrap();
            let embedding: Vec<f32> = floats.values().to_vec();

            results.push(StoredEmbedding {
                event_id: event_ids.value(i).to_string(),
                session_key: session_keys.value(i).to_string(),
                embedding,
                event_type: event_types.value(i).to_string(),
                created_at: created_ats.value(i).to_string(),
            });
        }

        Ok(results)
    }
}

#[async_trait::async_trait]
impl VectorStore for LanceVectorStore {
    async fn upsert(&self, records: Vec<VectorRecord>) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let batch = Self::records_to_batch(&records, self.dim)?;
        let schema = batch.schema();

        let mut merge = self.table.merge_insert(&["event_id"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        let data: Box<dyn arrow_array::RecordBatchReader + Send> =
            Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema));
        merge
            .execute(data)
            .await
            .map_err(|e| anyhow!("LanceDB upsert failed: {e}"))?;

        Ok(())
    }

    async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        min_score: f32,
        exclude: &HashSet<String>,
    ) -> Result<Vec<SearchResult>> {
        if top_k == 0 || query.is_empty() {
            return Ok(Vec::new());
        }
        let overfetch = top_k + exclude.len();
        let stream = self
            .table
            .query()
            .nearest_to(query)
            .map_err(|e| anyhow!("invalid query vector: {e}"))?
            .distance_type(DistanceType::Cosine)
            .limit(overfetch)
            .execute()
            .await
            .map_err(|e| anyhow!("LanceDB query failed: {e}"))?;
        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| anyhow!("LanceDB collect failed: {e}"))?;

        let mut results = Vec::new();
        for batch in &batches {
            let event_ids = batch
                .column_by_name("event_id")
                .ok_or_else(|| anyhow!("missing event_id column"))?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow!("event_id is not StringArray"))?;

            let distances = batch
                .column_by_name("_distance")
                .ok_or_else(|| anyhow!("missing _distance column"))?
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| anyhow!("_distance is not Float32Array"))?;

            for i in 0..batch.num_rows() {
                let event_id = event_ids.value(i).to_string();
                if exclude.contains(&event_id) {
                    continue;
                }
                // LanceDB cosine distance == 1 - cosine_similarity.
                let sim = 1.0 - distances.value(i);
                if sim < min_score {
                    continue;
                }
                results.push(SearchResult {
                    id: event_id,
                    score: sim,
                });
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(top_k);
        Ok(results)
    }

    async fn delete(&self, ids: &[String]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        // Build an IN(...) predicate; chunk to keep query strings reasonable.
        const CHUNK: usize = 500;
        let before = self
            .table
            .count_rows(None)
            .await
            .map_err(|e| anyhow!("count before delete failed: {e}"))?;

        for chunk in ids.chunks(CHUNK) {
            let list = chunk
                .iter()
                .map(|id| format!("'{}'", id.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(",");
            let predicate = format!("event_id IN ({list})");
            self.table
                .delete(&predicate)
                .await
                .map_err(|e| anyhow!("LanceDB delete failed: {e}"))?;
        }

        let after = self
            .table
            .count_rows(None)
            .await
            .map_err(|e| anyhow!("count after delete failed: {e}"))?;

        Ok((before - after) as u64)
    }

    async fn exists(&self, id: &str) -> Result<bool> {
        let count = self
            .table
            .count_rows(Some(format!("event_id = '{}'", id.replace('\'', "''"))))
            .await
            .map_err(|e| anyhow!("LanceDB exists check failed: {e}"))?;
        Ok(count > 0)
    }

    async fn count(&self) -> Result<i64> {
        let count = self
            .table
            .count_rows(None)
            .await
            .map_err(|e| anyhow!("LanceDB count failed: {e}"))?;
        Ok(count as i64)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    async fn load_all(&self) -> Result<Vec<StoredEmbedding>> {
        let stream = self
            .table
            .query()
            .execute()
            .await
            .map_err(|e| anyhow!("LanceDB load_all query failed: {e}"))?;
        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| anyhow!("LanceDB load_all collect failed: {e}"))?;

        let mut all = Vec::new();
        for batch in &batches {
            all.extend(Self::batch_to_stored(batch)?);
        }
        Ok(all)
    }

    async fn load_recent(&self, limit: usize) -> Result<Vec<StoredEmbedding>> {
        // LanceDB has no native ORDER BY ... LIMIT on non-indexed columns.
        // For the cold-start use case we only call this with a bounded
        // `hot_limit` (default 1000), and `created_at` is rfc3339 which is
        // lexicographically ordered, so in-memory sort is acceptable.
        let all = self.load_all().await?;
        let mut sorted = all;
        sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        sorted.truncate(limit);
        Ok(sorted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(id: &str, vec: Vec<f32>) -> VectorRecord {
        VectorRecord {
            id: id.to_string(),
            vector: vec,
            session_key: "sess-a".to_string(),
            event_type: "user_message".to_string(),
            content_hash: "hash".to_string(),
        }
    }

    /// Wrapper that keeps the temp directory alive for the lifetime of the
    /// test store.
    struct TestStore {
        store: LanceVectorStore,
        _tmp: tempfile::TempDir,
    }

    impl std::ops::Deref for TestStore {
        type Target = LanceVectorStore;
        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }

    async fn test_store(dim: usize) -> TestStore {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let store = LanceVectorStore::open(&path, "test_vectors", dim)
            .await
            .unwrap();
        TestStore { store, _tmp: tmp }
    }

    #[tokio::test]
    async fn test_upsert_and_count() {
        let store = test_store(3).await;
        assert_eq!(store.count().await.unwrap(), 0);

        store
            .upsert(vec![sample_record("e1", vec![1.0, 0.0, 0.0])])
            .await
            .unwrap();
        assert_eq!(store.count().await.unwrap(), 1);

        store
            .upsert(vec![sample_record("e1", vec![1.0, 0.0, 0.0])])
            .await
            .unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_exists() {
        let store = test_store(3).await;
        assert!(!store.exists("e1").await.unwrap());

        store
            .upsert(vec![sample_record("e1", vec![1.0, 0.0, 0.0])])
            .await
            .unwrap();
        assert!(store.exists("e1").await.unwrap());
    }

    #[tokio::test]
    async fn test_delete() {
        let store = test_store(3).await;
        store
            .upsert(vec![
                sample_record("e1", vec![1.0, 0.0, 0.0]),
                sample_record("e2", vec![0.0, 1.0, 0.0]),
            ])
            .await
            .unwrap();

        let deleted = store.delete(&["e1".to_string()]).await.unwrap();
        assert_eq!(deleted, 1);
        assert!(!store.exists("e1").await.unwrap());
        assert!(store.exists("e2").await.unwrap());
    }

    #[tokio::test]
    async fn test_search_cosine() {
        let store = test_store(3).await;
        store
            .upsert(vec![
                sample_record("e1", vec![1.0, 0.0, 0.0]),
                sample_record("e2", vec![0.0, 1.0, 0.0]),
                sample_record("e3", vec![0.9, 0.1, 0.0]),
            ])
            .await
            .unwrap();

        let results = store
            .search(&[1.0, 0.0, 0.0], 2, 0.0, &HashSet::new())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "e1");
        assert!(results[0].score > 0.99);
    }

    #[tokio::test]
    async fn test_search_excludes_ids() {
        let store = test_store(3).await;
        store
            .upsert(vec![
                sample_record("e1", vec![1.0, 0.0, 0.0]),
                sample_record("e2", vec![0.9, 0.1, 0.0]),
            ])
            .await
            .unwrap();

        let mut exclude = HashSet::new();
        exclude.insert("e1".to_string());
        let results = store
            .search(&[1.0, 0.0, 0.0], 5, 0.0, &exclude)
            .await
            .unwrap();
        assert!(results.iter().all(|r| r.id != "e1"));
    }

    #[tokio::test]
    async fn test_load_all() {
        let store = test_store(3).await;
        store
            .upsert(vec![
                sample_record("e1", vec![1.0, 0.0, 0.0]),
                sample_record("e2", vec![0.0, 1.0, 0.0]),
            ])
            .await
            .unwrap();

        let all = store.load_all().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_search_min_score_filter() {
        let store = test_store(3).await;
        store
            .upsert(vec![
                sample_record("e1", vec![1.0, 0.0, 0.0]),
                sample_record("e2", vec![0.0, 1.0, 0.0]),
            ])
            .await
            .unwrap();

        let results = store
            .search(&[1.0, 0.0, 0.0], 5, 0.5, &HashSet::new())
            .await
            .unwrap();
        assert!(results.iter().all(|r| r.score >= 0.5));
    }

    #[tokio::test]
    async fn test_ensure_index_below_threshold_noop() {
        let store = test_store(3).await;
        store
            .upsert(vec![sample_record("e1", vec![1.0, 0.0, 0.0])])
            .await
            .unwrap();
        let built = store.ensure_vector_index().await.unwrap();
        assert!(!built, "should not build index for tiny dataset");
    }
}
