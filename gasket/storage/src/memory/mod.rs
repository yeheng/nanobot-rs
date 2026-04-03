//! Long-term memory system for explicit knowledge persistence.
//!
//! This module provides types and utilities for managing explicit long-term memory
//! stored as Markdown files in `~/.gasket/memory/*.md`. Unlike SQLite (which stores
//! machine-state like sessions and events), memory files store user-curated knowledge:
//! facts, preferences, decisions, and reference material.
//!
//! ## Architecture
//!
//! - **Scenario-based organization:** Memories are organized into directories by scenario
//!   (profile, active, knowledge, decisions, episodes, reference)
//! - **Frequency-based decay:** Memories are tagged with access frequency (hot, warm, cold)
//!   for automated lifecycle management
//! - **Token budget tracking:** Each memory tracks its token count for budget enforcement
//! - **Supersession:** Old versions can reference their replacements for audit trails

mod types;

pub use types::*;
