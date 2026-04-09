//! Vault: Sensitive data isolation module
//!
//! Provides secure storage and runtime injection for sensitive data.
//!
//! This module provides the `VaultInjector` for injecting secrets into `ChatMessage` objects
//! and re-exports core types from the `core` submodule.
//!
//! # Design Principles
//!
//! 1. **Data Structure Isolation**: VaultStore is completely separated from memory/history
//! 2. **Runtime Injection**: Secrets are only injected at the last moment before sending to LLM
//! 3. **Zero Trust**: Sensitive data never persists to LLM-accessible storage
//!
//! # Usage
//!
//! ```ignore
//! use gasket_engine::vault::{VaultStore, VaultInjector};
//! use std::sync::Arc;
//!
//! // Create store
//! let store = Arc::new(VaultStore::new()?);
//! store.set("api_key", "sk-12345", Some("OpenAI API key"))?;
//!
//! // Create injector
//! let injector = VaultInjector::new(store);
//!
//! // Inject messages
//! let mut messages = vec![ChatMessage::user("Use {{vault:api_key}}")];
//! let report = injector.inject(&mut messages);
//! // messages[0].content == "Use sk-12345"
//! ```
//!
//! # Placeholder Format
//!
//! Use `{{vault:key_name}}` in your messages:
//!
//! ```text
//! "Connect to database with {{vault:db_password}}"
//! "API key: {{vault:openai_api_key}}"
//! "AWS credentials: {{vault:aws_access_key}} {{vault:aws_secret_key}}"
//! ```

pub mod core;
pub mod injector;

// Re-export from core for convenient access
pub use core::{
    contains_placeholders, contains_secrets, extract_keys, redact_message_secrets, redact_secrets,
    replace_placeholders, scan_placeholders, AtomicTimestamp, EncryptedData, KdfParams,
    Placeholder, VaultCrypto, VaultEntryV2, VaultError, VaultFileV2, VaultMetadata, VaultStore,
};

// Re-export injector types
pub use injector::{InjectionReport, VaultInjector};
