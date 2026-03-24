//! Cryptographic helpers for WeCom callback message verification and decryption.
//!
//! Implements the WeCom message encryption scheme:
//! - Signature: `SHA1(sort([token, timestamp, nonce, msg_encrypt]))`
//! - Encryption: AES-256-CBC with IV = key\[0..16\], PKCS#7 padding (32-byte boundary)
//! - Plaintext layout: `random(16B) + msg_len(4B, big-endian) + msg + receiveid`

use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use sha1::{Digest, Sha1};

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// Compute WeCom msg_signature.
///
/// `dev_msg_signature = SHA1(sort([token, timestamp, nonce, msg_encrypt]))`
pub fn compute_signature(token: &str, timestamp: &str, nonce: &str, msg_encrypt: &str) -> String {
    let mut params = [token, timestamp, nonce, msg_encrypt];
    params.sort();
    let joined: String = params.concat();

    let mut hasher = Sha1::new();
    hasher.update(joined.as_bytes());
    let hash = hasher.finalize();

    // hex-encode (lowercase)
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Decode an EncodingAESKey (43-char base64 without padding) into a 32-byte AES key.
pub fn decode_aes_key(encoding_aes_key: &str) -> anyhow::Result<Vec<u8>> {
    // EncodingAESKey is 43 chars of base64; add `=` padding to make it valid.
    let padded = format!("{}=", encoding_aes_key);
    let key = BASE64.decode(&padded)?;
    if key.len() != 32 {
        anyhow::bail!("EncodingAESKey decoded to {} bytes, expected 32", key.len());
    }
    Ok(key)
}

/// Decrypt a WeCom encrypted message.
///
/// Algorithm: AES-256-CBC, IV = AESKey\[0..16\], PKCS#7 padding.
/// Plaintext layout: `random(16B) + msg_len(4B, big-endian) + msg + receiveid`
pub fn decrypt_message(
    aes_key: &[u8],
    ciphertext_b64: &str,
    expected_receiveid: &str,
) -> anyhow::Result<String> {
    let ciphertext = BASE64.decode(ciphertext_b64)?;

    if ciphertext.len() < 32 || ciphertext.len() % 16 != 0 {
        anyhow::bail!(
            "Invalid ciphertext length: {} (must be >= 32 and multiple of 16)",
            ciphertext.len()
        );
    }

    // IV = first 16 bytes of AES key
    let iv: &[u8; 16] = aes_key[..16]
        .try_into()
        .map_err(|_| anyhow::anyhow!("AES key too short for IV"))?;
    let key: &[u8; 32] = aes_key
        .try_into()
        .map_err(|_| anyhow::anyhow!("AES key must be 32 bytes"))?;

    // Decrypt with NoPadding — WeCom uses PKCS#7 padding to a 32-byte boundary
    // (not the standard 16-byte AES block size), so we must handle it manually.
    let mut buf = ciphertext.clone();
    let decrypted = Aes256CbcDec::new(key.into(), iv.into())
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .map_err(|e| anyhow::anyhow!("AES decryption failed: {}", e))?;

    // Remove PKCS#7 padding (32-byte boundary, per WeCom spec)
    if decrypted.is_empty() {
        anyhow::bail!("Decrypted data is empty");
    }
    let pad_len = *decrypted.last().unwrap() as usize;
    if pad_len == 0 || pad_len > 32 || pad_len > decrypted.len() {
        anyhow::bail!("Invalid PKCS#7 padding value: {}", pad_len);
    }
    let decrypted = &decrypted[..decrypted.len() - pad_len];

    // Parse: random(16) + msg_len(4) + msg + receiveid
    if decrypted.len() < 20 {
        anyhow::bail!(
            "Decrypted data too short: {} bytes (need at least 20)",
            decrypted.len()
        );
    }

    let msg_len = u32::from_be_bytes(decrypted[16..20].try_into().unwrap()) as usize;

    if 20 + msg_len > decrypted.len() {
        anyhow::bail!(
            "msg_len={} exceeds available data ({})",
            msg_len,
            decrypted.len() - 20
        );
    }

    let msg = &decrypted[20..20 + msg_len];
    let receiveid = &decrypted[20 + msg_len..];

    // Verify receiveid matches corpid
    if receiveid != expected_receiveid.as_bytes() {
        anyhow::bail!(
            "ReceiveId mismatch: expected '{}', got '{}'",
            expected_receiveid,
            String::from_utf8_lossy(receiveid)
        );
    }

    String::from_utf8(msg.to_vec())
        .map_err(|e| anyhow::anyhow!("Decrypted message is not valid UTF-8: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};

    type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

    /// Helper: encrypt a WeCom-style plaintext for testing.
    fn encrypt_wecom_payload(aes_key: &[u8], message: &str, receiveid: &str) -> String {
        let random_bytes = [0u8; 16];
        let msg_bytes = message.as_bytes();
        let msg_len = (msg_bytes.len() as u32).to_be_bytes();

        let mut plaintext = Vec::new();
        plaintext.extend_from_slice(&random_bytes);
        plaintext.extend_from_slice(&msg_len);
        plaintext.extend_from_slice(msg_bytes);
        plaintext.extend_from_slice(receiveid.as_bytes());

        // PKCS#7 pad to 32-byte boundary
        let pad_len = 32 - (plaintext.len() % 32);
        plaintext.extend(std::iter::repeat_n(pad_len as u8, pad_len));

        let iv: &[u8; 16] = aes_key[..16].try_into().unwrap();
        let key: &[u8; 32] = aes_key.try_into().unwrap();

        let mut buf = plaintext.clone();
        let buf_len = buf.len();
        Aes256CbcEnc::new(key.into(), iv.into())
            .encrypt_padded_mut::<NoPadding>(&mut buf, buf_len)
            .unwrap();

        BASE64.encode(&buf)
    }

    #[test]
    fn test_compute_signature() {
        let sig = compute_signature("token", "1234567890", "nonce", "encrypt");
        assert_eq!(sig.len(), 40); // SHA1 hex = 40 chars
    }

    #[test]
    fn test_decode_aes_key_valid() {
        // 43 base64 chars → 32 bytes
        let encoding_aes_key = "YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoxMjM0NTY";
        let key = decode_aes_key(encoding_aes_key).unwrap();
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn test_decode_aes_key_invalid_length() {
        let result = decode_aes_key("dG9vc2hvcnQ"); // "tooshort" in base64
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_message_roundtrip() {
        let aes_key_b64 = "YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoxMjM0NTY";
        let aes_key = decode_aes_key(aes_key_b64).unwrap();
        let receiveid = "test_corp_id";
        let message = "Hello, WeCom!";

        let ciphertext_b64 = encrypt_wecom_payload(&aes_key, message, receiveid);
        let decrypted = decrypt_message(&aes_key, &ciphertext_b64, receiveid).unwrap();
        assert_eq!(decrypted, message);
    }

    #[test]
    fn test_decrypt_message_wrong_receiveid() {
        let aes_key_b64 = "YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoxMjM0NTY";
        let aes_key = decode_aes_key(aes_key_b64).unwrap();

        let ciphertext_b64 = encrypt_wecom_payload(&aes_key, "Hello!", "test_corp_id");
        let result = decrypt_message(&aes_key, &ciphertext_b64, "wrong_corpid");
        assert!(result.is_err());
    }
}
