//! Vault encryption module
//!
//! Provides encryption/decryption using XChaCha20-Poly1305 ( AEAD )
//! and key derivation using Argon2id.
//!
//! # Security Properties
//!
//! - **Encryption**: XChaCha20-Poly1305 (AEAD) - provides confidentiality and integrity
//! - **Key Derivation**: Argon2id - memory-hard KDF resistant to GPU/ASIC attacks
//! - **Nonce**: 24-byte random nonce for each encryption (never reuse nonce with same key)
//! - **Memory Safety**: Sensitive data is zeroized on drop using the `zeroize` crate
//!
//! # Usage
//!
//! ```ignore
//! use nanobot_core::vault::crypto::{VaultCrypto, KdfParams};
//!
//! // Derive key from password
//! let params = KdfParams::default();
//! let crypto = VaultCrypto::derive_key("my-password", &params)?;
//!
//! // Encrypt
//! let encrypted = crypto.encrypt("secret-value")?;
//!
//! // Decrypt
//! let decrypted = crypto.decrypt(&encrypted.ciphertext, &encrypted.nonce)?;
//! assert_eq!(decrypted, "secret-value");
//! ```

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::ZeroizeOnDrop;

use super::VaultError;

/// Argon2id KDF parameters
///
/// These parameters control the memory and CPU hardness of key derivation.
/// Higher values = more security but slower.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    /// Algorithm name (always "argon2id")
    pub algorithm: String,
    /// Salt for key derivation (32 bytes, base64 encoded in storage)
    #[serde(with = "salt_base64")]
    pub salt: [u8; 32],
    /// Memory cost in KiB (default: 64 MB)
    pub memory_cost: u32,
    /// Time cost / iterations (default: 3)
    pub time_cost: u32,
    /// Parallelism (default: 4)
    pub parallelism: u32,
}

mod salt_base64 {
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(salt: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BASE64_STANDARD.encode(salt).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = String::deserialize(deserializer)?;
        let decoded = BASE64_STANDARD
            .decode(&s)
            .map_err(|e| serde::de::Error::custom(format!("Invalid base64 salt: {}", e)))?;
        if decoded.len() != 32 {
            return Err(serde::de::Error::custom(format!(
                "Salt must be 32 bytes, got {}",
                decoded.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&decoded);
        Ok(arr)
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        let mut salt = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut salt);

        Self {
            algorithm: "argon2id".to_string(),
            salt,
            memory_cost: 65536, // 64 MB
            time_cost: 3,
            parallelism: 4,
        }
    }
}

impl KdfParams {
    /// Create new KDF parameters with a random salt
    pub fn new() -> Self {
        Self::default()
    }

    /// Create KDF parameters with a specific salt (for testing or loading)
    pub fn with_salt(salt: [u8; 32]) -> Self {
        Self {
            algorithm: "argon2id".to_string(),
            salt,
            memory_cost: 65536,
            time_cost: 3,
            parallelism: 4,
        }
    }
}

/// Encrypted data result
#[derive(Debug, Clone)]
pub struct EncryptedData {
    /// Encrypted ciphertext (base64 encoded for storage)
    pub ciphertext: Vec<u8>,
    /// Nonce used for encryption (24 bytes for XChaCha20)
    pub nonce: [u8; 24],
}

/// Vault cryptography handler
///
/// Holds the derived encryption key and provides encrypt/decrypt operations.
/// The key is zeroized when this struct is dropped.
#[derive(ZeroizeOnDrop)]
pub struct VaultCrypto {
    /// Derived encryption key (32 bytes for ChaCha20)
    key: [u8; 32],
}

impl VaultCrypto {
    /// Derive an encryption key from a password using Argon2id
    ///
    /// # Arguments
    ///
    /// * `password` - The master password
    /// * `params` - KDF parameters including salt
    ///
    /// # Returns
    ///
    /// A `VaultCrypto` instance with the derived key, or an error
    pub fn derive_key(password: &str, params: &KdfParams) -> Result<Self, VaultError> {
        use argon2::{Algorithm, Argon2, Params, Version};

        let params_builder = Params::new(
            params.memory_cost, // m_cost
            params.time_cost,   // t_cost
            params.parallelism, // p_cost
            Some(32),           // output length (32 bytes for ChaCha20)
        )
        .map_err(|e| VaultError::Encryption(format!("Failed to create Argon2 params: {}", e)))?;

        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params_builder);

        let mut key = [0u8; 32];
        argon2
            .hash_password_into(password.as_bytes(), &params.salt, &mut key)
            .map_err(|e| {
                VaultError::Encryption(format!("Argon2id key derivation failed: {}", e))
            })?;

        Ok(Self { key })
    }

    /// Create a VaultCrypto from an existing key (for testing)
    #[cfg(test)]
    pub fn from_key(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// Encrypt plaintext using XChaCha20-Poly1305
    ///
    /// Generates a random 24-byte nonce for each encryption.
    ///
    /// # Arguments
    ///
    /// * `plaintext` - The data to encrypt
    ///
    /// # Returns
    ///
    /// An `EncryptedData` struct containing ciphertext and nonce
    pub fn encrypt(&self, plaintext: &str) -> Result<EncryptedData, VaultError> {
        let cipher = XChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|e| VaultError::Encryption(format!("Failed to initialize cipher: {}", e)))?;

        // Generate random 24-byte nonce
        let mut nonce_bytes = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);

        // Encrypt with AEAD
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| VaultError::Encryption(format!("Encryption failed: {}", e)))?;

