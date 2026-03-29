//! Cryptographic operations for credential storage (AES-256-GCM).

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Cryptographic errors.
#[derive(Debug, Error)]
pub enum CryptoError {
    /// Decryption failed.
    #[error("decryption failed")]
    DecryptionFailed,
    /// Invalid key length (expected 32 bytes).
    #[error("invalid key length")]
    InvalidKeyLength,
    /// Encryption failed.
    #[error("encryption failed")]
    EncryptionFailed,
    /// The supplied string is not valid hexadecimal.
    #[error("invalid hex encoding")]
    InvalidHex,
}

/// A 32-byte AES-256-GCM encryption key that is zeroed from memory on drop.
///
/// The inner bytes are private and never exposed directly. Construct via
/// [`CryptoKey::from_bytes`] or [`CryptoKey::from_hex`].
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct CryptoKey([u8; 32]);

impl CryptoKey {
    /// Create a [`CryptoKey`] from a raw 32-byte array.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Create a [`CryptoKey`] from a 64-character hex string (upper or lower case).
    ///
    /// # Errors
    ///
    /// Returns [`CryptoError::InvalidHex`] if `hex_str` is not valid hexadecimal.
    /// Returns [`CryptoError::InvalidKeyLength`] if the decoded bytes are not exactly 32.
    pub fn from_hex(hex_str: &str) -> Result<Self, CryptoError> {
        let bytes = hex::decode(hex_str).map_err(|_| CryptoError::InvalidHex)?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| CryptoError::InvalidKeyLength)?;
        Ok(Self(arr))
    }
}

