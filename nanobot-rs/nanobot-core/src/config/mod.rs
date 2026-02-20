//! Configuration management
//!
//! Compatible with Python nanobot config format (`~/.nanobot/config.yaml`)

mod loader;
mod schema;

pub use loader::{config_dir, config_path, load_config, ConfigLoader};
pub use schema::*;
