use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    P0 = 0, // Real-time (new messages)
    P1 = 1, // Incremental (memory writes)
    P2 = 2, // Batch (backfill, compaction)
}

pub struct IndexingQueue<T> {
    p0_tx: mpsc::Sender<T>,
    p0_rx: mpsc::Receiver<T>,
    p1_tx: mpsc::Sender<T>,
    p1_rx: mpsc::Receiver<T>,
    p2_tx: mpsc::Sender<T>,
    p2_rx: mpsc::Receiver<T>,
    depth: Arc<AtomicUsize>,
    max_depth: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("Queue is full")]
    Full,
}

impl<T> IndexingQueue<T> {
    pub fn new(max_depth: usize) -> Self {
        let per_queue = max_depth / 3;
        let (p0_tx, p0_rx) = mpsc::channel(per_queue);
        let (p1_tx, p1_rx) = mpsc::channel(per_queue);
        let (p2_tx, p2_rx) = mpsc::channel(per_queue);

        Self {
            p0_tx,
            p0_rx,
            p1_tx,
            p1_rx,
            p2_tx,
            p2_rx,
            depth: Arc::new(AtomicUsize::new(0)),
            max_depth,
        }
    }

    pub async fn push(&self, item: T, priority: Priority) -> Result<(), QueueError> {
        let tx = match priority {
            Priority::P0 => &self.p0_tx,
            Priority::P1 => &self.p1_tx,
            Priority::P2 => &self.p2_tx,
        };

        match tx.try_send(item) {
            Ok(_) => {
                self.depth.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(_) => Err(QueueError::Full),
        }
    }

    pub async fn pop(&mut self) -> Option<T> {
        let result = tokio::select! {
            biased;
            item = self.p0_rx.recv() => item,
            item = self.p1_rx.recv() => item,
            item = self.p2_rx.recv() => item,
        };

        if result.is_some() {
            self.depth.fetch_sub(1, Ordering::Relaxed);
        }

        result
    }

    pub fn depth(&self) -> usize {
        self.depth.load(Ordering::Relaxed)
    }

    pub fn max_depth(&self) -> usize {
        self.max_depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn priority_ordering_p0_before_p1_and_p2() {
        let mut queue = IndexingQueue::<&str>::new(300);

        // Push in reverse priority order
        queue.push("low", Priority::P2).await.unwrap();
        queue.push("mid", Priority::P1).await.unwrap();
        queue.push("high", Priority::P0).await.unwrap();

        // P0 should come out first
        assert_eq!(Some("high"), queue.pop().await);
        assert_eq!(Some("mid"), queue.pop().await);
        assert_eq!(Some("low"), queue.pop().await);
    }

    #[tokio::test]
    async fn depth_tracking() {
        let mut queue = IndexingQueue::<i32>::new(300);

        assert_eq!(0, queue.depth());

        queue.push(1, Priority::P0).await.unwrap();
        queue.push(2, Priority::P1).await.unwrap();
        queue.push(3, Priority::P2).await.unwrap();

        assert_eq!(3, queue.depth());

        queue.pop().await;
        assert_eq!(2, queue.depth());

        queue.pop().await;
        queue.pop().await;
        assert_eq!(0, queue.depth());
    }

    #[tokio::test]
    async fn queue_full_returns_error() {
        // max_depth=3 → per_queue=1 (each priority channel holds 1 item)
        let mut queue = IndexingQueue::<i32>::new(3);

        assert!(queue.push(1, Priority::P0).await.is_ok());
        // Second push to P0 should fail (channel capacity = 1)
        assert!(matches!(
            queue.push(2, Priority::P0).await,
            Err(QueueError::Full)
        ));
    }

    #[tokio::test]
    async fn fifo_ordering_within_same_priority() {
        let mut queue = IndexingQueue::<&str>::new(300);

        queue.push("first", Priority::P1).await.unwrap();
        queue.push("second", Priority::P1).await.unwrap();

        assert_eq!(Some("first"), queue.pop().await);
        assert_eq!(Some("second"), queue.pop().await);
    }
}
