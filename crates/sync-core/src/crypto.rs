//! End-to-end encryption primitives (DESIGN.md §19.7, Phase Sk).
//!
//! Authenticated symmetric encryption for the sync layer's
//! payloads. The user supplies a passphrase; we derive a 32-byte
//! key with Argon2id and use it to AES-256-GCM-encrypt every log
//! file, snapshot body, and sound asset before it leaves the
//! device. The sync storage only ever sees ciphertext.
//!
//! ## What's encrypted
//!
//! - Log file bytes (the JSONL stream of events)
//! - Snapshot body (the typed state dump)
//! - Sound assets (binary audio)
//!
//! ## What's NOT encrypted
//!
//! `meta.json` is **always plaintext** even with E2E enabled —
//! per §19.7. Devices need to read `schema_version`,
//! `min_app_version`, `e2e_enabled`, and the device registry
//! BEFORE they could possibly prompt for a passphrase, so the
//! coordination file stays open.
//!
//! ## Wire format
//!
//! Encrypted blob layout (single concatenated byte buffer):
//!
//! ```text
//!   ┌──────────┬────────────────────────────────┐
//!   │ nonce    │ ciphertext+tag                 │
//!   │ 12 bytes │ plaintext.len() + 16 bytes     │
//!   └──────────┴────────────────────────────────┘
//! ```
//!
//! The 12-byte nonce is freshly random per encryption. AES-GCM's
//! `encrypt` appends a 16-byte authentication tag to the
//! ciphertext; we don't separate the two on the wire — the
//! decrypt side feeds the suffix back into `decrypt`.
//!
//! ## Argon2 parameters
//!
//! Defaults: `m_cost = 19456 KiB (≈19 MB)`, `t_cost = 2`,
//! `p_cost = 1`. These are the OWASP-recommended interactive
//! params for Argon2id (2023). Stored in [`EncryptionParams`]
//! alongside the salt so the same passphrase derives the same key
//! across devices and across app versions even if the defaults
//! shift in a future release.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::Argon2;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::error::{SyncError, SyncResult};

/// Length of an AES-256 key in bytes.
pub const KEY_LEN: usize = 32;
/// Length of an AES-GCM nonce in bytes (12 = 96 bits; standard).
pub const NONCE_LEN: usize = 12;
/// Length of a fresh KDF salt. 16 bytes ≈ 128 bits of randomness;
/// the recommendation from RFC 9106 (Argon2) is "at least 128 bits".
pub const SALT_LEN: usize = 16;

/// Default Argon2id memory cost in KiB (~19 MB). Recommended by
/// OWASP for interactive logins.
pub const DEFAULT_M_COST: u32 = 19_456;
/// Default Argon2id time cost (iterations).
pub const DEFAULT_T_COST: u32 = 2;
/// Default Argon2id parallelism. Keep at 1 so single-threaded
/// derivation is deterministic; bumping this gives no real benefit
/// for a 32-byte key.
pub const DEFAULT_P_COST: u32 = 1;

/// KDF parameters stored alongside the dataset in `meta.json`.
/// Every device that wants to join derives its key from the same
/// passphrase + the same salt + cost params, so the resulting
/// 32-byte key is identical across devices without ever
/// transmitting the key itself.
///
/// Stored as base64 strings on the wire because JSON has no
/// binary type. Cost params are plain integers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptionParams {
    /// Base64-encoded random salt. Generated once when E2E is
    /// enabled on the dataset; never changes for the life of the
    /// dataset (changing it would invalidate every device's
    /// derived key).
    pub salt: String,
    /// Argon2id memory cost in KiB. Default
    /// [`DEFAULT_M_COST`].
    pub m_cost: u32,
    /// Argon2id time cost (iterations). Default
    /// [`DEFAULT_T_COST`].
    pub t_cost: u32,
    /// Argon2id parallelism. Default [`DEFAULT_P_COST`].
    pub p_cost: u32,
}

impl EncryptionParams {
    /// Mint a fresh parameter set with default cost values + a
    /// random salt. Called once when the user enables E2E on a
    /// new dataset.
    pub fn fresh() -> Self {
        let mut salt = [0u8; SALT_LEN];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        Self {
            salt: BASE64.encode(salt),
            m_cost: DEFAULT_M_COST,
            t_cost: DEFAULT_T_COST,
            p_cost: DEFAULT_P_COST,
        }
    }

    fn salt_bytes(&self) -> SyncResult<Vec<u8>> {
        BASE64.decode(&self.salt).map_err(|err| {
            SyncError::protocol(format!("decode salt: {err}"))
        })
    }
}

/// Derive a 32-byte AES key from `passphrase` and the dataset's
/// Argon2 parameters. Deterministic — calling twice with the same
/// inputs returns the same key.
pub fn derive_key(
    passphrase: &str,
    params: &EncryptionParams,
) -> SyncResult<[u8; KEY_LEN]> {
    let salt = params.salt_bytes()?;
    let argon = Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(params.m_cost, params.t_cost, params.p_cost, None)
            .map_err(|err| {
                SyncError::internal(format!("argon2 params: {err}"))
            })?,
    );
    let mut out = [0u8; KEY_LEN];
    argon
        .hash_password_into(passphrase.as_bytes(), &salt, &mut out)
        .map_err(|err| SyncError::internal(format!("argon2 derive: {err}")))?;
    Ok(out)
}

