//! Cross-device sync commands (DESIGN.md §19, Phases Sd–Sf).
//!
//! Verbs exposed to the frontend:
//!
//!   - `configure_sync_adapter(config)` — install / swap the
//!     runtime adapter and persist the choice in `user_prefs`. The
//!     "steady state" command, used once the user has already
//!     decided which dataset they're joining.
//!   - `sync_now()` — manual trigger. Returns a `SyncRoundReport`
//!     so the dialogue can show "12 events applied" without a
//!     follow-up status fetch.
//!   - `get_sync_status()` — read-only snapshot for the status
//!     indicator.
//!   - `set_sync_interval(minutes)` — Phase Se. Adjusts the
//!     periodic scheduler.
//!   - `preview_sync_target(config)` — Phase Sf. Reads `meta.json`
//!     at the given config WITHOUT touching the live orchestrator.
//!     Frontend uses the result to drive the onboarding dialog.
//!   - `accept_remote_dataset(config, deviceName)` — Phase Sf.
//!     "Datensatz übernehmen". Configures the adapter, pulls every
//!     log, applies, registers this device in meta.json.
//!   - `adopt_local_dataset(config, deviceName)` — Phase Sf.
//!     "Neu beginnen". Overwrites the remote `meta.json` with one
//!     that names only this device. The frontend is responsible for
//!     the destructive-action confirmation prompt.
//!
//! Adapter configuration values DO NOT propagate via the event log
//! (per §19.2.1) — they're device-local. The user_prefs whitelist
//! already excludes everything under `sync.adapter.*`.

use std::path::PathBuf;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::Deserialize;
use sync_adapter_local::LocalFsSyncAdapter;
use sync_adapter_sftp::{
    HostKeyPreview, HostKeyVerifier, SftpAuth, SftpSyncAdapter,
};
use sync_adapter_webdav::{WebDavCredentials, WebDavSyncAdapter};
use sync_core::{
    derive_key, fresh_data_key, resolve_data_key, wrap_key, EncryptingAdapter,
    EncryptionParams, SyncAdapter, KEY_LEN,
};
use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
use crate::event_log::{
    CompactionReport, OnboardingReport, OnboardingService, SyncOrchestrator, SyncPreview,
    SyncRoundReport, SyncScheduler, SyncStatus,
};
use crate::secrets::{self, SecretSlot};
use crate::sftp_host_keys::UserPrefsHostKeyVerifier;
use crate::sync_log::{SyncLogEntry, SyncLogRepo, MAX_LOG_ROWS};
use crate::user_prefs::UserPrefsRepo;

/// `user_prefs` key naming the currently-configured adapter
/// family. Empty / missing → no adapter, sync disabled.
const PREF_ADAPTER_KIND: &str = "sync.adapter.kind";

/// Family-specific config (per-kind sub-key). For `kind="local"`
/// we just need a filesystem path.
const PREF_LOCAL_PATH: &str = "sync.adapter.local.path";

/// WebDAV adapter config keys. The URL + user live in user_prefs
/// (device-local; never propagated); the password is stored in the
/// platform keychain via the `secrets` module against a fixed
/// pseudo-account id so we get a single managed slot.
const PREF_WEBDAV_URL: &str = "sync.adapter.webdav.url";
const PREF_WEBDAV_USER: &str = "sync.adapter.webdav.user";

/// SFTP adapter config keys. Same device-local / never-synced
/// guarantee as the WebDAV pair. Password / key passphrase live
/// in the keychain under a separate pseudo-account.
const PREF_SFTP_HOST: &str = "sync.adapter.sftp.host";
const PREF_SFTP_PORT: &str = "sync.adapter.sftp.port";
const PREF_SFTP_USER: &str = "sync.adapter.sftp.user";
const PREF_SFTP_PATH: &str = "sync.adapter.sftp.path";
/// `"password"` or `"key"` — selects which auth variant the
/// adapter builds. Mirrors the frontend's radio.
const PREF_SFTP_AUTH_METHOD: &str = "sync.adapter.sftp.authMethod";
/// Absolute path to the SSH private key file when `authMethod`
/// is "key". The path is local-only, not a secret — it can live
/// in user_prefs without going through the keychain.
const PREF_SFTP_KEY_PATH: &str = "sync.adapter.sftp.keyPath";

/// Pseudo-account id used to store the WebDAV password in the
/// platform keychain. The `secrets` module is account-scoped; we
/// use this fixed string so the sync adapter has its own managed
/// keychain entry independent of any user-facing account row.
const WEBDAV_SECRET_ACCOUNT: &str = "sync.adapter.webdav";

/// Pseudo-account id for the SFTP password, separate from the
/// WebDAV one so switching backends doesn't accidentally invalidate
/// the other family's stored credential.
const SFTP_SECRET_ACCOUNT: &str = "sync.adapter.sftp";

/// Pseudo-account id for the SSH-key passphrase. Stored in its
/// own slot so a user that switches from password to key auth
/// (or vice versa) doesn't clobber the inactive credential.
const SFTP_KEY_SECRET_ACCOUNT: &str = "sync.adapter.sftp.key";

/// `user_prefs` key flagging whether the current sync dataset is
/// E2E-encrypted. Mirrors the boolean in `meta.json`; lets
/// `build_adapter_from_prefs` decide synchronously whether to wrap
/// the adapter in `EncryptingAdapter` without needing an async
/// `fetch_meta` round-trip.
const PREF_E2E_ENABLED: &str = "sync.adapter.e2eEnabled";

/// Pseudo-account id for the cross-device sync encryption key.
/// Different from the WebDAV password slot so disabling sync
/// encryption doesn't accidentally invalidate the WebDAV
/// credentials (or vice versa).
const E2E_SECRET_ACCOUNT: &str = "sync.adapter.e2e";

/// Request body for [`configure_sync_adapter`] and the onboarding
/// commands. The kind is flattened so the frontend can build:
///
/// ```jsonc
/// { "kind": "local",  "path":   "/mnt/nas/aperio" }
/// { "kind": "webdav", "url":    "https://cloud.example.com/.../aperio/",
///                     "user":   "alice",
///                     "password": "hunter2" }    // optional on re-edit
/// { "kind": "none" }   // disconnects any configured adapter
/// ```
///
/// Future adapter kinds (`sftp`, `dropbox`, `googledrive`, …) will
/// add their own branches as new struct variants.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncAdapterConfig {
    /// Filesystem path-based adapter — DESIGN.md §19.6 entry.
    Local { path: String },
    /// WebDAV adapter (DESIGN.md §19.6). The URL must point at the
    /// collection that holds `log/`, `snapshot.json`, etc. — for
    /// Nextcloud that's typically
    /// `https://<host>/remote.php/dav/files/<user>/<folder>/`.
    ///
    /// `password` is optional: if `None`, the previously-stored
    /// keychain password is reused. The Settings UI uses that to
    /// support "edit URL without re-typing the password". An empty
    /// string is treated as "no auth", same as omitting the field.
    Webdav {
        url: String,
        user: String,
        #[serde(default)]
        password: Option<String>,
    },
    /// SFTP adapter (DESIGN.md §19.6). `host` is bare; `port`
    /// defaults to 22 on the frontend; `path` is an absolute
    /// remote path.
    ///
    /// `auth_method` discriminates between password and SSH-key
    /// auth. Same Option<String> reuse contract for `password`
    /// and `key_passphrase`: `None` or empty re-fetches the
    /// stored keychain secret so URL/user edits don't require
    /// re-typing.
    Sftp {
        host: String,
        #[serde(default = "default_sftp_port")]
        port: u16,
        user: String,
        path: String,
        #[serde(default = "default_sftp_auth_method")]
        auth_method: String,
        /// Password for `auth_method = "password"`.
        #[serde(default)]
        password: Option<String>,
        /// Filesystem path to a PEM / OpenSSH private key when
        /// `auth_method = "key"`.
        #[serde(default)]
        key_path: Option<String>,
        /// Optional passphrase for an encrypted key. Empty
        /// string is treated as "no passphrase".
        #[serde(default)]
        key_passphrase: Option<String>,
    },
    /// Explicit disconnect. The orchestrator drops its adapter
    /// handle; subsequent `sync_now` calls return a clear "not
    /// configured" error rather than silently no-oping.
    None,
}

