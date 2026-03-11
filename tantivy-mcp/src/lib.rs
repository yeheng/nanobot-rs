//! Tantivy MCP - Standalone MCP Index Server
//!
//! A Model Context Protocol (MCP) server providing full-text search capabilities
//! using the Tantivy search engine. This is a completely independent project
//! with zero dependencies on nanobot.

pub mod error;
pub mod index;
pub mod maintenance;
pub mod mcp;
mod tools;

pub use error::{Error, Result};
pub use tools::register_tools;
