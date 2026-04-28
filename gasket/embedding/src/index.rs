//! Simple in-memory vector index with linear scan.
//!
//! For the expected data size (< 100k vectors), brute-force cosine similarity
//! is faster than HNSW rebuild overhead and has zero maintenance cost.

use parking_lot::RwLock;

/// A single entry in the index.
struct Entry {
    event_id: String,
    vector: Vec<f32>,
}

/// In-memory vector index backed by a flat array and linear scan.
///
/// All operations are protected by a single RwLock — no deadlocks, no
/// stale state between tables.
pub struct MemoryIndex {
    inner: RwLock<IndexData>,
}

struct IndexData {
    entries: Vec<Entry>,
}

impl MemoryIndex {
    /// Construct an empty index.
    pub fn new(_dim: usize) -> Self {
        Self {
            inner: RwLock::new(IndexData {
                entries: Vec::new(),
            }),
        }
    }

    /// Insert a vector associated with the given `event_id`.
    ///
    /// If the `event_id` already exists, the old vector is overwritten.
    pub fn insert(&self, event_id: String, vector: Vec<f32>) {
        let mut data = self.inner.write();
        if let Some(entry) = data.entries.iter_mut().find(|e| e.event_id == event_id) {
            entry.vector = vector;
        } else {
            data.entries.push(Entry { event_id, vector });
        }
    }

    /// Search for the `k` nearest vectors to the given query.
    ///
    /// Returns `(event_id, cosine_similarity)` pairs sorted by descending similarity.
    pub fn search(&self, vector: &[f32], k: usize) -> Vec<(String, f32)> {
        let data = self.inner.read();
        let mut scored: Vec<(String, f32)> = data
            .entries
            .iter()
            .map(|e| (e.event_id.clone(), cosine_similarity(vector, &e.vector)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
    }

    /// Remove an entry by `event_id`.
    pub fn remove(&self, event_id: &str) {
        let mut data = self.inner.write();
        if let Some(pos) = data.entries.iter().position(|e| e.event_id == event_id) {
            data.entries.swap_remove(pos);
        }
    }

    /// Return the number of live entries.
    pub fn len(&self) -> usize {
        self.inner.read().entries.len()
    }

    /// Return `true` if there are no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Compute cosine similarity between two vectors.
///
/// Returns a value in `[-1.0, 1.0]`. Returns `0.0` for zero-length vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
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
    fn test_remove_tombstone() {
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
    fn test_len_excludes_tombstones() {
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
        // e1 should be first (most similar), e2 second.
        assert_eq!(results[0].0, "e1");
        assert_eq!(results[1].0, "e2");
        // Results should be in descending similarity order.
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
}
