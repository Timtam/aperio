//! [`EncryptingAdapter`] — transparent E2E layer on top of any
//! [`SyncAdapter`] (DESIGN.md §19.7, Phase Sk).
//!
//! Wraps an inner adapter and adds AES-256-GCM encryption to
//! every payload that crosses the network. The orchestrator and
//! the rest of the sync engine see the same `SyncAdapter` API;
//! the encryption layer is invisible to them.
//!
//! ## What gets encrypted vs not
//!
//! | Method                | Behaviour                                                                |
//! |-----------------------|--------------------------------------------------------------------------|
//! | `test_connection`     | pass-through                                                             |
//! | `fetch_meta`          | pass-through (`meta.json` is **always plaintext** per §19.7)             |
//! | `push_meta`           | pass-through                                                             |
//! | `fetch_new_logs`      | decrypt each `LogFile.bytes` after fetch                                 |
//! | `push_log`            | encrypt `LogFile.bytes` before push                                       |
//! | `fetch_snapshot`      | decrypt the snapshot's body                                              |
//! | `push_snapshot`       | encrypt the snapshot's body                                              |
//! | `delete_log`          | pass-through                                                             |
//! | `push_sound_asset`    | encrypt the audio bytes                                                  |
//! | `fetch_sound_asset`   | decrypt the audio bytes                                                  |
//!
//! ## Snapshot body wrapping
//!
//! Snapshots have a typed [`SnapshotMetadata`] header + an
//! untyped `serde_json::Value` body. The body would expose
//! plaintext shapes to the storage adapter (it serialises the
//! Snapshot as JSON before writing). To hide the body without
//! changing the adapter trait, we wrap the encrypted ciphertext
//! into a sentinel object:
//!
//! ```jsonc
//! {
//!   "_aperio_encrypted_v1": "<base64 of encrypt(body_json_bytes)>"
//! }
//! ```
//!
//! `fetch_snapshot` unwraps it on the way out. Plaintext
//! snapshots (E2E disabled) pass through unchanged — the sentinel
//! key acts as the detector.

use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::{json, Value};

use crate::adapter::{DeviceCursor, SyncAdapter};
use crate::crypto::{decrypt, encrypt, KEY_LEN, NONCE_LEN};
use crate::error::{SyncError, SyncResult};

/// Constant size delta between a log file's plaintext and its encrypted
/// remote form: the prepended nonce plus AES-GCM's 16-byte auth tag (GCM
/// is a stream mode — no padding). Used to translate the cursor's
/// plaintext-domain applied lengths into the ciphertext domain the inner
/// adapter's listing reports.
const CIPHERTEXT_OVERHEAD: u64 = NONCE_LEN as u64 + 16;
use crate::log::{LogFile, LogFileName};
use crate::meta::MetaJson;
use crate::snapshot::Snapshot;

/// Sentinel key marking an encrypted snapshot body. Versioned so
/// a future re-cut of the format (e.g. compression-then-encrypt)
/// can coexist with v1 readers by bumping the suffix.
const ENCRYPTED_BODY_KEY: &str = "_aperio_encrypted_v1";

/// E2E-encryption wrapper around any [`SyncAdapter`]. Constructed
/// once after onboarding (or after the user enters their
/// passphrase on app start) and lives for the orchestrator's
/// lifetime.
///
/// Holds the raw 32-byte AES key. The wrapper is `Debug` but the
/// key field is intentionally excluded — we don't want it
/// landing in a `tracing` log accidentally.
pub struct EncryptingAdapter {
    inner: Arc<dyn SyncAdapter>,
    key: [u8; KEY_LEN],
}

impl std::fmt::Debug for EncryptingAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptingAdapter")
            .field("inner", &"<dyn SyncAdapter>")
            .field("key", &"<redacted>")
            .finish()
    }
}

impl EncryptingAdapter {
    pub fn new(inner: Arc<dyn SyncAdapter>, key: [u8; KEY_LEN]) -> Self {
        Self { inner, key }
    }