fn default_sftp_port() -> u16 {
    22
}

fn default_sftp_auth_method() -> String {
    "password".to_string()
}

/// Build a fresh adapter instance from a [`SyncAdapterConfig`] —
/// validates the inputs, returns an `Arc<dyn SyncAdapter>` ready to
/// hand to the orchestrator or the onboarding service.
///
/// The `None` variant returns Err: the caller is asking for an
/// adapter to operate on, and a disconnect has no adapter to make.
fn build_adapter(
    config: &SyncAdapterConfig,
    db: &crate::db::SharedConn,
) -> CommandResult<Arc<dyn SyncAdapter>> {
    match config {
        SyncAdapterConfig::Local { path } => {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "sync path must not be empty".into(),
                });
            }
            Ok(Arc::new(LocalFsSyncAdapter::new(PathBuf::from(trimmed))))
        }
        SyncAdapterConfig::Webdav { url, user, password } => {
            let trimmed_url = url.trim();
            if trimmed_url.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "WebDAV URL must not be empty".into(),
                });
            }
            let trimmed_user = user.trim();
            // Resolve the password from one of two sources:
            //   - the request body (set on a fresh connect / when
            //     the user re-types it in Settings)
            //   - the keychain (set on a URL-only edit, or on app
            //     start when restoring from prefs)
            let resolved_password = match password.as_deref().map(str::trim) {
                Some(p) if !p.is_empty() => Some(p.to_string()),
                _ => secrets::retrieve(WEBDAV_SECRET_ACCOUNT, SecretSlot::Password)
                    .ok(),
            };
            let credentials = match (trimmed_user.is_empty(), resolved_password) {
                (false, Some(pw)) => WebDavCredentials::basic(trimmed_user, &pw),
                _ => WebDavCredentials::None,
            };
            let adapter = WebDavSyncAdapter::new(trimmed_url, credentials)
                .map_err(sync_err)?;
            Ok(Arc::new(adapter))
        }
        SyncAdapterConfig::Sftp {
            host,
            port,
            user,
            path,
            auth_method,
            password,
            key_path,
            key_passphrase,
        } => {
            let trimmed_host = host.trim();
            let trimmed_user = user.trim();
            let trimmed_path = path.trim();
            if trimmed_host.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "SFTP host must not be empty".into(),
                });
            }
            if trimmed_user.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "SFTP user must not be empty".into(),
                });
            }
            if trimmed_path.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "SFTP path must not be empty".into(),
                });
            }
            // Build the auth method. `password` and `key` are the
            // two we support; anything else surfaces as
            // invalid_input rather than silently picking a
            // default.
            let auth = match auth_method.as_str() {
                "password" => {
                    // Same Option-reuse contract as the WebDAV
                    // branch: Some + non-empty → use the supplied
                    // value; None or empty → re-fetch the
                    // previously-stored keychain secret so
                    // host/user edits don't require re-typing.
                    let resolved = match password.as_deref().map(str::trim) {
                        Some(p) if !p.is_empty() => p.to_string(),
                        _ => secrets::retrieve(
                            SFTP_SECRET_ACCOUNT,
                            SecretSlot::Password,
                        )
                        .map_err(|err| CommandError {
                            code: "auth",
                            message: format!(
                                "no SFTP password configured: {err}",
                            ),
                        })?,
                    };
                    SftpAuth::Password { password: resolved }
                }
                "key" => {
                    let path = key_path
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or(CommandError {
                            code: "invalid_input",
                            message: "SSH key path must not be empty".into(),
                        })?;
                    // Passphrase same Option-reuse contract: empty
                    // / None → re-fetch keychain. An unencrypted
                    // key with neither side supplying a passphrase
                    // round-trips as `None`.
                    let passphrase = match key_passphrase
                        .as_deref()
                        .map(str::trim)
                    {
                        Some(p) if !p.is_empty() => Some(p.to_string()),
                        _ => secrets::retrieve(
                            SFTP_KEY_SECRET_ACCOUNT,
                            SecretSlot::Password,
                        )
                        .ok()
                        .filter(|s| !s.is_empty()),
                    };
                    SftpAuth::PrivateKey {
                        path: PathBuf::from(path),
                        passphrase,
                    }
                }
                other => {
                    return Err(CommandError {
                        code: "invalid_input",
                        message: format!(
                            "unknown SFTP auth method: {other}",
                        ),
                    });
                }
            };
            let verifier =
                Arc::new(UserPrefsHostKeyVerifier::new(db.clone()));
            Ok(Arc::new(SftpSyncAdapter::new(
                trimmed_host,
                *port,
                trimmed_user,
                auth,
                PathBuf::from(trimmed_path),
                verifier,
            )))
        }
        SyncAdapterConfig::None => Err(CommandError {
            code: "invalid_input",
            message: "cannot build adapter from None kind".into(),
        }),
    }
}

