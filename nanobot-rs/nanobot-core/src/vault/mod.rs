//! Vault: Sensitive data isolation module
//!
//! Provides secure storage and runtime injection for sensitive data.
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
//! use nanobot_core::vault::{VaultStore, VaultInjector};
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

mod crypto;
mod error;
mod injector;
mod redaction;
mod scanner;
mod store;

pub use crypto::{EncryptedData, KdfParams, VaultCrypto};
pub use error::VaultError;
pub use injector::{InjectionReport, VaultInjector};
pub use redaction::{contains_secrets, redact_message_secrets, redact_secrets};
pub use scanner::{
    contains_placeholders, extract_keys, replace_placeholders, scan_placeholders, Placeholder,
};
pub use store::{AtomicTimestamp, VaultEntryV2, VaultFileV2, VaultMetadata, VaultStore};