    /// Pull the inner adapter back out — used by tests + the
    /// command layer when it needs to issue a plaintext meta.json
    /// write without the encryption wrapper getting in the way
    /// (it doesn't, but the explicit handle makes intent clear).
    pub fn inner(&self) -> &Arc<dyn SyncAdapter> {
        &self.inner
    }
}

#[async_trait]
impl SyncAdapter for EncryptingAdapter {
    async fn test_connection(&self) -> SyncResult<()> {
        self.inner.test_connection().await
    }

    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        // meta.json is always plaintext (§19.7) — pass through.
        self.inner.fetch_meta().await
    }

    async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
        self.inner.push_meta(meta).await
    }

    async fn fetch_new_logs(&self, since: &DeviceCursor) -> SyncResult<Vec<LogFile>> {
        // Translate the cursor's applied lengths from the PLAINTEXT domain
        // (what the engine sees and records — this wrapper decrypts below)
        // into the CIPHERTEXT domain the inner adapter's listing reports.
        // AES-GCM adds a constant overhead (12-byte nonce + 16-byte tag,
        // no padding), so the translation is exact. Without it the
        // growth-refetch check compared plaintext lengths against
        // ciphertext sizes — always "grown" — and re-downloaded every
        // remembered file every round on an E2E dataset. (Lengths recorded
        // BEFORE E2E was enabled translate to plaintext+28, which is
        // exactly the unchanged file's encrypted size — so such files are
        // correctly SKIPPED until they genuinely grow, at which point the
        // encrypted size exceeds plaintext+28 and the re-fetch fires.)
        let translated = DeviceCursor {
            known_lengths: since
                .known_lengths
                .iter()
                .map(|k| crate::KnownLogLength {
                    name: k.name.clone(),
                    len: k.len + CIPHERTEXT_OVERHEAD,
                })
                .collect(),
            ..since.clone()
        };
        let raw = self.inner.fetch_new_logs(&translated).await?;
        let mut out = Vec::with_capacity(raw.len());
        for log in raw {
            let plaintext = decrypt(&self.key, &log.bytes)?;
            out.push(LogFile {
                name: log.name,
                bytes: plaintext,
            });
        }
        Ok(out)
    }

    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        let ciphertext = encrypt(&self.key, &log.bytes)?;
        let wrapped = LogFile {
            name: log.name.clone(),
            bytes: ciphertext,
        };
        self.inner.push_log(&wrapped).await
    }

    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
        let snapshot = match self.inner.fetch_snapshot().await? {
            Some(s) => s,
            None => return Ok(None),
        };
        // Look for the sentinel — a snapshot pushed by this
        // wrapper carries the encrypted-body marker. A snapshot
        // from a plaintext era (before E2E was enabled) lacks
        // the marker; pass it through unchanged so the user can
        // still read pre-E2E history if they downgrade.
        if let Some(blob_b64) = snapshot
            .body
            .get(ENCRYPTED_BODY_KEY)
            .and_then(Value::as_str)
        {
            let blob = BASE64.decode(blob_b64).map_err(|err| {
                SyncError::protocol(format!("decode encrypted snapshot body: {err}"))
            })?;
            let plaintext = decrypt(&self.key, &blob)?;
            let body: Value = serde_json::from_slice(&plaintext)?;
            Ok(Some(Snapshot {
                metadata: snapshot.metadata,
                body,
            }))
        } else {
            Ok(Some(snapshot))
        }
    }

    async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
        let body_bytes = serde_json::to_vec(&snapshot.body)?;
        let ciphertext = encrypt(&self.key, &body_bytes)?;
        let wrapped = Snapshot {
            metadata: snapshot.metadata.clone(),
            body: json!({ ENCRYPTED_BODY_KEY: BASE64.encode(&ciphertext) }),
        };
        self.inner.push_snapshot(&wrapped).await
    }

    async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
        self.inner.delete_log(name).await
    }

    async fn push_sound_asset(&self, hash: &str, extension: &str, bytes: &[u8]) -> SyncResult<()> {
        let ciphertext = encrypt(&self.key, bytes)?;
        self.inner
            .push_sound_asset(hash, extension, &ciphertext)
            .await
    }

    async fn fetch_sound_asset(&self, hash: &str, extension: &str) -> SyncResult<Option<Vec<u8>>> {
        match self.inner.fetch_sound_asset(hash, extension).await? {
            Some(blob) => Ok(Some(decrypt(&self.key, &blob)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::LogFileName;
    use crate::meta::MetaJson;
    use crate::snapshot::Snapshot;
    use chrono::Utc;
    use std::sync::Mutex;

    /// Trivial in-memory adapter for testing the wrapper. Just
    /// echoes back whatever it was given.
    struct FakeAdapter {
        meta: Mutex<Option<MetaJson>>,
        snapshot: Mutex<Option<Snapshot>>,
        logs: Mutex<Vec<LogFile>>,
        sounds: Mutex<Vec<(String, String, Vec<u8>)>>,
    }

    impl FakeAdapter {
        fn new() -> Self {
            Self {
                meta: Mutex::new(None),
                snapshot: Mutex::new(None),
                logs: Mutex::new(Vec::new()),
                sounds: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl SyncAdapter for FakeAdapter {
        async fn test_connection(&self) -> SyncResult<()> {
            Ok(())
        }
        async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
            Ok(self.meta.lock().unwrap().clone())
        }
        async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
            *self.meta.lock().unwrap() = Some(meta.clone());
            Ok(())
        }
        async fn fetch_new_logs(&self, _since: &DeviceCursor) -> SyncResult<Vec<LogFile>> {
            Ok(self.logs.lock().unwrap().clone())
        }
        async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
            self.logs.lock().unwrap().push(log.clone());
            Ok(())
        }
        async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
            Ok(self.snapshot.lock().unwrap().clone())
        }
        async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
            *self.snapshot.lock().unwrap() = Some(snapshot.clone());
            Ok(())
        }
        async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
            let mut logs = self.logs.lock().unwrap();
            logs.retain(|l| l.name.to_filename() != name.to_filename());
            Ok(())
        }
        async fn push_sound_asset(&self, hash: &str, ext: &str, bytes: &[u8]) -> SyncResult<()> {
            self.sounds
                .lock()
                .unwrap()
                .push((hash.to_string(), ext.to_string(), bytes.to_vec()));
            Ok(())
        }
        async fn fetch_sound_asset(&self, hash: &str, ext: &str) -> SyncResult<Option<Vec<u8>>> {
            Ok(self
                .sounds
                .lock()
                .unwrap()
                .iter()
                .find(|(h, e, _)| h == hash && e == ext)
                .map(|(_, _, b)| b.clone()))
        }
    }

    fn fixture_logname() -> LogFileName {
        LogFileName::new(
            Utc::now(),
            crate::device::DeviceId::from_string("dev-test".into()),
        )
    }

    #[tokio::test]
    async fn push_log_then_fetch_round_trips_through_encryption() {
        let inner: Arc<dyn SyncAdapter> = Arc::new(FakeAdapter::new());
        let encrypting = EncryptingAdapter::new(Arc::clone(&inner), [9u8; KEY_LEN]);
        let plaintext = br#"{"id":"evt-1","type":"event.created"}"#.to_vec();
        let log = LogFile {
            name: fixture_logname(),
            bytes: plaintext.clone(),
        };
        encrypting.push_log(&log).await.unwrap();

        // Inner adapter stores ciphertext, NOT the plaintext.
        let stored = inner.fetch_new_logs(&DeviceCursor::epoch()).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_ne!(stored[0].bytes, plaintext, "inner adapter saw plaintext");
        assert!(
            stored[0].bytes.len() > plaintext.len(),
            "ciphertext should include nonce + tag",
        );

        // Wrapper round-trips back to the plaintext.
        let fetched = encrypting
            .fetch_new_logs(&DeviceCursor::epoch())
            .await
            .unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].bytes, plaintext);
    }

    #[tokio::test]
    async fn snapshot_body_is_replaced_by_encrypted_sentinel_on_push() {
        let inner: Arc<dyn SyncAdapter> = Arc::new(FakeAdapter::new());
        let encrypting = EncryptingAdapter::new(Arc::clone(&inner), [3u8; KEY_LEN]);
        let snapshot = Snapshot::new(
            Utc::now(),
            "1.0.0",
            serde_json::json!({ "events": [{ "id": "e1", "title": "secret" }] }),
        );
        encrypting.push_snapshot(&snapshot).await.unwrap();

        let stored = inner.fetch_snapshot().await.unwrap().unwrap();
        // The inner adapter sees the sentinel-wrapped body.
        assert!(stored.body.get(ENCRYPTED_BODY_KEY).is_some());
        // The original "title": "secret" string must NOT appear
        // in the on-disk body anywhere — that's the whole point.
        let raw = serde_json::to_string(&stored.body).unwrap();
        assert!(!raw.contains("secret"), "plaintext leaked through: {raw}",);

        // The wrapper round-trips the snapshot body back.
        let fetched = encrypting.fetch_snapshot().await.unwrap().unwrap();
        assert_eq!(fetched.body, snapshot.body);
        assert_eq!(fetched.metadata, snapshot.metadata);
    }

    #[tokio::test]
    async fn meta_passes_through_unencrypted() {
        let inner: Arc<dyn SyncAdapter> = Arc::new(FakeAdapter::new());
        let encrypting = EncryptingAdapter::new(Arc::clone(&inner), [4u8; KEY_LEN]);
        let meta = MetaJson::fresh("1.0.0");
        encrypting.push_meta(&meta).await.unwrap();
        let inner_stored = inner.fetch_meta().await.unwrap().unwrap();
        // Inner adapter has the exact MetaJson — no encryption
        // applied. The §19.7 contract requires this so onboarding
        // devices can read schema_version + e2e_enabled before
        // they have a key.
        assert_eq!(inner_stored, meta);
    }

    #[tokio::test]
    async fn sound_asset_is_encrypted_on_push() {
        let inner: Arc<dyn SyncAdapter> = Arc::new(FakeAdapter::new());
        let encrypting = EncryptingAdapter::new(Arc::clone(&inner), [7u8; KEY_LEN]);
        let bytes = b"fake-audio-data";
        encrypting
            .push_sound_asset("abc123", "mp3", bytes)
            .await
            .unwrap();
        // Inner stored ciphertext.
        let inner_bytes = inner
            .fetch_sound_asset("abc123", "mp3")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(inner_bytes, bytes);
        // Wrapper decrypts back.
        let recovered = encrypting
            .fetch_sound_asset("abc123", "mp3")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered, bytes);
    }

    #[tokio::test]
    async fn fetch_log_with_wrong_key_reports_a_decryption_failure() {
        // Push with key A; try to fetch with a wrapper holding
        // key B → AES-GCM's auth tag fails, surface as Auth so
        // the UI prompts for the correct passphrase.
        let inner: Arc<dyn SyncAdapter> = Arc::new(FakeAdapter::new());
        let push_wrap = EncryptingAdapter::new(Arc::clone(&inner), [1u8; KEY_LEN]);
        let fetch_wrap = EncryptingAdapter::new(Arc::clone(&inner), [2u8; KEY_LEN]);
        push_wrap
            .push_log(&LogFile {
                name: fixture_logname(),
                bytes: b"secret".to_vec(),
            })
            .await
            .unwrap();
        let err = fetch_wrap
            .fetch_new_logs(&DeviceCursor::epoch())
            .await
            .unwrap_err();
        assert!(matches!(err, SyncError::DecryptionFailed(_)));
    }

    #[tokio::test]
    async fn plaintext_snapshot_without_sentinel_passes_through_unchanged() {
        // Forward-compat: a snapshot pushed before E2E was
        // enabled (no sentinel marker) survives a fetch through
        // the encrypting wrapper. The body is returned as-is so
        // the user can still read the pre-E2E history.
        let inner: Arc<dyn SyncAdapter> = Arc::new(FakeAdapter::new());
        let snap = Snapshot::new(Utc::now(), "1.0.0", serde_json::json!({ "events": [] }));
        inner.push_snapshot(&snap).await.unwrap();
        let encrypting = EncryptingAdapter::new(Arc::clone(&inner), [8u8; KEY_LEN]);
        let fetched = encrypting.fetch_snapshot().await.unwrap().unwrap();
        assert_eq!(fetched.body, snap.body);
    }
}