/// Persist the (already-validated) adapter config into `user_prefs`
/// so the next app start restores the same adapter. Mirrors what
/// `build_adapter_from_prefs` will read back.
fn persist_adapter_config(
    prefs: &UserPrefsRepo,
    config: &SyncAdapterConfig,
) -> CommandResult<()> {
    match config {
        SyncAdapterConfig::Local { path } => {
            let trimmed = path.trim();
            prefs.set(PREF_ADAPTER_KIND, "local").map_err(internal)?;
            prefs.set(PREF_LOCAL_PATH, trimmed).map_err(internal)?;
            Ok(())
        }
        SyncAdapterConfig::Webdav { url, user, password } => {
            prefs.set(PREF_ADAPTER_KIND, "webdav").map_err(internal)?;
            prefs.set(PREF_WEBDAV_URL, url.trim()).map_err(internal)?;
            prefs.set(PREF_WEBDAV_USER, user.trim()).map_err(internal)?;
            // Only overwrite the keychain when the request body
            // explicitly carries a non-empty password. URL/user
            // edits that omit the password keep the prior secret.
            if let Some(pw) = password.as_deref().map(str::trim) {
                if !pw.is_empty() {
                    secrets::store(
                        WEBDAV_SECRET_ACCOUNT,
                        SecretSlot::Password,
                        pw,
                    )
                    .map_err(|err| CommandError {
                        code: "internal",
                        message: format!("keychain store: {err}"),
                    })?;
                }
            }
            Ok(())
        }
        SyncAdapterConfig::Sftp {
            host,
            port,
            user,
            path,
            auth_method,
            password,
            key_path,
            key_passphrase,
        } => {
            prefs.set(PREF_ADAPTER_KIND, "sftp").map_err(internal)?;
            prefs.set(PREF_SFTP_HOST, host.trim()).map_err(internal)?;
            prefs.set(PREF_SFTP_PORT, &port.to_string()).map_err(internal)?;
            prefs.set(PREF_SFTP_USER, user.trim()).map_err(internal)?;
            prefs.set(PREF_SFTP_PATH, path.trim()).map_err(internal)?;
            prefs
                .set(PREF_SFTP_AUTH_METHOD, auth_method.trim())
                .map_err(internal)?;
            if let Some(p) = key_path.as_deref().map(str::trim) {
                if !p.is_empty() {
                    prefs.set(PREF_SFTP_KEY_PATH, p).map_err(internal)?;
                }
            }
            // Only overwrite the password keychain when the
            // request body carries a non-empty value. Same
            // reasoning as the WebDAV branch.
            if let Some(pw) = password.as_deref().map(str::trim) {
                if !pw.is_empty() {
                    secrets::store(
                        SFTP_SECRET_ACCOUNT,
                        SecretSlot::Password,
                        pw,
                    )
                    .map_err(|err| CommandError {
                        code: "internal",
                        message: format!("keychain store: {err}"),
                    })?;
                }
            }
            if let Some(pp) = key_passphrase.as_deref().map(str::trim) {
                if !pp.is_empty() {
                    secrets::store(
                        SFTP_KEY_SECRET_ACCOUNT,
                        SecretSlot::Password,
                        pp,
                    )
                    .map_err(|err| CommandError {
                        code: "internal",
                        message: format!("keychain store: {err}"),
                    })?;
                }
            }
            Ok(())
        }
        SyncAdapterConfig::None => {
            prefs.delete(PREF_ADAPTER_KIND).map_err(internal)?;
            Ok(())
        }
    }
}

/// Translate a [`sync_core::SyncError`] into a [`CommandError`]
/// with a stable code the frontend can pattern-match.
///
/// Critical mapping for Phase Sf: `NotFound` becomes `not_found`,
/// which the onboarding dialog uses to decide whether to flip from
/// "übernehmen" to "neu beginnen" without surfacing a stack trace.
fn sync_err(err: sync_core::SyncError) -> CommandError {
    use sync_core::SyncError as E;
    let code: &'static str = match &err {
        E::Io(_) => "io",
        E::Network(_) => "network",
        E::Auth(_) => "auth",
        E::Protocol(_) => "protocol",
        E::EncryptionRequired => "encryption_required",
        E::NotFound(_) => "not_found",
        E::SchemaTooOld { .. } => "schema_too_old",
        E::StaleDevice { .. } => "stale_device",
        E::Internal(_) => "internal",
    };
    CommandError {
        code,
        message: err.to_string(),
    }
}

fn internal(err: impl std::fmt::Display) -> CommandError {
    CommandError {
        code: "internal",
        message: err.to_string(),
    }
}

/// Read the persisted 32-byte AES key from the keychain. Base64
/// is the on-disk encoding because the keyring crate's backend
/// rejects null bytes on some platforms.
fn load_e2e_key() -> Option<[u8; KEY_LEN]> {
    let raw = secrets::retrieve(E2E_SECRET_ACCOUNT, SecretSlot::SyncEncryptionKey).ok()?;
    let bytes = BASE64.decode(raw).ok()?;
    if bytes.len() != KEY_LEN {
        return None;
    }
    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(&bytes);
    Some(out)
}

/// Persist the 32-byte AES key in the keychain.
fn store_e2e_key(key: &[u8; KEY_LEN]) -> CommandResult<()> {
    let encoded = BASE64.encode(key);
    secrets::store(
        E2E_SECRET_ACCOUNT,
        SecretSlot::SyncEncryptionKey,
        &encoded,
    )
    .map_err(|err| CommandError {
        code: "internal",
        message: format!("keychain store sync key: {err}"),
    })
}

/// Drop the keychain entry for the sync key. Used by
/// `disable_sync_encryption` after the dataset has been
/// transitioned to plaintext, and reserved for any future
/// "disconnect + wipe keychain" flow.
fn delete_e2e_key() {
    let _ = secrets::delete(E2E_SECRET_ACCOUNT, SecretSlot::SyncEncryptionKey);
}

/// If `key` is present, wrap the plain adapter in
/// `EncryptingAdapter`. Otherwise return the plain adapter
/// unchanged. Consolidates the "did we just configure E2E" check
/// at every call site (configure, onboard, restore-from-prefs).
fn wrap_if_encrypted(
    plain: Arc<dyn SyncAdapter>,
    key: Option<[u8; KEY_LEN]>,
) -> Arc<dyn SyncAdapter> {
    match key {
        Some(k) => Arc::new(EncryptingAdapter::new(plain, k)),
        None => plain,
    }
}

