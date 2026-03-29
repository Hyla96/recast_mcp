//! Cryptographic operations for credential storage (AES-256-GCM).

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use thiserror::Error;

/// Cryptographic errors.
#[derive(Debug, Error)]
pub enum CryptoError {
    /// Decryption failed.
    #[error("decryption failed")]
    DecryptionFailed,
    /// Invalid key length.
    #[error("invalid key length")]
    InvalidKeyLength,
    /// Encryption failed.
    #[error("encryption failed")]
    EncryptionFailed,
}

/// Encrypt plaintext using AES-256-GCM with a random IV.
///
/// Returns the IV (12 bytes) + ciphertext as a single byte vector.
///
/// # Arguments
///
/// * `key` - The 32-byte encryption key.
/// * `plaintext` - The plaintext to encrypt.
///
/// # Returns
///
/// A vector containing the 12-byte IV followed by the ciphertext.
///
/// # Errors
///
/// Returns `CryptoError::InvalidKeyLength` if the key is not 32 bytes.
/// Returns `CryptoError::EncryptionFailed` if encryption fails.
pub fn encrypt(key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if key.len() != 32 {
        return Err(CryptoError::InvalidKeyLength);
    }

    // Generate a random 12-byte nonce
    let mut nonce_bytes = [0u8; 12];
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Create cipher
    let key_array: [u8; 32] = key.try_into().map_err(|_| CryptoError::InvalidKeyLength)?;
    let cipher = Aes256Gcm::new(&key_array.into());

    // Encrypt
    let ciphertext = cipher
        .encrypt(nonce, Payload::from(plaintext))
        .map_err(|_| CryptoError::EncryptionFailed)?;

    // Return IV + ciphertext
    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypt a ciphertext encrypted with `encrypt()`.
///
/// Expects the input to be IV (12 bytes) + ciphertext.
///
/// # Arguments
///
/// * `key` - The 32-byte decryption key.
/// * `data` - The IV + ciphertext to decrypt.
///
/// # Returns
///
/// The plaintext on success.
///
/// # Errors
///
/// Returns `CryptoError::InvalidKeyLength` if the key is not 32 bytes.
/// Returns `CryptoError::DecryptionFailed` if the data is too short, invalid format, or decryption fails.
pub fn decrypt(key: &[u8], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if key.len() != 32 {
        return Err(CryptoError::InvalidKeyLength);
    }

    if data.len() < 12 {
        return Err(CryptoError::DecryptionFailed);
    }

    // Extract nonce and ciphertext
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    // Create cipher
    let key_array: [u8; 32] = key.try_into().map_err(|_| CryptoError::InvalidKeyLength)?;
    let cipher = Aes256Gcm::new(&key_array.into());

    // Decrypt
    cipher
        .decrypt(nonce, Payload::from(ciphertext))
        .map_err(|_| CryptoError::DecryptionFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [0x42u8; 32];
        let plaintext = b"hello world";

        let ciphertext = encrypt(&key, plaintext).expect("encrypt failed");
        let decrypted = decrypt(&key, &ciphertext).expect("decrypt failed");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let key1 = [0x42u8; 32];
        let key2 = [0x43u8; 32];
        let plaintext = b"hello world";

        let ciphertext = encrypt(&key1, plaintext).expect("encrypt failed");
        let result = decrypt(&key2, &ciphertext);

        assert!(matches!(result, Err(CryptoError::DecryptionFailed)));
    }

    #[test]
    fn test_decrypt_with_tampered_ciphertext_fails() {
        let key = [0x42u8; 32];
        let plaintext = b"hello world";

        let mut ciphertext = encrypt(&key, plaintext).expect("encrypt failed");
        // Tamper with the ciphertext (not the IV)
        if ciphertext.len() > 12 {
            ciphertext[12] ^= 0xFF;
        }

        let result = decrypt(&key, &ciphertext);
        assert!(matches!(result, Err(CryptoError::DecryptionFailed)));
    }

    #[test]
    fn test_encrypt_generates_different_ivs() {
        let key = [0x42u8; 32];
        let plaintext = b"hello world";

        let ciphertext1 = encrypt(&key, plaintext).expect("encrypt 1 failed");
        let ciphertext2 = encrypt(&key, plaintext).expect("encrypt 2 failed");

        // The IVs should be different (first 12 bytes)
        assert_ne!(&ciphertext1[..12], &ciphertext2[..12]);
    }
}