/// Encrypt plaintext using AES-256-GCM with a random IV.
///
/// Returns the IV (12 bytes) + ciphertext as a single byte vector.
///
/// # Arguments
///
/// * `key` - The encryption key.
/// * `plaintext` - The plaintext to encrypt.
///
/// # Returns
///
/// A vector containing the 12-byte IV followed by the ciphertext.
///
/// # Errors
///
/// Returns `CryptoError::EncryptionFailed` if encryption fails.
pub fn encrypt(key: &CryptoKey, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    // Generate a random 12-byte nonce.
    let mut nonce_bytes = [0u8; 12];
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let cipher = Aes256Gcm::new(&key.0.into());

    let ciphertext = cipher
        .encrypt(nonce, Payload::from(plaintext))
        .map_err(|_| CryptoError::EncryptionFailed)?;

    // Return IV || ciphertext.
    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypt a ciphertext encrypted with [`encrypt`].
///
/// Expects the input to be IV (12 bytes) + ciphertext. The returned plaintext
/// is wrapped in [`Zeroizing`] and is zeroed when dropped.
///
/// # Arguments
///
/// * `key` - The decryption key.
/// * `data` - The IV + ciphertext to decrypt.
///
/// # Returns
///
/// The plaintext on success, wrapped in [`Zeroizing`].
///
/// # Errors
///
/// Returns `CryptoError::DecryptionFailed` if the data is too short, has an
/// invalid format, or if AES-GCM authentication fails (tampered ciphertext).
pub fn decrypt(key: &CryptoKey, data: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if data.len() < 12 {
        return Err(CryptoError::DecryptionFailed);
    }

    let (nonce_bytes, ciphertext) = data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = Aes256Gcm::new(&key.0.into());

    let plaintext = cipher
        .decrypt(nonce, Payload::from(ciphertext))
        .map_err(|_| CryptoError::DecryptionFailed)?;

    Ok(Zeroizing::new(plaintext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = CryptoKey::from_bytes([0x42u8; 32]);
        let plaintext = b"hello world";

        let ciphertext = encrypt(&key, plaintext).expect("encrypt failed");
        let decrypted = decrypt(&key, &ciphertext).expect("decrypt failed");

        assert_eq!(decrypted.as_slice(), plaintext);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let key1 = CryptoKey::from_bytes([0x42u8; 32]);
        let key2 = CryptoKey::from_bytes([0x43u8; 32]);
        let plaintext = b"hello world";

        let ciphertext = encrypt(&key1, plaintext).expect("encrypt failed");
        let result = decrypt(&key2, &ciphertext);

        assert!(matches!(result, Err(CryptoError::DecryptionFailed)));
    }

    #[test]
    fn test_decrypt_with_tampered_ciphertext_fails() {
        let key = CryptoKey::from_bytes([0x42u8; 32]);
        let plaintext = b"hello world";

        let mut ciphertext = encrypt(&key, plaintext).expect("encrypt failed");
        // Tamper with the ciphertext (not the IV).
        if ciphertext.len() > 12 {
            ciphertext[12] ^= 0xFF;
        }

        let result = decrypt(&key, &ciphertext);
        assert!(matches!(result, Err(CryptoError::DecryptionFailed)));
    }

    #[test]
    fn test_encrypt_generates_different_ivs() {
        let key = CryptoKey::from_bytes([0x42u8; 32]);
        let plaintext = b"hello world";

        let ciphertext1 = encrypt(&key, plaintext).expect("encrypt 1 failed");
        let ciphertext2 = encrypt(&key, plaintext).expect("encrypt 2 failed");

        // The IVs should be different (first 12 bytes).
        assert_ne!(&ciphertext1[..12], &ciphertext2[..12]);
    }

    #[test]
    fn test_decrypt_with_too_short_data() {
        let key = CryptoKey::from_bytes([0x42u8; 32]);
        let short_data = [0u8; 11];
        let result = decrypt(&key, &short_data);
        assert!(matches!(result, Err(CryptoError::DecryptionFailed)));
    }

    #[test]
    fn test_crypto_key_from_bytes_roundtrip() {
        let raw = [0xABu8; 32];
        let key = CryptoKey::from_bytes(raw);
        // Verify encrypt/decrypt still works (key bytes preserved).
        let plaintext = b"key from bytes test";
        let ct = encrypt(&key, plaintext).expect("encrypt");
        let pt = decrypt(&key, &ct).expect("decrypt");
        assert_eq!(pt.as_slice(), plaintext);
    }

    #[test]
    fn test_crypto_key_from_hex_valid() {
        // 64 hex chars = 32 bytes.
        let hex = "a".repeat(64);
        let result = CryptoKey::from_hex(&hex);
        assert!(result.is_ok(), "expected Ok for valid 32-byte hex key");
    }

    #[test]
    fn test_crypto_key_from_hex_invalid_chars() {
        let result = CryptoKey::from_hex("not-valid-hex!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
        assert!(matches!(result, Err(CryptoError::InvalidHex)));
    }

    #[test]
    fn test_crypto_key_from_hex_wrong_length() {
        // 62 hex chars = 31 bytes (not 32).
        let hex = "a".repeat(62);
        let result = CryptoKey::from_hex(&hex);
        assert!(matches!(result, Err(CryptoError::InvalidKeyLength)));
    }

    #[test]
    fn test_crypto_key_from_hex_encrypt_decrypt() {
        // A known valid 32-byte hex key.
        let hex = "4242424242424242424242424242424242424242424242424242424242424242";
        let key = CryptoKey::from_hex(hex).expect("from_hex failed");
        let plaintext = b"from hex key test";
        let ct = encrypt(&key, plaintext).expect("encrypt");
        let pt = decrypt(&key, &ct).expect("decrypt");
        assert_eq!(pt.as_slice(), plaintext);
    }

    #[test]
    fn test_decrypt_result_is_zeroizing() {
        // Confirm the return type is Zeroizing<Vec<u8>> (compile-time check via type annotation).
        let key = CryptoKey::from_bytes([0x01u8; 32]);
        let ct = encrypt(&key, b"zeroize me").expect("encrypt");
        let pt: Zeroizing<Vec<u8>> = decrypt(&key, &ct).expect("decrypt");
        assert_eq!(pt.as_slice(), b"zeroize me");
    }
}