/// Install / swap the active sync adapter. Persists the user's
/// choice so the next app start reconstructs the same adapter
/// in `lib.rs`'s setup phase.
///
/// **Note:** This is the "I already onboarded; just swap the
/// adapter" command. New users go through `preview_sync_target` +
/// `accept_remote_dataset` / `adopt_local_dataset` instead, which
/// configures the adapter as part of the onboarding flow.
#[tauri::command]
pub async fn configure_sync_adapter(
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
    onboarding: State<'_, Arc<OnboardingService>>,
    config: SyncAdapterConfig,
) -> CommandResult<()> {
    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);
    match &config {
        SyncAdapterConfig::Local { .. }
        | SyncAdapterConfig::Webdav { .. }
        | SyncAdapterConfig::Sftp { .. } => {
            // Persist BEFORE building the adapter so the keychain
            // entry for the new WebDAV password is in place; the
            // adapter constructor then reads it back when the
            // request body omitted the password (e.g. URL-only
            // edit). Then we probe.
            persist_adapter_config(&prefs, &config)?;
            let plain = build_adapter(&config, &shared)?;
            // Probe the connection before keeping the adapter
            // active — misconfigurations should surface immediately
            // at the settings dialog, not hours later when the
            // first sync_now runs.
            plain.test_connection().await.map_err(sync_err)?;
            // Phase Sk: inspect the target's `meta.json` to decide
            // whether to wrap with `EncryptingAdapter`. We don't
            // re-derive the key here — that requires the
            // passphrase. If the target is E2E and we already have
            // the key in our keychain (same logical dataset across
            // adapter swap), reuse it. Otherwise refuse — the
            // onboarding flow is the right path for "I'm joining a
            // new encrypted dataset".
            let target_meta = plain.fetch_meta().await.map_err(sync_err)?;
            // Phase Sl: refuse the swap if the target dataset
            // requires a newer Aperio than the running build. The
            // Settings dialog gets the `schema_too_old` error code
            // and renders the §19.13 update prompt; the user can
            // either update or pick a different target.
            if let Some(m) = target_meta.as_ref() {
                sync_core::ensure_compatible(m, onboarding.app_version())
                    .map_err(sync_err)?;
            }
            let e2e_target = target_meta
                .as_ref()
                .map(|m| m.e2e_enabled)
                .unwrap_or(false);
            let key = if e2e_target {
                let k = load_e2e_key().ok_or(CommandError {
                    code: "encryption_required",
                    message: "target dataset is encrypted; onboard via accept_remote_dataset first"
                        .into(),
                })?;
                Some(k)
            } else {
                None
            };
            let adapter = wrap_if_encrypted(plain, key);
            orchestrator.configure(adapter);
            // Keep PREF_E2E_ENABLED in sync with what we just
            // discovered on the target meta. The keychain key
            // stays either way; the flag is the source of truth
            // for "should we wrap on next boot".
            if e2e_target {
                prefs.set(PREF_E2E_ENABLED, "true").map_err(internal)?;
            } else {
                let _ = prefs.delete(PREF_E2E_ENABLED);
            }
            // Kick the scheduler so the user sees data flow
            // immediately instead of waiting up to one interval
            // for the periodic loop. The debounce window swallows
            // any pile of mutations the writer queued while the
            // adapter was unconfigured.
            scheduler.kick();
        }
        SyncAdapterConfig::None => {
            orchestrator.deconfigure();
            persist_adapter_config(&prefs, &config)?;
            // Keep PREF_LOCAL_PATH / PREF_WEBDAV_* around so re-
            // enabling the same backend is one click away. The
            // keychain password also stays — it's never synced and
            // a user reconnecting to the same dataset wouldn't
            // want to re-type it.
        }
    }
    Ok(())
}

/// Set the periodic sync interval (in minutes). Values below 1 are
/// clamped to 1 so a typo can't pin the scheduler into a hot loop.
/// Returns the value actually persisted so the Settings UI can echo
/// it back into its slider.
#[tauri::command]
pub async fn set_sync_interval(
    scheduler: State<'_, Arc<SyncScheduler>>,
    minutes: u32,
) -> CommandResult<u32> {
    scheduler.set_interval_minutes(minutes).map_err(|err| CommandError {
        code: "internal",
        message: err,
    })
}

/// Trigger one sync round (push pending logs + fetch & apply
/// new ones). On success, clears the scheduler's failure latch —
/// so a user who clicks "Sync now" after a transient hiccup is
/// out of the warning state immediately, not on the next
/// periodic tick.
///
/// Records the outcome in the §19.9 Sync-Protokoll so manual
/// triggers show up alongside periodic ones in the Settings
/// history list.
#[tauri::command]
pub async fn sync_now(
    app: tauri::AppHandle,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
) -> CommandResult<SyncRoundReport> {
    let started = std::time::Instant::now();
    let result = orchestrator.sync_now().await;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    scheduler.record_manual_outcome(
        &app,
        crate::sync_log::SyncTrigger::Manual,
        &result,
        duration_ms,
    );
    // Mirror the scheduler-loop bookkeeping: on a manual-sync
    // failure we latch the error code so the StatusBar's auth
    // banner picks up; on success we clear both counters.
    let report = match result {
        Ok(r) => r,
        Err(err) => {
            scheduler.note_failure(&err);
            return Err(sync_err(err));
        }
    };
    scheduler.note_success();
    // If new conflicts landed during this manual round, kick
    // the frontend's conflict-count refetch + notification
    // path. Same logic as the periodic scheduler's `run_round`.
    if report.conflicts > 0 {
        if let Err(err) =
            tauri::Emitter::emit(&app, "sync-conflicts-changed", ())
        {
            tracing::warn!(
                ?err,
                "failed to emit sync-conflicts-changed after manual sync",
            );
        }
    }
    Ok(report)
}

/// Read-only status snapshot for the status indicator. Returns
/// the scheduler-decorated status so `sustained_failure` and
/// any future scheduler-level flags are visible to the frontend.
#[tauri::command]
pub async fn get_sync_status(
    scheduler: State<'_, Arc<SyncScheduler>>,
) -> CommandResult<SyncStatus> {
    Ok(scheduler.current_status())
}

/// Manually trigger a compaction round (Phase Sg, §19.10). Snapshots
/// the current local state, pushes `snapshot.json`, advances
/// `meta.json.snapshot_timestamp`, and GCs every log file older than
/// the new snapshot horizon (clamped by the slowest device's
/// `last_seen_log`).
///
/// Returns counters the Settings UI can render directly. The
/// scheduler also dispatches this same code path automatically when
/// the thresholds (§19.10) fire — the command is the "user got
/// impatient" override.
#[tauri::command]
pub async fn compact_now(
    app: tauri::AppHandle,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
) -> CommandResult<CompactionReport> {
    // Borrow the adapter handle for the duration of the compaction.
    // If none is configured, fail fast with a clear code rather than
    // letting the compactor panic later.
    let adapter = {
        let inner = orchestrator.adapter_handle();
        match inner {
            Some(a) => a,
            None => {
                return Err(CommandError {
                    code: "not_configured",
                    message: "no sync adapter configured".into(),
                });
            }
        }
    };
    let started = std::time::Instant::now();
    let result = orchestrator
        .compactor()
        .compact_now(adapter.as_ref())
        .await;
    let duration_ms =
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    // §19.10 — surface the outcome in the Protokoll regardless of
    // success/failure so the user has an audit trail of every
    // compaction run. Mirrors the manual-sync_now bookkeeping.
    scheduler.record_compaction_outcome(&app, &result, duration_ms);
    result.map_err(sync_err)
}

/// Test the supplied adapter config end-to-end without committing
/// anything. Builds the adapter, calls `test_connection`, throws
/// away the adapter handle. Intended for the SyncPanel's
/// "Verbindung testen" button so the user can verify URL / host /
/// credentials in isolation before they hit Connect.
///
/// SFTP semantics: the test path uses the same UserPrefs-backed
/// host-key verifier the real adapter would use. If the user
/// hasn't pinned yet, the silent-TOFU verifier will accept and
/// pin on first contact — same behaviour as if they'd clicked
/// Connect directly. That's fine: the trust-dialog flow is
/// upstream of this command, and the user wouldn't reach the
/// test button without having seen it.
///
/// Returns no payload on success; failures map to the standard
/// `sync_err` codes (`network`, `auth`, `not_found`, …) so the
/// frontend can reuse the same error formatting it already has.
#[tauri::command]
pub async fn test_sync_adapter(
    db: State<'_, DbHandle>,
    config: SyncAdapterConfig,
) -> CommandResult<()> {
    let shared = db.shared();
    let adapter = build_adapter(&config, &shared)?;
    adapter.test_connection().await.map_err(sync_err)
}

