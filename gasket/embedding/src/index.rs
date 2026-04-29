//! In-memory vector index with linear scan over a contiguous buffer.
//!
//! For the expected data size (< 100k vectors) brute-force similarity is
//! faster than HNSW rebuild overhead and has zero maintenance cost. Vectors
//! are stored unit-normalized so similarity == dot product, and IDs are
//! refcounted (`Arc<str>`) to keep search hot paths allocation-free.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

/// In-memory vector index backed by a flat row-major buffer and a hash map
/// from id to row.
pub struct MemoryIndex {
    inner: RwLock<IndexData>,
    dim: usize,
}

struct IndexData {
    /// Row-major buffer, `len() == ids.len() * dim`. Each row is unit-norm.
    vectors: Vec<f32>,
    /// Parallel array: `ids[i]` is the id stored at row `i`.
    ids: Vec<Arc<str>>,
    /// Reverse lookup id → row index.
    by_id: HashMap<Arc<str>, usize>,
}

impl MemoryIndex {
    /// Construct an empty index with the given embedding dimension.
    pub fn new(dim: usize) -> Self {
        assert!(dim > 0, "MemoryIndex dim must be > 0");
        Self {
            inner: RwLock::new(IndexData {
                vectors: Vec::new(),
                ids: Vec::new(),
                by_id: HashMap::new(),
            }),
            dim,
        }
    }

    /// Embedding dimension this index was created with.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Insert (or overwrite) a vector. The vector is unit-normalized in-place
    /// before being stored, so subsequent `search` calls compute cosine
    /// similarity via plain dot product.
    pub fn insert(&self, event_id: String, mut vector: Vec<f32>) {
        if vector.len() != self.dim {
            tracing::warn!(
                "MemoryIndex::insert: skipping event {} — wrong dim ({}, expected {})",
                event_id,
                vector.len(),
                self.dim
            );
            return;
        }
        normalize_in_place(&mut vector);

        let mut data = self.inner.write();
        let id: Arc<str> = Arc::from(event_id.into_boxed_str());
        if let Some(&row) = data.by_id.get(&id) {
            let start = row * self.dim;
            data.vectors[start..start + self.dim].copy_from_slice(&vector);
        } else {
            let row = data.ids.len();
            data.vectors.extend_from_slice(&vector);
            data.ids.push(id.clone());
            data.by_id.insert(id, row);
        }
    }

    /// Search for the `k` nearest vectors to the given query.
    ///
    /// Returns `(event_id, cosine_similarity)` sorted by descending similarity.
    /// Wrong-dim queries return an empty result.
    pub fn search(&self, query: &[f32], k: usize) -> Vec<(String, f32)> {
        if k == 0 || query.len() != self.dim {
            return Vec::new();
        }
        let mut q = query.to_vec();
        normalize_in_place(&mut q);

        let data = self.inner.read();
        let n = data.ids.len();
        if n == 0 {
            return Vec::new();
        }

        // Top-k via a small fixed-size buffer; for typical k (5-20) this is
        // faster and simpler than a heap.
        let cap = k.min(n);
        let mut top: Vec<(f32, usize)> = Vec::with_capacity(cap);
        for row in 0..n {
            let start = row * self.dim;
            let v = &data.vectors[start..start + self.dim];
            let score = dot(&q, v);
            if top.len() < cap {
                top.push((score, row));
            } else {
                // Replace the weakest if this one is stronger.
                let (min_idx, &(min_score, _)) = top
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1 .0.partial_cmp(&b.1 .0).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap();
                if score > min_score {
                    top[min_idx] = (score, row);
                }
            }
        }

        top.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        top.into_iter()
            .map(|(score, row)| (data.ids[row].as_ref().to_owned(), score))
            .collect()
    }

    /// Remove an entry by id. O(1) average.
    pub fn remove(&self, event_id: &str) {
        let mut data = self.inner.write();
        let key: Arc<str> = Arc::from(event_id);
        let Some(row) = data.by_id.remove(&key) else {
            return;
        };
        let last = data.ids.len() - 1;
        if row != last {
            // Swap-remove: move last row into the freed slot.
            let dim = self.dim;
            let (front, back) = data.vectors.split_at_mut((last) * dim);
            front[row * dim..row * dim + dim].copy_from_slice(&back[..dim]);
            data.ids.swap(row, last);
            let moved_id = data.ids[row].clone();
            data.by_id.insert(moved_id, row);
        }
        data.vectors.truncate(last * self.dim);
        data.ids.pop();
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.inner.read().ids.len()
    }

