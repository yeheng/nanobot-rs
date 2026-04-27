//! Brute-force vector index with soft-delete support.
//!
//! Uses cosine similarity for nearest-neighbor search. Adequate for up to ~100K entries.
//! The `instant-distance` HNSW crate is available but requires batch construction (no incremental
//! inserts), so this implementation uses brute-force as the MVP approach.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;

/// In-memory vector index with soft-delete (tombstone) support.
///
/// Maps external `event_id` strings to internal `u64` keys and stores raw float vectors.
/// Search returns results sorted by cosine similarity (highest first).
pub struct HnswIndex {
    vectors: RwLock<HashMap<u64, Vec<f32>>>,
    id_map: RwLock<HashMap<u64, String>>,
    reverse_map: RwLock<HashMap<String, u64>>,
    tombstones: RwLock<HashSet<String>>,
    next_id: AtomicU64,
}

impl HnswIndex {
    /// Construct an empty index.
    ///
    /// The `dim` parameter is accepted for API compatibility but not currently used
    /// by the brute-force implementation.
    pub fn new(_dim: usize) -> Self {
        Self {
            vectors: RwLock::new(HashMap::new()),
            id_map: RwLock::new(HashMap::new()),
            reverse_map: RwLock::new(HashMap::new()),
            tombstones: RwLock::new(HashSet::new()),
            next_id: AtomicU64::new(0),
        }
    }

    /// Insert a vector associated with the given `event_id`.
    ///
    /// If the `event_id` already exists, it is re-inserted (overwriting the old vector)
    /// and removed from the tombstone set.
    pub fn insert(&self, event_id: String, vector: Vec<f32>) {
        // If already present, reuse the existing internal key.
        if let Some(&existing_key) = self.reverse_map.read().get(&event_id) {
            self.vectors.write().insert(existing_key, vector);
            self.tombstones.write().remove(&event_id);
            return;
        }

        let key = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.vectors.write().insert(key, vector);
        self.id_map.write().insert(key, event_id.clone());
        self.reverse_map.write().insert(event_id.clone(), key);
        self.tombstones.write().remove(&event_id);
    }

    /// Search for the `k` nearest vectors to the given query.
    ///
    /// Returns `(event_id, cosine_similarity)` pairs sorted by descending similarity.
    /// Tombstoned entries are excluded from results.
    pub fn search(&self, vector: &[f32], k: usize) -> Vec<(String, f32)> {
        let tombstones = self.tombstones.read();
        let vectors = self.vectors.read();
        let id_map = self.id_map.read();

        let mut scored: Vec<(u64, f32)> = vectors
            .iter()
            .filter_map(|(&key, v)| {
                let event_id = match id_map.get(&key) {
                    Some(id) => id,
                    None => return None,
                };
                if tombstones.contains(event_id) {
                    return None;
                }
                let sim = cosine_similarity(vector, v);
                Some((key, sim))
            })
            .collect();

        // Sort by descending similarity.
        scored.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);

        scored
            .into_iter()
            .map(|(key, sim)| {
                let event_id = id_map.get(&key).expect("key must exist in id_map").clone();
                (event_id, sim)
            })
            .collect()
    }

    /// Soft-delete an entry by adding it to the tombstone set.
    ///
    /// The vector remains in storage but is excluded from search results and `len()`.
    pub fn remove(&self, event_id: &str) {
        self.tombstones.write().insert(event_id.to_string());
    }

    /// Return the number of live (non-tombstoned) entries.
    pub fn len(&self) -> usize {
        let tombstones = self.tombstones.read();
        let vectors = self.vectors.read();
        vectors.len() - tombstones.len()
    }

    /// Return `true` if there are no live entries.
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
        let index = HnswIndex::new(3);
        index.insert("e1".into(), vec3(1.0, 0.0, 0.0));
        index.insert("e2".into(), vec3(0.0, 1.0, 0.0));

        let results = index.search(&vec3(1.0, 0.0, 0.0), 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "e1");
        assert!(results[0].1 > 0.99, "similarity should be close to 1.0");
    }

    #[test]
    fn test_remove_tombstone() {
        let index = HnswIndex::new(3);
        index.insert("e1".into(), vec3(1.0, 0.0, 0.0));
        index.insert("e2".into(), vec3(0.9, 0.1, 0.0));

        index.remove("e1");

        let results = index.search(&vec3(1.0, 0.0, 0.0), 2);
        assert!(
            results.iter().all(|(id, _)| id != "e1"),
            "tombstoned entry should not appear in search results"
        );
    }

    #[test]
    fn test_len_excludes_tombstones() {
        let index = HnswIndex::new(3);
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
        let index = HnswIndex::new(3);
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
        let index = HnswIndex::new(3);
        let results = index.search(&vec3(1.0, 0.0, 0.0), 5);
        assert!(results.is_empty());
    }
}
