//! Global broker singleton — infrastructure-level access.

use std::sync::{Arc, OnceLock};

use crate::MemoryBroker;

static GLOBAL_BROKER: OnceLock<Arc<MemoryBroker>> = OnceLock::new();

/// Initialize the global broker. Idempotent — subsequent calls are no-ops.
pub fn init_broker(broker: MemoryBroker) {
    let _ = GLOBAL_BROKER.set(Arc::new(broker));
}

/// Get a static reference to the global broker. Panics if not initialized.
pub fn get_broker() -> &'static MemoryBroker {
    let arc = GLOBAL_BROKER
        .get()
        .expect("get_broker called before init_broker");
    &**arc
}

/// Get an `Arc` handle to the global broker. Panics if not initialized.
///
/// Use this when a component's API requires `Arc<MemoryBroker>`.
/// The Arc is cheap to clone (just an atomic refcount increment).
pub fn broker_arc() -> Arc<MemoryBroker> {
    GLOBAL_BROKER
        .get()
        .expect("broker_arc called before init_broker")
        .clone()
}
