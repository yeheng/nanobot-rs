//! HNSW-based vector index with soft-delete (tombstone) support.
//!
//! Uses the `instant-distance` crate for approximate nearest-neighbor search.
//! The HNSW graph is rebuilt lazily on the first `search()` after any insert/remove.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::RwLock;

/// A single entry in the index.
struct Entry {
    id: u64,
    event_id: String,
    vector: Vec<f32>,
}

/// Wrapper around a float vector so it can be used with `instant-distance`.
#[derive(Clone)]
struct EmbeddingPoint {
    vec: Vec<f32>,
}

impl instant_distance::Point for EmbeddingPoint {
    /// Cosine distance: 1 - cosine_similarity.
    ///
    /// HNSW expects smaller values to mean "closer".
    fn distance(&self, other: &Self) -> f32 {
        1.0 - cosine_similarity(&self.vec, &other.vec)
    }
}

/// In-memory vector index backed by `instant-distance` HNSW.
///
/// Features:
/// - Approximate nearest-neighbor search (O(log N) instead of O(N))
/// - Soft-delete via tombstones
/// - Lazy rebuild: HNSW graph is only reconstructed when dirty
pub struct HnswIndex {
    /// All entries ever inserted (including overwritten ones).
    entries: RwLock<Vec<Entry>>,
    /// Cached HNSW graph. `None` when empty or dirty.
    hnsw: RwLock<Option<instant_distance::HnswMap<EmbeddingPoint, String>>>,
    /// Soft-deleted event IDs.
    tombstones: RwLock<HashSet<String>>,
    /// Maps external `event_id` → internal `u64` key.
    reverse_map: RwLock<HashMap<String, u64>>,
    /// Monotonic key generator.
    next_id: AtomicU64,
    /// Set to `true` whenever an insert or remove happens.
    dirty: AtomicBool,
}

impl HnswIndex {
    /// Construct an empty index.
    ///
    /// The `dim` parameter is accepted for API compatibility but not used
    /// by the HNSW implementation (the dimension is derived from vectors).
    pub fn new(_dim: usize) -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            hnsw: RwLock::new(None),
            tombstones: RwLock::new(HashSet::new()),
            reverse_map: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
        }
    }

    /// Insert a vector associated with the given `event_id`.
    ///
    /// If the `event_id` already exists, the old vector is overwritten
    /// and the entry is removed from the tombstone set.
    pub fn insert(&self, event_id: String, vector: Vec<f32>) {
        let mut reverse_map = self.reverse_map.write();
        if let Some(&existing_key) = reverse_map.get(&event_id) {
            // Overwrite existing entry in-place.
            let mut entries = self.entries.write();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == existing_key) {
                entry.vector = vector;
            }
            self.tombstones.write().remove(&event_id);
            self.dirty.store(true, Ordering::Relaxed);
            return;
        }

        let key = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.entries.write().push(Entry {
            id: key,
            event_id: event_id.clone(),
            vector,
        });
        reverse_map.insert(event_id.clone(), key);
        self.tombstones.write().remove(&event_id);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Search for the `k` nearest vectors to the given query.
    ///
    /// Returns `(event_id, cosine_similarity)` pairs sorted by descending similarity.
    /// Tombstoned entries are excluded from results.
    pub fn search(&self, vector: &[f32], k: usize) -> Vec<(String, f32)> {
        self.rebuild_if_needed();

        let hnsw_guard = self.hnsw.read();
        let hnsw = match hnsw_guard.as_ref() {
            Some(h) => h,
            None => return Vec::new(),
        };

        let query = EmbeddingPoint {
            vec: vector.to_vec(),
        };

        let mut search = instant_distance::Search::default();
        let raw = hnsw.search(&query, &mut search);

        raw.take(k)
            .map(|item| (item.value.clone(), 1.0 - item.distance))
            .collect()
    }

    /// Soft-delete an entry by adding it to the tombstone set.
    ///
    /// The vector remains in storage but is excluded from search results and `len()`.
    pub fn remove(&self, event_id: &str) {
        self.tombstones.write().insert(event_id.to_string());
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Return the number of live (non-tombstoned) entries.
    pub fn len(&self) -> usize {
        let tombstones = self.tombstones.read();
        let entries = self.entries.read();
        entries.len().saturating_sub(tombstones.len())
    }

    /// Return `true` if there are no live entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // -----------------------------------------------------------------------
    // Internal: lazy rebuild
    // -----------------------------------------------------------------------

    /// Rebuild the HNSW graph if the index is dirty.
    fn rebuild_if_needed(&self) {
        if !self.dirty.load(Ordering::Relaxed) {
            return;
        }

        let entries = self.entries.read();
        let tombstones = self.tombstones.read();

        let mut points = Vec::with_capacity(entries.len());
        let mut values = Vec::with_capacity(entries.len());

        for entry in entries.iter() {
            if tombstones.contains(&entry.event_id) {
                continue;
            }
            points.push(EmbeddingPoint {
                vec: entry.vector.clone(),
            });
            values.push(entry.event_id.clone());
        }

        let new_hnsw = if !points.is_empty() {
            // Use a generous ef_search so we don't miss top-k due to approximation.
            let builder = instant_distance::Builder::default().ef_search(200);
            Some(builder.build(points, values))
        } else {
            None
        };

        *self.hnsw.write() = new_hnsw;
        self.dirty.store(false, Ordering::Relaxed);
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
        assert!(
            results[0].1 > 0.95,
            "similarity should be close to 1.0, got {}",
            results[0].1
        );
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

    #[test]
    fn test_overwrite_reuses_key() {
        let index = HnswIndex::new(3);
        index.insert("e1".into(), vec3(1.0, 0.0, 0.0));
        index.insert("e1".into(), vec3(0.0, 1.0, 0.0));

        let results = index.search(&vec3(0.0, 1.0, 0.0), 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "e1");
        assert!(results[0].1 > 0.95);
    }
}
