//! In-memory LRU credential cache.
//!
//! Caches encrypted credential blobs (`IV || ciphertext`), `auth_type`, and
//! `key_name` keyed by `server_id`. Decryption happens on every cache hit —
//! the cache stores encrypted bytes only, so the raw key is never held in the
//! hot-path data structure.
//!
//! **Lock discipline**: [`CredentialCache`] wraps [`std::sync::Mutex`].
//! Callers **MUST NOT** hold the lock guard across `.await` points. The
//! correct pattern is:
//!
//! ```rust,ignore
//! let maybe_value = {
//!     let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
//!     guard.get(&key).cloned()
//! }; // guard dropped here — safe to .await after this point
//! ```

use std::num::NonZeroUsize;
use std::sync::Mutex;

use lru::LruCache;
use uuid::Uuid;

/// Maximum number of credential entries to retain in the LRU cache.
pub const CACHE_CAPACITY: usize = 10_000;

/// A single cached credential entry.
///
/// The `encrypted_payload` field holds the raw `IV (12 bytes) || AES-256-GCM
/// ciphertext` blob from the `credentials.encrypted_payload` database column.
/// Decryption is performed on every request from this cached ciphertext.
#[derive(Clone)]
pub struct CachedCredential {
    /// Raw `IV || ciphertext` bytes. Decrypted on every request; never stored
    /// as plaintext.
    pub encrypted_payload: Vec<u8>,
    /// Authentication type: one of `bearer`, `api_key_header`, `api_key_query`,
    /// or `basic`.
    pub auth_type: String,
    /// Header / query-parameter name for `api_key_header` / `api_key_query`
    /// credential types. `None` for `bearer` and `basic`.
    pub key_name: Option<String>,
}

/// Thread-safe LRU credential cache, keyed by `server_id`.
///
/// Uses [`std::sync::Mutex`] (not `tokio::sync::Mutex`) because the critical
/// section is always trivially short — a single `get` or `put` call — and
/// `std::sync::Mutex` must **never** be held across `.await`.
pub type CredentialCache = Mutex<LruCache<Uuid, CachedCredential>>;

/// Constructs a new [`CredentialCache`] with [`CACHE_CAPACITY`] entries.
///
/// Uses the constant capacity directly — the conversion to `NonZeroUsize` is
/// safe because `CACHE_CAPACITY` is a positive compile-time constant.
pub fn new_cache() -> CredentialCache {
    let cap = match NonZeroUsize::new(CACHE_CAPACITY) {
        Some(n) => n,
        None => NonZeroUsize::MIN,
    };
    Mutex::new(LruCache::new(cap))
}
