//! Criterion benchmark: Argon2id verification latency.
//!
//! Measures the time to verify a Bearer token against an Argon2id PHC hash
//! using the same parameters as the production gateway (m=65536, t=1, p=1).
//! The acceptance criterion is ≤ 2 ms per verification on CI hardware.
//!
//! Note: benches/ cannot import from the binary crate, so the Argon2 logic
//! is inlined here using the same parameters as gateway::auth.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Algorithm, Argon2, Params, Version,
};
use base64ct::{Base64UrlUnpadded, Encoding};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Token generation parameters — must match gateway::auth constants.
const TOKEN_BYTES: usize = 32;
const ARGON2_MEMORY_KIB: u32 = 65_536;
const ARGON2_TIME_COST: u32 = 1;
const ARGON2_PARALLELISM: u32 = 1;

fn bench_argon2_verify(c: &mut Criterion) {
    // Generate a token and hash once; verification is what we are measuring.
    let raw_bytes: [u8; TOKEN_BYTES] = rand::random();
    let raw_token = Base64UrlUnpadded::encode_string(&raw_bytes);

    let salt = SaltString::generate(&mut OsRng);
    let params = Params::new(ARGON2_MEMORY_KIB, ARGON2_TIME_COST, ARGON2_PARALLELISM, None)
        .expect("valid Argon2 params");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let hash = argon2
        .hash_password(raw_token.as_bytes(), &salt)
        .expect("hash_password must succeed")
        .to_string();

    c.bench_function("argon2_verify", |b| {
        b.iter(|| {
            let parsed = PasswordHash::new(black_box(&hash)).expect("parse PHC hash");
            Argon2::default()
                .verify_password(black_box(raw_token.as_bytes()), &parsed)
                .is_ok()
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().measurement_time(std::time::Duration::from_secs(10));
    targets = bench_argon2_verify
}
criterion_main!(benches);
