//! Script tools module
//!
//! This module provides support for external script tools that can be
//! integrated into Gasket via YAML manifests. Scripts communicate via
//! either Simple (stdin/stdout JSON) or JSON-RPC 2.0 protocols.

pub mod manifest;

// Re-export primary types for convenience
pub use manifest::{Permission, RuntimeConfig, ScriptManifest, ScriptProtocol};
