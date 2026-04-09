pub mod crypto;
pub mod error;
pub mod redaction;
pub mod scanner;
pub mod store;

// Re-export key types for convenience
pub use crypto::{EncryptedData, KdfParams, VaultCrypto};
pub use error::VaultError;
pub use redaction::{contains_secrets, redact_message_secrets, redact_secrets};
pub use scanner::{
    contains_placeholders, extract_keys, replace_placeholders, scan_placeholders, Placeholder,
};
pub use store::{AtomicTimestamp, VaultEntryV2, VaultFileV2, VaultMetadata, VaultStore};