/// Encrypt `plaintext` with the given key. Returns the wire blob
/// (nonce || ciphertext+tag).
///
/// A fresh random nonce is generated per call — never reuse a
/// nonce with the same key. AES-GCM's security guarantees break
/// catastrophically on nonce reuse, so we let the OS RNG mint one
/// for every encryption.
pub fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8]) -> SyncResult<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher.encrypt(nonce, plaintext).map_err(|err| {
        SyncError::internal(format!("AES-GCM encrypt: {err}"))
    })?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a wire blob produced by [`encrypt`].
///
/// Failures here typically mean **the key is wrong** — either the
/// user typed the wrong passphrase, or this device's key is from
/// a different dataset. We collapse all failure modes into
/// [`SyncError::Auth`] so the UI can present a single coherent
/// "wrong passphrase" message rather than leaking ciphertext
/// state.
pub fn decrypt(key: &[u8; KEY_LEN], blob: &[u8]) -> SyncResult<Vec<u8>> {
    if blob.len() < NONCE_LEN {
        return Err(SyncError::protocol(
            "encrypted blob shorter than nonce",
        ));
    }
    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher.decrypt(nonce, ciphertext).map_err(|_| {
        // Don't echo the underlying error — it might leak whether
        // the tag mismatch was at the start or end, which the
        // BSON-style attacks exploit.
        SyncError::auth(
            "decryption failed — wrong passphrase or corrupt blob",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_params_has_unique_salt() {
        let a = EncryptionParams::fresh();
        let b = EncryptionParams::fresh();
        assert_ne!(a.salt, b.salt);
        assert_eq!(a.m_cost, DEFAULT_M_COST);
        assert_eq!(a.t_cost, DEFAULT_T_COST);
        assert_eq!(a.p_cost, DEFAULT_P_COST);
    }

    #[test]
    fn derive_key_is_deterministic() {
        // Use a tiny cost set so the test runs fast.
        let params = EncryptionParams {
            salt: BASE64.encode([7u8; SALT_LEN]),
            m_cost: 64,
            t_cost: 1,
            p_cost: 1,
        };
        let k1 = derive_key("hunter2", &params).unwrap();
        let k2 = derive_key("hunter2", &params).unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn derive_key_changes_with_passphrase() {
        let params = EncryptionParams {
            salt: BASE64.encode([7u8; SALT_LEN]),
            m_cost: 64,
            t_cost: 1,
            p_cost: 1,
        };
        let k1 = derive_key("hunter2", &params).unwrap();
        let k2 = derive_key("hunter3", &params).unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn derive_key_changes_with_salt() {
        let p1 = EncryptionParams {
            salt: BASE64.encode([7u8; SALT_LEN]),
            m_cost: 64,
            t_cost: 1,
            p_cost: 1,
        };
        let p2 = EncryptionParams {
            salt: BASE64.encode([9u8; SALT_LEN]),
            m_cost: 64,
            t_cost: 1,
            p_cost: 1,
        };
        let k1 = derive_key("same", &p1).unwrap();
        let k2 = derive_key("same", &p2).unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn encrypt_decrypt_round_trips() {
        let key = [42u8; KEY_LEN];
        let plaintext = b"this is some sync event JSON";
        let blob = encrypt(&key, plaintext).unwrap();
        // Length: 12 (nonce) + plaintext + 16 (tag).
        assert_eq!(blob.len(), NONCE_LEN + plaintext.len() + 16);
        let recovered = decrypt(&key, &blob).unwrap();
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn encrypt_with_different_nonce_each_call() {
        // Two encryptions of the same plaintext with the same key
        // must produce different ciphertexts because the nonce is
        // freshly random. This is the core property AES-GCM
        // needs — nonce reuse breaks the security.
        let key = [1u8; KEY_LEN];
        let pt = b"identical payload";
        let a = encrypt(&key, pt).unwrap();
        let b = encrypt(&key, pt).unwrap();
        assert_ne!(a, b);
        // Both still decrypt back to the same plaintext.
        assert_eq!(decrypt(&key, &a).unwrap(), pt);
        assert_eq!(decrypt(&key, &b).unwrap(), pt);
    }

    #[test]
    fn decrypt_with_wrong_key_returns_auth_error() {
        let plaintext = b"private";
        let blob = encrypt(&[1u8; KEY_LEN], plaintext).unwrap();
        let err = decrypt(&[2u8; KEY_LEN], &blob).unwrap_err();
        assert!(
            matches!(err, SyncError::Auth(_)),
            "expected Auth, got: {err:?}",
        );
    }

    #[test]
    fn decrypt_with_corrupted_blob_returns_auth_error() {
        let key = [3u8; KEY_LEN];
        let mut blob = encrypt(&key, b"some payload").unwrap();
        // Flip a single ciphertext byte (after the 12-byte nonce).
        // AES-GCM's auth tag catches the tamper and refuses to
        // decrypt.
        blob[15] ^= 0x80;
        let err = decrypt(&key, &blob).unwrap_err();
        assert!(matches!(err, SyncError::Auth(_)));
    }

    #[test]
    fn decrypt_with_truncated_blob_returns_protocol_error() {
        let err = decrypt(&[0u8; KEY_LEN], &[0u8; 5]).unwrap_err();
        // Truncated blob — not even a nonce — is a malformed wire
        // value, not an auth failure. Different error variant so
        // the UI doesn't claim "wrong passphrase" for what's
        // actually a corrupted upload.
        assert!(matches!(err, SyncError::Protocol(_)));
    }

    #[test]
    fn params_round_trip_through_json() {
        let p = EncryptionParams::fresh();
        let s = serde_json::to_string(&p).unwrap();
        let back: EncryptionParams = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }
}
