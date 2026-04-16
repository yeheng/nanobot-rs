//! Pure-Rust vector math: cosine similarity and top-K retrieval.
//!
//! No C extensions, no SIMD intrinsics — just plain iterators that the
//! compiler auto-vectorises under `-C opt-level=2`.  384-dim dot products
//! run in single-digit nanoseconds on modern CPUs.

/// Cosine similarity between two vectors.
///
/// Returns a value in `[-1.0, 1.0]` where `1.0` means identical direction.
/// A small epsilon (`1e-8`) prevents division by zero when either vector
/// is the zero vector.
#[inline]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "vector dimensions must match");

    let (mut dot, mut norm_a, mut norm_b) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt()).max(1e-8)
}

/// Find the top-K most similar items from a set of candidates.
///
/// Returns `(identifier, score)` pairs sorted by **descending** similarity.
/// Ties are broken arbitrarily (unstable sort for speed).
pub fn top_k_similar<'a>(
    query: &[f32],
    candidates: &'a [(String, Vec<f32>)],
    k: usize,
) -> Vec<(&'a str, f32)> {
    let mut scores: Vec<(&str, f32)> = candidates
        .iter()
        .map(|(name, vec)| (name.as_str(), cosine_similarity(query, vec)))
        .collect();

    if scores.len() > k {
        scores.select_nth_unstable_by(k, |a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
        scores.truncate(k);
    }
    scores.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scores
}

// ── bytemuck helpers for BLOB serialisation (requires local-embedding feature) ──

/// Convert a float slice to a byte slice (zero-copy).
///
/// Only available when the `local-embedding` feature is enabled.
#[cfg(feature = "local-embedding")]
#[inline]
pub fn embedding_to_bytes(embedding: &[f32]) -> &[u8] {
    bytemuck::cast_slice(embedding)
}

/// Convert a byte slice back to a float slice (zero-copy).
///
/// Only available when the `local-embedding` feature is enabled.
///
/// # Panics
///
/// Panics if `bytes.len()` is not a multiple of 4 (i.e. not aligned to `f32`).
#[cfg(feature = "local-embedding")]
#[inline]
pub fn bytes_to_embedding(bytes: &[u8]) -> &[f32] {
    bytemuck::cast_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_orthogonal_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn test_opposite_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_zero_vector_does_not_panic() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_top_k_order() {
        let query = vec![1.0, 0.0, 0.0];
        let candidates = vec![
            ("east".to_string(), vec![1.0, 0.0, 0.0]),
            ("north".to_string(), vec![0.0, 1.0, 0.0]),
            ("northeast".to_string(), vec![0.707, 0.707, 0.0]),
        ];

        let top = top_k_similar(&query, &candidates, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "east");
        assert!((top[0].1 - 1.0).abs() < 1e-3);
        assert_eq!(top[1].0, "northeast");
    }

    #[test]
    fn test_top_k_empty_candidates() {
        let query = vec![1.0, 0.0];
        let candidates: Vec<(String, Vec<f32>)> = vec![];
        let top = top_k_similar(&query, &candidates, 5);
        assert!(top.is_empty());
    }

    #[test]
    #[cfg(feature = "local-embedding")]
    fn test_bytemuck_roundtrip() {
        let original = vec![0.1f32, 0.2, 0.3, 0.4, 0.5];
        let bytes = embedding_to_bytes(&original);
        let recovered = bytes_to_embedding(bytes);
        assert_eq!(original.as_slice(), recovered);
    }

    #[test]
    #[cfg(feature = "local-embedding")]
    fn test_bytemuck_empty() {
        let empty: Vec<f32> = vec![];
        let bytes = embedding_to_bytes(&empty);
        assert!(bytes.is_empty());
        let recovered = bytes_to_embedding(bytes);
        assert!(recovered.is_empty());
    }
}