// ---------------------------------------------------------------------------
// Phase Sf — onboarding commands.
// ---------------------------------------------------------------------------

/// Probe a remote without committing to it. Reads `meta.json` and
/// returns a [`SyncPreview`] the frontend uses to render the
/// onboarding dialog ("found existing data — adopt or overwrite?").
///
/// Side-effect free: the user can step back from the dialog and
/// pick a different path without leaving cruft in `user_prefs` or
/// the orchestrator state.
#[tauri::command]
pub async fn preview_sync_target(
    db: State<'_, DbHandle>,
    onboarding: State<'_, Arc<OnboardingService>>,
    config: SyncAdapterConfig,
) -> CommandResult<SyncPreview> {
    let shared = db.shared();
    let adapter = build_adapter(&config, &shared)?;
    onboarding.preview(adapter.as_ref()).await.map_err(sync_err)
}

/// "Datensatz übernehmen" path of the §19.11 onboarding flow.
///
/// Configures the orchestrator with the chosen adapter, pulls every
/// remote log into the applier, registers this device in
/// `meta.json`, and persists the user_prefs entries so the next app
/// start restores the same adapter and device name.
///
/// On success the scheduler is kicked so the next round emits the
/// post-onboarding heartbeat.
#[tauri::command]
pub async fn accept_remote_dataset(
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
    onboarding: State<'_, Arc<OnboardingService>>,
    config: SyncAdapterConfig,
    device_name: Option<String>,
    passphrase: Option<String>,
) -> CommandResult<OnboardingReport> {
    let shared = db.shared();
    let plain = build_adapter(&config, &shared)?;
    plain.test_connection().await.map_err(sync_err)?;

    // Phase Sk: peek at meta.json to see if the dataset is
    // encrypted. If it is, we must derive the key BEFORE the
    // accept_remote flow tries to read snapshots or logs — the
    // applier needs decrypted bytes.
    let meta = plain.fetch_meta().await.map_err(sync_err)?;
    let e2e_active = meta
        .as_ref()
        .map(|m| m.e2e_enabled)
        .unwrap_or(false);
    let key: Option<[u8; KEY_LEN]> = if e2e_active {
        let pp = passphrase.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let pp = pp.ok_or(CommandError {
            code: "encryption_required",
            message: "this dataset is encrypted; a passphrase is required"
                .into(),
        })?;
        let params = meta
            .as_ref()
            .and_then(|m| m.e2e_params.clone())
            .ok_or(CommandError {
                code: "protocol",
                message: "meta.json says e2e but carries no params".into(),
            })?;
        // `resolve_data_key` handles both layouts: v1 datasets
        // (no `wrapped_data_key`) where the passphrase-derived
        // key IS the DEK, and v2 datasets where it's a KEK that
        // unwraps the actual DEK stored in meta.json. Either way
        // we end up with the byte sequence that decrypts the
        // logs + snapshot.
        let dek = resolve_data_key(pp, &params).map_err(sync_err)?;
        Some(dek)
    } else {
        None
    };
    let adapter = wrap_if_encrypted(Arc::clone(&plain), key);

    // Run the onboarding side first. If it fails (e.g. remote has
    // no meta.json or the passphrase is wrong → applier fails to
    // parse JSON), we haven't yet altered the orchestrator's
    // state — the next attempt can pick a different path.
    let trimmed = device_name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let report = onboarding
        .accept_remote(adapter.as_ref(), trimmed)
        .await
        .map_err(sync_err)?;

    // Commit the choice into the orchestrator + user_prefs only
    // now that onboarding has succeeded.
    orchestrator.configure(Arc::clone(&adapter));
    let prefs = UserPrefsRepo::new(&shared);
    persist_adapter_config(&prefs, &config)?;
    // Persist E2E state alongside the adapter config — the
    // restore-on-boot path reads both.
    if let Some(k) = key {
        store_e2e_key(&k)?;
        prefs.set(PREF_E2E_ENABLED, "true").map_err(internal)?;
    } else {
        // Joining a non-E2E dataset wipes any stale flag from a
        // previous session.
        let _ = prefs.delete(PREF_E2E_ENABLED);
    }
    scheduler.kick();
    Ok(report)
}

/// "Neu beginnen" path of the §19.11 onboarding flow.
///
/// Overwrites the remote `meta.json` with a fresh one naming only
/// this device, then wires the orchestrator + scheduler so the
/// already-queued pending logs push on the next round.
///
/// Caller MUST gate this behind an explicit confirmation prompt —
/// existing remote logs become orphans the moment the new
/// `meta.json` lands (they're physically still there but no device
/// claims them in the registry; compaction in Phase Sg eventually
/// reaps them).
#[tauri::command]
pub async fn adopt_local_dataset(
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
    onboarding: State<'_, Arc<OnboardingService>>,
    config: SyncAdapterConfig,
    device_name: Option<String>,
    passphrase: Option<String>,
) -> CommandResult<OnboardingReport> {
    let shared = db.shared();
    let plain = build_adapter(&config, &shared)?;
    plain.test_connection().await.map_err(sync_err)?;

    // Phase Sk + §19.7 passphrase rotation: fresh datasets land
    // directly as v2 (KEK + DEK). We mint a random DEK, derive a
    // KEK from the passphrase + fresh params, wrap the DEK with
    // the KEK, and write both into `meta.json`. The DEK becomes
    // the long-term data key; later passphrase changes only
    // re-wrap it — the on-the-wire ciphertext stays untouched.
    //
    // Legacy v1 datasets (no `wrapped_data_key`) created by older
    // app versions still onboard via the v1 read path in
    // `accept_remote_dataset` above; this branch only mints
    // fresh datasets so producing v2 here doesn't break any
    // existing deployment.
    let trimmed_pp = passphrase
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (key, e2e_params) = match trimmed_pp {
        Some(pp) => {
            let mut params = EncryptionParams::fresh();
            let kek = derive_key(pp, &params).map_err(sync_err)?;
            let dek = fresh_data_key();
            let wrapped = wrap_key(&kek, &dek).map_err(sync_err)?;
            params.wrapped_data_key = Some(wrapped);
            (Some(dek), Some(params))
        }
        None => (None, None),
    };
    let adapter = wrap_if_encrypted(Arc::clone(&plain), key);

    let trimmed = device_name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let report = onboarding
        .adopt_local(adapter.as_ref(), trimmed, e2e_params)
        .await
        .map_err(sync_err)?;

    orchestrator.configure(Arc::clone(&adapter));
    let prefs = UserPrefsRepo::new(&shared);
    persist_adapter_config(&prefs, &config)?;
    if let Some(k) = key {
        store_e2e_key(&k)?;
        prefs.set(PREF_E2E_ENABLED, "true").map_err(internal)?;
    } else {
        let _ = prefs.delete(PREF_E2E_ENABLED);
    }
    scheduler.kick();
    Ok(report)
}

