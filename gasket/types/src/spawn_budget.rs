//! Global concurrency budget for subagent spawning.

use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Orchestrator-session-scoped budget for the number of concurrently running workers.
///
/// All spawn paths (`spawn` / `spawn_parallel` / future dispatch tools) must
/// `acquire()` a permit before launching a worker. The returned permit is owned
/// by the worker's tokio task; it is released automatically when the task ends,
/// which limits concurrent inflight workers (not start-rate).
#[derive(Clone, Debug)]
pub struct SpawnBudget {
    semaphore: Arc<Semaphore>,
    max_concurrency: usize,
}

impl SpawnBudget {
    /// Creates a budget with `max_concurrency` permits. Values < 1 are clamped to 1
    /// to prevent a permanent deadlock.
    pub fn new(max_concurrency: usize) -> Self {
        let n = max_concurrency.max(1);
        Self {
            semaphore: Arc::new(Semaphore::new(n)),
            max_concurrency: n,
        }
    }

    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    /// Acquires a permit. The returned permit MUST be moved into the
    /// spawned worker's tokio task; on drop it returns the permit.
    pub async fn acquire(&self) -> OwnedSemaphorePermit {
        // The Semaphore is held inside this Budget's Arc and is never closed.
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("SpawnBudget semaphore unexpectedly closed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn clamps_zero_to_one() {
        assert_eq!(SpawnBudget::new(0).max_concurrency(), 1);
    }

    #[test]
    fn preserves_positive_values() {
        assert_eq!(SpawnBudget::new(3).max_concurrency(), 3);
    }

    #[tokio::test]
    async fn permit_released_on_drop() {
        let b = SpawnBudget::new(1);
        let p = b.acquire().await;
        drop(p);
        // Should not block:
        let _ = tokio::time::timeout(Duration::from_millis(100), b.acquire())
            .await
            .expect("acquire should succeed after drop");
    }

    #[tokio::test]
    async fn caps_concurrent_inflight() {
        let b = SpawnBudget::new(2);
        let inflight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for _ in 0..5 {
            let b = b.clone();
            let inflight = inflight.clone();
            let peak = peak.clone();
            handles.push(tokio::spawn(async move {
                let _permit = b.acquire().await;
                let cur = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                inflight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }
}