    /// True if there are no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Compute cosine similarity between two vectors.
///
/// Returns a value in `[-1.0, 1.0]`. Returns `0.0` for zero-length vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot_p = dot(a, b);
    let norm_a = dot(a, a).sqrt();
    let norm_b = dot(b, b).sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot_p / (norm_a * norm_b)
}

/// Unit-normalize a vector in place. Zero vectors are left untouched.
pub fn normalize_in_place(v: &mut [f32]) {
    let norm = dot(v, v).sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return;
    }
    let inv = 1.0 / norm;
    for x in v.iter_mut() {
        *x *= inv;
    }
}

#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec3(a: f32, b: f32, c: f32) -> Vec<f32> {
        vec![a, b, c]
    }

    #[test]
    fn test_insert_and_search() {
        let index = MemoryIndex::new(3);
        index.insert("e1".into(), vec3(1.0, 0.0, 0.0));
        index.insert("e2".into(), vec3(0.0, 1.0, 0.0));

        let results = index.search(&vec3(1.0, 0.0, 0.0), 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "e1");
        assert!(
            results[0].1 > 0.95,
            "similarity should be close to 1.0, got {}",
            results[0].1
        );
    }

    #[test]
    fn test_remove() {
        let index = MemoryIndex::new(3);
        index.insert("e1".into(), vec3(1.0, 0.0, 0.0));
        index.insert("e2".into(), vec3(0.9, 0.1, 0.0));

        index.remove("e1");

        let results = index.search(&vec3(1.0, 0.0, 0.0), 2);
        assert!(
            results.iter().all(|(id, _)| id != "e1"),
            "removed entry should not appear in search results"
        );
    }

    #[test]
    fn test_remove_first_then_search() {
        let index = MemoryIndex::new(3);
        index.insert("e1".into(), vec3(1.0, 0.0, 0.0));
        index.insert("e2".into(), vec3(0.0, 1.0, 0.0));
        index.insert("e3".into(), vec3(0.0, 0.0, 1.0));

        index.remove("e1");
        assert_eq!(index.len(), 2);

        let results = index.search(&vec3(0.0, 1.0, 0.0), 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "e2");
    }

    #[test]
    fn test_len() {
        let index = MemoryIndex::new(3);
        index.insert("e1".into(), vec3(1.0, 0.0, 0.0));
        index.insert("e2".into(), vec3(0.0, 1.0, 0.0));
        assert_eq!(index.len(), 2);

        index.remove("e1");
        assert_eq!(index.len(), 1);

        index.remove("e2");
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn test_search_returns_multiple_results() {
        let index = MemoryIndex::new(3);
        index.insert("e1".into(), vec3(1.0, 0.0, 0.0));
        index.insert("e2".into(), vec3(0.9, 0.1, 0.0));
        index.insert("e3".into(), vec3(0.0, 0.0, 1.0));

        let results = index.search(&vec3(1.0, 0.0, 0.0), 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "e1");
        assert_eq!(results[1].0, "e2");
        assert!(results[0].1 >= results[1].1);
    }

    #[test]
    fn test_empty_search() {
        let index = MemoryIndex::new(3);
        let results = index.search(&vec3(1.0, 0.0, 0.0), 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_overwrite_reuses_key() {
        let index = MemoryIndex::new(3);
        index.insert("e1".into(), vec3(1.0, 0.0, 0.0));
        index.insert("e1".into(), vec3(0.0, 1.0, 0.0));

        let results = index.search(&vec3(0.0, 1.0, 0.0), 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "e1");
        assert!(results[0].1 > 0.95);
    }

    #[test]
    fn test_wrong_dim_insert_skipped() {
        let index = MemoryIndex::new(3);
        index.insert("bad".into(), vec![1.0, 0.0]); // wrong dim
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn test_wrong_dim_query_returns_empty() {
        let index = MemoryIndex::new(3);
        index.insert("e1".into(), vec3(1.0, 0.0, 0.0));
        let results = index.search(&[1.0, 0.0], 1);
        assert!(results.is_empty());
    }

    #[test]
    fn test_normalize_does_not_break_zero_vectors() {
        let mut v = vec![0.0, 0.0, 0.0];
        normalize_in_place(&mut v);
        assert_eq!(v, vec![0.0, 0.0, 0.0]);
    }
}