        Ok(EncryptedData {
            ciphertext,
            nonce: nonce_bytes,
        })
    }

    /// Decrypt ciphertext using XChaCha20-Poly1305
    ///
    /// # Arguments
    ///
    /// * `ciphertext` - The encrypted data
    /// * `nonce` - The 24-byte nonce used during encryption
    ///
    /// # Returns
    ///
    /// The decrypted plaintext string
    pub fn decrypt(&self, ciphertext: &[u8], nonce: &[u8; 24]) -> Result<String, VaultError> {
        let cipher = XChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|e| VaultError::Encryption(format!("Failed to initialize cipher: {}", e)))?;

        let nonce = XNonce::from_slice(nonce);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| VaultError::Encryption(format!("Decryption failed: {}", e)))?;

        String::from_utf8(plaintext)
            .map_err(|e| VaultError::Encryption(format!("Invalid UTF-8 in decrypted data: {}", e)))
    }

    /// Encrypt plaintext and return base64-encoded strings for storage
    ///
    /// Convenience method that returns base64-encoded ciphertext and nonce.
    pub fn encrypt_to_base64(&self, plaintext: &str) -> Result<(String, String), VaultError> {
        let encrypted = self.encrypt(plaintext)?;
        Ok((
            BASE64_STANDARD.encode(&encrypted.ciphertext),
            BASE64_STANDARD.encode(encrypted.nonce),
        ))
    }

    /// Decrypt base64-encoded ciphertext
    ///
    /// Convenience method that accepts base64-encoded ciphertext and nonce.
    pub fn decrypt_from_base64(
        &self,
        ciphertext_b64: &str,
        nonce_b64: &str,
    ) -> Result<String, VaultError> {
        let ciphertext = BASE64_STANDARD
            .decode(ciphertext_b64)
            .map_err(|e| VaultError::Encryption(format!("Invalid base64 ciphertext: {}", e)))?;

        let nonce_bytes = BASE64_STANDARD
            .decode(nonce_b64)
            .map_err(|e| VaultError::Encryption(format!("Invalid base64 nonce: {}", e)))?;

        if nonce_bytes.len() != 24 {
            return Err(VaultError::Encryption(format!(
                "Nonce must be 24 bytes, got {}",
                nonce_bytes.len()
            )));
        }

        let mut nonce = [0u8; 24];
        nonce.copy_from_slice(&nonce_bytes);

        self.decrypt(&ciphertext, &nonce)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kdf_params_default() {
        let params = KdfParams::default();
        assert_eq!(params.algorithm, "argon2id");
        assert_eq!(params.memory_cost, 65536);
        assert_eq!(params.time_cost, 3);
        assert_eq!(params.parallelism, 4);
        // Salt should be non-zero (very unlikely to be all zeros)
        assert!(params.salt.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_derive_key() {
        let params = KdfParams::default();
        let crypto = VaultCrypto::derive_key("test-password", &params).unwrap();

        // Derive again with same params - should get same key
        let crypto2 = VaultCrypto::derive_key("test-password", &params).unwrap();

        // Test by encrypting with one and decrypting with another
        let encrypted = crypto.encrypt("secret").unwrap();
        let decrypted = crypto2
            .decrypt(&encrypted.ciphertext, &encrypted.nonce)
            .unwrap();
        assert_eq!(decrypted, "secret");
    }

    #[test]
    fn test_derive_key_different_passwords() {
        let params = KdfParams::default();
        let crypto1 = VaultCrypto::derive_key("password1", &params).unwrap();
        let crypto2 = VaultCrypto::derive_key("password2", &params).unwrap();

        let encrypted = crypto1.encrypt("secret").unwrap();

        // Decrypting with wrong key should fail
        let result = crypto2.decrypt(&encrypted.ciphertext, &encrypted.nonce);
        assert!(result.is_err());
    }

    #[test]
    fn test_derive_key_different_salts() {
        let params1 = KdfParams::default();
        let params2 = KdfParams::default();

        let crypto1 = VaultCrypto::derive_key("password", &params1).unwrap();
        let crypto2 = VaultCrypto::derive_key("password", &params2).unwrap();

        let encrypted = crypto1.encrypt("secret").unwrap();

        // Different salt = different key, should fail to decrypt
        let result = crypto2.decrypt(&encrypted.ciphertext, &encrypted.nonce);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let params = KdfParams::default();
        let crypto = VaultCrypto::derive_key("test-password", &params).unwrap();

        let plaintext = "my-super-secret-api-key-12345";
        let encrypted = crypto.encrypt(plaintext).unwrap();

        assert_ne!(encrypted.ciphertext, plaintext.as_bytes());
        assert_eq!(encrypted.nonce.len(), 24);

        let decrypted = crypto
            .decrypt(&encrypted.ciphertext, &encrypted.nonce)
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_decrypt_base64() {
        let params = KdfParams::default();
        let crypto = VaultCrypto::derive_key("test-password", &params).unwrap();

        let plaintext = "my-super-secret-api-key";
        let (ciphertext_b64, nonce_b64) = crypto.encrypt_to_base64(plaintext).unwrap();

        // Verify base64 encoding
        assert!(BASE64_STANDARD.decode(&ciphertext_b64).is_ok());
        assert!(BASE64_STANDARD.decode(&nonce_b64).is_ok());

        let decrypted = crypto
            .decrypt_from_base64(&ciphertext_b64, &nonce_b64)
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_multiple_encryptions_different_nonces() {
        let params = KdfParams::default();
        let crypto = VaultCrypto::derive_key("test-password", &params).unwrap();

        let plaintext = "same-plaintext";
        let encrypted1 = crypto.encrypt(plaintext).unwrap();
        let encrypted2 = crypto.encrypt(plaintext).unwrap();

        // Same plaintext should produce different ciphertext due to random nonce
        assert_ne!(encrypted1.ciphertext, encrypted2.ciphertext);
        assert_ne!(encrypted1.nonce, encrypted2.nonce);

        // Both should decrypt to the same plaintext
        assert_eq!(
            crypto
                .decrypt(&encrypted1.ciphertext, &encrypted1.nonce)
                .unwrap(),
            plaintext
        );
        assert_eq!(
            crypto
                .decrypt(&encrypted2.ciphertext, &encrypted2.nonce)
                .unwrap(),
            plaintext
        );
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let params = KdfParams::default();
        let crypto = VaultCrypto::derive_key("test-password", &params).unwrap();

        let encrypted = crypto.encrypt("secret").unwrap();

        // Tamper with ciphertext
        let mut tampered = encrypted.ciphertext.clone();
        if !tampered.is_empty() {
            tampered[0] ^= 0xFF;
        }

        let result = crypto.decrypt(&tampered, &encrypted.nonce);
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_nonce_fails() {
        let params = KdfParams::default();
        let crypto = VaultCrypto::derive_key("test-password", &params).unwrap();

        let encrypted = crypto.encrypt("secret").unwrap();

        // Use wrong nonce
        let mut wrong_nonce = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut wrong_nonce);

        let result = crypto.decrypt(&encrypted.ciphertext, &wrong_nonce);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_plaintext() {
        let params = KdfParams::default();
        let crypto = VaultCrypto::derive_key("test-password", &params).unwrap();

        let encrypted = crypto.encrypt("").unwrap();
        let decrypted = crypto
            .decrypt(&encrypted.ciphertext, &encrypted.nonce)
            .unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_unicode_plaintext() {
        let params = KdfParams::default();
        let crypto = VaultCrypto::derive_key("test-password", &params).unwrap();

        let plaintext = "秘密密码 🔐 パスワード";
        let encrypted = crypto.encrypt(plaintext).unwrap();
        let decrypted = crypto
            .decrypt(&encrypted.ciphertext, &encrypted.nonce)
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
