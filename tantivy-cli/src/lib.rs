//! Tantivy CLI - Full-text search command line tool
//!
//! A command-line interface for managing Tantivy full-text search indexes.

pub mod error;
pub mod index;
pub mod maintenance;

pub use error::{Error, Result};
