//! Global configuration singleton — read-only after init.

use std::sync::OnceLock;

use super::app_config::Config;

static GLOBAL_CONFIG: OnceLock<Config> = OnceLock::new();

/// Initialize the global config. Idempotent — subsequent calls are no-ops.
pub fn init_config(config: Config) {
    let _ = GLOBAL_CONFIG.set(config);
}

/// Get a reference to the global config. Panics if not initialized.
pub fn get_config() -> &'static Config {
    GLOBAL_CONFIG
        .get()
        .expect("get_config called before init_config")
}