/// §19.7 — rotate the dataset's E2E passphrase.
///
/// Verifies the **old passphrase** against the dataset's
/// current wrap (or, on a legacy v1 dataset, against the
/// keychain-stored direct key), then rewraps the long-term
/// data-encryption key (DEK) under a fresh key-encryption key
/// (KEK) derived from the new passphrase + a freshly-rotated
/// salt. The DEK itself never changes — so no log files,
/// snapshots, or sound assets need to be re-encrypted, and
/// other devices that already have the DEK in their keychain
/// keep syncing without interruption.
///
/// On the first successful change on a legacy v1 dataset this
/// silently migrates it to v2: the meta.json that lands on the
/// remote carries `wrapped_data_key`, and subsequent passphrase
/// changes on any device benefit from the cheap re-wrap flow.
///
/// Error mapping:
///   - wrong old passphrase → `auth`
///   - adapter not configured or not E2E → `not_configured`
///   - meta.json missing or malformed → `protocol`
///   - network / IO errors during the meta read or write →
///     `io` / `network`
///
/// Atomicity: the meta.json `push_meta` is the single committing
/// step. A failure before that leaves the dataset on the old
/// passphrase; a failure after it commits but before we return
/// the user still sees the change, because the DEK in their
/// keychain didn't change.
#[tauri::command]
pub async fn change_sync_passphrase(
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    old_passphrase: String,
    new_passphrase: String,
) -> CommandResult<()> {
    let new_pp = new_passphrase.trim();
    if new_pp.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "new passphrase must not be empty".into(),
        });
    }
    let old_pp = old_passphrase.trim();
    if old_pp.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "current passphrase must not be empty".into(),
        });
    }

    let adapter = orchestrator.adapter_handle().ok_or(CommandError {
        code: "not_configured",
        message: "no sync adapter is configured".into(),
    })?;

    // Pull current meta.json. The EncryptingAdapter wraps reads,
    // but meta.json is always plaintext (§19.7) so the
    // `fetch_meta` path is a pass-through either way.
    let meta = adapter
        .fetch_meta()
        .await
        .map_err(sync_err)?
        .ok_or(CommandError {
            code: "not_found",
            message: "remote has no meta.json — onboard first".into(),
        })?;
    if !meta.e2e_enabled {
        return Err(CommandError {
            code: "not_configured",
            message: "dataset is not encrypted; nothing to rotate".into(),
        });
    }
    let current_params = meta.e2e_params.clone().ok_or(CommandError {
        code: "protocol",
        message: "meta.json says e2e but carries no params".into(),
    })?;

    // Verify the old passphrase. On v2 datasets this unwraps
    // `wrapped_data_key` with the derived KEK; on v1 it derives
    // the direct key. Either way the returned bytes are the DEK
    // we should be encrypting blobs with.
    let dek = resolve_data_key(old_pp, &current_params).map_err(sync_err)?;

    // Defence in depth — the keychain on this device should
    // already hold the DEK. If it doesn't (someone forced-
    // restored a backup, or the keychain entry got corrupted),
    // we still proceed: the user typed the correct passphrase
    // and we just recovered a usable DEK from the wrap.
    // Re-write the keychain to be sure the next boot loads
    // the right key.
    store_e2e_key(&dek)?;

    // Derive the new KEK with a fresh salt — same Argon2
    // parameters as before so the cost profile doesn't drift,
    // but the new salt means an old precomputed table buys
    // the attacker nothing on the new wrap.
    let mut new_params = current_params;
    new_params.rotate_salt();
    new_params.wrapped_data_key = None;
    let new_kek = derive_key(new_pp, &new_params).map_err(sync_err)?;
    let new_wrap = wrap_key(&new_kek, &dek).map_err(sync_err)?;
    new_params.wrapped_data_key = Some(new_wrap);

    // Build the updated meta.json and push it. After this
    // returns successfully the new passphrase is the
    // authoritative one — other devices that re-onboard from
    // here on will need it; existing devices keep working
    // because the DEK in their keychain is unchanged.
    let mut updated = meta;
    updated.e2e_params = Some(new_params);
    adapter.push_meta(&updated).await.map_err(sync_err)?;

    Ok(())
}

/// Outcome counters for [`disable_sync_encryption`]. Surfaced to
/// the user so the success message can read "12 logs rewritten,
/// snapshot rewritten" rather than a generic "done".
#[derive(Debug, Default, serde::Serialize)]
pub struct DisableE2eReport {
    pub logs_rewritten: usize,
    pub snapshot_rewritten: bool,
}

