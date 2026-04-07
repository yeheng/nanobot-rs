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
