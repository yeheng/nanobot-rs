//! Global database singleton — initialized once in main.

use std::sync::OnceLock;

use crate::SqliteStore;

static GLOBAL_DB: OnceLock<SqliteStore> = OnceLock::new();

/// Initialize the global database. Idempotent — subsequent calls are no-ops.
pub fn init_db(db: SqliteStore) {
    let _ = GLOBAL_DB.set(db);
}

/// Get a reference to the global database. Panics if not initialized.
pub fn get_db() -> &'static SqliteStore {
    GLOBAL_DB.get().expect("get_db called before init_db")
}