/// §19.7 — turn off end-to-end encryption on the dataset.
///
/// In-place migration: verify the user's current passphrase,
/// fetch every encrypted log + snapshot via the encrypting
/// wrapper (which decrypts on the way), then push the plaintext
/// bytes through the bare adapter (overwriting the encrypted
/// originals at the same paths). The meta.json update is the
/// last step and the atomic commit — a crash before it leaves
/// the encrypted view authoritative, a crash after it
/// completes makes the plaintext view authoritative.
///
/// **Other devices need to re-onboard** after this completes.
/// Their local config still says e2e_enabled=true and their
/// keychain holds the DEK — they'll try to decrypt the new
/// plaintext bytes, get garbage, and fail the sync. The UI
/// gates this command behind a strong confirmation so the user
/// understands the cluster-wide impact.
///
/// Sound assets are intentionally NOT re-pushed here. Custom
/// sounds the user has locally can be re-uploaded by the next
/// sync round once disable completes (clearing the pushed
/// marker would force exactly that re-push); sounds that exist
/// only on the remote in encrypted form stay encrypted — they
/// were already inaccessible to fresh devices, and disabling
/// E2E doesn't change that.
///
/// Error mapping mirrors `change_sync_passphrase`:
///   - wrong current passphrase → `auth`
///   - adapter not configured or not E2E → `not_configured`
///   - meta.json missing or malformed → `protocol`
#[tauri::command]
pub async fn disable_sync_encryption(
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    current_passphrase: String,
) -> CommandResult<DisableE2eReport> {
    let pp = current_passphrase.trim();
    if pp.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "current passphrase must not be empty".into(),
        });
    }

    // 1. Borrow the active (encrypting) adapter so we can decrypt
    //    on the way down.
    let encrypting = orchestrator.adapter_handle().ok_or(CommandError {
        code: "not_configured",
        message: "no sync adapter is configured".into(),
    })?;

    // 2. Verify the current passphrase against the dataset's wrap.
    //    Same logic as change_sync_passphrase — `resolve_data_key`
    //    handles both v1 (direct key) and v2 (KEK + wrapped DEK)
    //    layouts.
    let meta_before = encrypting
        .fetch_meta()
        .await
        .map_err(sync_err)?
        .ok_or(CommandError {
            code: "not_found",
            message: "remote has no meta.json — nothing to disable".into(),
        })?;
    if !meta_before.e2e_enabled {
        return Err(CommandError {
            code: "not_configured",
            message: "dataset is not encrypted; nothing to disable".into(),
        });
    }
    let current_params = meta_before.e2e_params.clone().ok_or(CommandError {
        code: "protocol",
        message: "meta.json says e2e but carries no params".into(),
    })?;
    let _verified_dek =
        resolve_data_key(pp, &current_params).map_err(sync_err)?;

    // 3. Build a fresh PLAIN adapter from the persisted config.
    //    The encrypting wrapper is what the orchestrator holds —
    //    we need an unwrapped handle to push plaintext bytes. The
    //    same builder lib.rs's app-start path uses, so the auth +
    //    config bits are guaranteed to match.
    let shared = db.shared();
    let plain =
        build_adapter_from_prefs(&shared).ok_or(CommandError {
            code: "not_configured",
            message: "couldn't rebuild the underlying plain adapter".into(),
        })?;

    let mut report = DisableE2eReport::default();

    // 4. Re-encrypt every log: fetch via encrypting (decrypts),
    //    push via plain (writes verbatim). Adapter push_log
    //    overwrites at the same path, so no orphan files.
    let logs = encrypting
        .fetch_new_logs(&sync_core::DeviceCursor::epoch())
        .await
        .map_err(sync_err)?;
    for log in logs {
        plain.push_log(&log).await.map_err(sync_err)?;
        report.logs_rewritten += 1;
    }

    // 5. Same for the snapshot, if one exists. Brand-new
    //    datasets that never compacted skip this branch.
    if let Some(snapshot) = encrypting
        .fetch_snapshot()
        .await
        .map_err(sync_err)?
    {
        plain.push_snapshot(&snapshot).await.map_err(sync_err)?;
        report.snapshot_rewritten = true;
    }

    // 6. Commit the disable atomically by overwriting meta.json
    //    with e2e_enabled=false + clearing e2e_params. After
    //    this lands, the cluster is officially plaintext.
    let mut updated = meta_before;
    updated.e2e_enabled = false;
    updated.e2e_params = None;
    plain.push_meta(&updated).await.map_err(sync_err)?;

    // 7. Swap the orchestrator over to the plain adapter so
    //    subsequent rounds in this process don't try to wrap
    //    pushes with the (now-defunct) key.
    orchestrator.configure(Arc::clone(&plain));

    // 8. Clean up local state: drop the keychain entry and
    //    flip the pref. Failures here are logged but don't
    //    fail the command — the on-the-wire state is what
    //    matters, and the local prefs catch up on the next
    //    boot via build_adapter_from_prefs.
    let prefs = UserPrefsRepo::new(&shared);
    let _ = prefs.delete(PREF_E2E_ENABLED);
    delete_e2e_key();

    Ok(report)
}

/// Helper used by `lib.rs::setup` to reconstruct the adapter
/// from the persisted prefs on app start. Returns `Ok(None)`
/// when no adapter was configured before — the orchestrator
/// stays in its initial unconfigured state.
pub fn build_adapter_from_prefs(
    db: &crate::db::SharedConn,
) -> Option<Arc<dyn SyncAdapter>> {
    let prefs = UserPrefsRepo::new(db);
    let kind = prefs.get(PREF_ADAPTER_KIND).ok().flatten()?;
    let plain: Arc<dyn SyncAdapter> = match kind.as_str() {
        "local" => {
            let path = prefs.get(PREF_LOCAL_PATH).ok().flatten()?;
            if path.trim().is_empty() {
                return None;
            }
            Arc::new(LocalFsSyncAdapter::new(PathBuf::from(path)))
        }
        "webdav" => {
            let url = prefs.get(PREF_WEBDAV_URL).ok().flatten()?;
            if url.trim().is_empty() {
                return None;
            }
            let user = prefs
                .get(PREF_WEBDAV_USER)
                .ok()
                .flatten()
                .unwrap_or_default();
            let password = secrets::retrieve(
                WEBDAV_SECRET_ACCOUNT,
                SecretSlot::Password,
            )
            .ok();
            let credentials = match (user.trim().is_empty(), password) {
                (false, Some(pw)) => WebDavCredentials::basic(user.trim(), &pw),
                _ => WebDavCredentials::None,
            };
            let adapter = WebDavSyncAdapter::new(url.trim(), credentials).ok()?;
            Arc::new(adapter)
        }
        "sftp" => {
            let host = prefs.get(PREF_SFTP_HOST).ok().flatten()?;
            if host.trim().is_empty() {
                return None;
            }
            let port = prefs
                .get(PREF_SFTP_PORT)
                .ok()
                .flatten()
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(22);
            let user = prefs.get(PREF_SFTP_USER).ok().flatten()?;
            if user.trim().is_empty() {
                return None;
            }
            let path = prefs.get(PREF_SFTP_PATH).ok().flatten()?;
            if path.trim().is_empty() {
                return None;
            }
            let auth_method = prefs
                .get(PREF_SFTP_AUTH_METHOD)
                .ok()
                .flatten()
                .unwrap_or_else(|| "password".to_string());
            let auth = match auth_method.as_str() {
                "key" => {
                    let key_path =
                        prefs.get(PREF_SFTP_KEY_PATH).ok().flatten()?;
                    if key_path.trim().is_empty() {
                        return None;
                    }
                    let passphrase = secrets::retrieve(
                        SFTP_KEY_SECRET_ACCOUNT,
                        SecretSlot::Password,
                    )
                    .ok()
                    .filter(|s| !s.is_empty());
                    SftpAuth::PrivateKey {
                        path: PathBuf::from(key_path.trim()),
                        passphrase,
                    }
                }
                // "password" + anything unknown both fall to
                // password auth — forward-compat for a future
                // auth method that an older Aperio doesn't know.
                _ => {
                    let password = secrets::retrieve(
                        SFTP_SECRET_ACCOUNT,
                        SecretSlot::Password,
                    )
                    .ok()?;
                    SftpAuth::Password { password }
                }
            };
            let verifier =
                Arc::new(UserPrefsHostKeyVerifier::new(db.clone()));
            Arc::new(SftpSyncAdapter::new(
                host.trim(),
                port,
                user.trim(),
                auth,
                PathBuf::from(path.trim()),
                verifier,
            ))
        }
        // Forward-compat: an unknown kind (left over from a
        // future Aperio version) is silently treated as "no
        // adapter configured" rather than a hard error. The
        // user reconfigures in Settings; we don't crash the
        // app over it.
        _ => return None,
    };

    // Phase Sk: if the dataset is flagged E2E in user_prefs,
    // wrap with `EncryptingAdapter` using the keychain-stored
    // key. If the key is missing (keychain wiped, fresh OS
    // install with the same data dir), bail out so the user
    // re-runs onboarding rather than syncing garbage.
    let e2e_on = prefs
        .get(PREF_E2E_ENABLED)
        .ok()
        .flatten()
        .as_deref()
        == Some("true");
    if e2e_on {
        let key = load_e2e_key()?;
        Some(wrap_if_encrypted(plain, Some(key)))
    } else {
        Some(plain)
    }
}

// ---------------------------------------------------------------------------
// Phase Sm — SFTP host-key trust dialog support.
// ---------------------------------------------------------------------------

/// Probe an SFTP server's SHA256 host-key fingerprint WITHOUT
/// committing the pin. The frontend calls this right before it
/// would otherwise call `configure_sync_adapter` for an SFTP
/// target so it can:
///
/// - On `New` — show the §19.5 "first-use; verify the fingerprint
///   out-of-band" trust dialog before any TOFU happens.
/// - On `Changed` — show the §19.5 "host key changed; verify
///   before accepting" warning, with both stored + presented
///   fingerprints side-by-side.
/// - On `Unchanged` — skip the dialog and proceed straight to
///   configure.
///
/// Reads from the same user_prefs-backed verifier the real
/// adapter uses, so the answers line up.
#[tauri::command]
pub async fn preview_sftp_host_key(
    db: State<'_, DbHandle>,
    host: String,
    port: u16,
) -> CommandResult<HostKeyPreview> {
    let trimmed_host = host.trim();
    if trimmed_host.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "SFTP host must not be empty".into(),
        });
    }
    let shared = db.shared();
    let verifier: Arc<dyn HostKeyVerifier> =
        Arc::new(UserPrefsHostKeyVerifier::new(shared.clone()));
    // Auth + base_path don't matter here — probe_host_key_fingerprint
    // aborts the handshake before authenticating. Pass placeholders.
    let adapter = SftpSyncAdapter::new(
        trimmed_host,
        port,
        "preview",
        SftpAuth::Password {
            password: String::new(),
        },
        PathBuf::from("/"),
        verifier,
    );
    adapter.preview_host_key().await.map_err(sync_err)
}

/// Commit a TOFU acceptance the user has explicitly confirmed in
/// the trust dialog. The orchestrator never calls this on its
/// own — pinning a fingerprint is always a user gesture (§19.5).
///
/// Used both for first-use ("New") and for key-change
/// ("Changed") flows; in the second case the new fingerprint
/// overwrites the stored one.
#[tauri::command]
pub async fn trust_sftp_host_key(
    db: State<'_, DbHandle>,
    host_port: String,
    fingerprint: String,
) -> CommandResult<()> {
    let trimmed_host_port = host_port.trim();
    let trimmed_fp = fingerprint.trim();
    if trimmed_host_port.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "host_port must not be empty".into(),
        });
    }
    if trimmed_fp.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "fingerprint must not be empty".into(),
        });
    }
    let shared = db.shared();
    let verifier = UserPrefsHostKeyVerifier::new(shared);
    verifier.record(trimmed_host_port, trimmed_fp);
    Ok(())
}

// ---------------------------------------------------------------------------
// §19.10 — stale-device resume.
// ---------------------------------------------------------------------------

/// Re-pull the current snapshot after the user confirmed the
/// §19.10 "this device was offline for a while" dialog. Applies
/// the snapshot to local SQLite, replays any post-snapshot logs,
/// clears our `stale` flag in meta.json. Clears the orchestrator
/// latch on success so the next sync round proceeds normally.
///
/// Returns the resume's apply counts so the frontend can show
/// "12 events applied" after the dialog closes — same payload
/// shape as the onboarding `accept_remote_dataset` command.
#[tauri::command]
pub async fn resume_stale_device(
    app: tauri::AppHandle,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    onboarding: State<'_, Arc<OnboardingService>>,
) -> CommandResult<OnboardingReport> {
    let adapter = orchestrator.adapter_handle().ok_or(CommandError {
        code: "not_configured",
        message: "no sync adapter configured".into(),
    })?;
    let report = onboarding
        .resume_from_stale(adapter.as_ref())
        .await
        .map_err(sync_err)?;
    // Drop the latched stale flag so subsequent sync rounds run
    // normally + the status badge clears.
    orchestrator.clear_stale_device();
    // Frontend listens to `sync-status` to refresh its `stale_device_since`
    // mirror. Emit a synthetic status update so the resume dialog
    // closes promptly without waiting for the next periodic round.
    let status = orchestrator.status();
    if let Err(err) = tauri::Emitter::emit(
        &app,
        "sync-status",
        crate::event_log::SyncStatusPayload {
            status,
            report: None,
            error: None,
        },
    ) {
        tracing::warn!(?err, "failed to emit post-resume sync-status");
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// Phase Sm follow-up — §19.9 "Detailliertes Sync-Protokoll".
// ---------------------------------------------------------------------------

/// Read recent sync rounds from the `sync_log` table, newest
/// first. `limit` caps the returned set; values above the
/// retention ceiling are silently clamped (no error — the user
/// just gets whatever's in the table).
///
/// The Settings → Synchronisation → Protokoll list calls this
/// on mount and on every `sync-log-changed` event the scheduler
/// emits after a round.
#[tauri::command]
pub async fn list_sync_log_entries(
    db: State<'_, DbHandle>,
    limit: Option<u32>,
) -> CommandResult<Vec<SyncLogEntry>> {
    let shared = db.shared();
    let repo = SyncLogRepo::new(&shared);
    // Default to the full retention cap; the table prunes itself
    // so this is the natural upper bound.
    let n = limit.unwrap_or(MAX_LOG_ROWS).min(MAX_LOG_ROWS);
    repo.list(n).map_err(internal)
}

/// Drop every row from `sync_log`. The Protokoll component's
/// "Verlauf leeren" button calls this when the user wants to
/// scrub history before sharing a screen recording / bug report.
#[tauri::command]
pub async fn clear_sync_log(db: State<'_, DbHandle>) -> CommandResult<()> {
    let shared = db.shared();
    let repo = SyncLogRepo::new(&shared);
    repo.clear().map_err(internal)
}

/// Drop the pinned fingerprint for a host_port. Used by the
/// SyncPanel's "Pin vergessen" gesture — when a user knows
/// their server's key has rotated, they can clear the old pin
/// proactively rather than waiting for the next connect to fail
/// with a mismatch dialog. The next connect goes through the
/// first-use trust dialog again.
#[tauri::command]
pub async fn forget_sftp_host_key(
    db: State<'_, DbHandle>,
    host_port: String,
) -> CommandResult<()> {
    let trimmed = host_port.trim();
    if trimmed.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "host_port must not be empty".into(),
        });
    }
    let shared = db.shared();
    let verifier = UserPrefsHostKeyVerifier::new(shared);
    verifier.forget(trimmed);
    Ok(())
}

/// Read the currently-pinned fingerprint for a host_port, or
/// `None` if nothing is pinned yet. Lets the SyncPanel render
/// "Aktueller Pin: SHA256:…" without having to probe the
/// server, so the "Vergessen" button can stay informative even
/// when the server is unreachable.
#[tauri::command]
pub async fn get_pinned_sftp_host_key(
    db: State<'_, DbHandle>,
    host_port: String,
) -> CommandResult<Option<String>> {
    let trimmed = host_port.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let shared = db.shared();
    let verifier = UserPrefsHostKeyVerifier::new(shared);
    Ok(verifier.peek(trimmed))
}
