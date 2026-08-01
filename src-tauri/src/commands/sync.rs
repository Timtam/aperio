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

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use plugin_core::shim::FfiSyncAdapter;
use plugin_core::PluginManager;
use serde::{Deserialize, Serialize};
use sync_core::{
    derive_key, fresh_data_key, resolve_data_key, wrap_key, EncryptingAdapter, EncryptionParams,
    SyncAdapter, KEY_LEN,
};
use tauri::State;

use super::{run_plugin_auth, run_plugin_probe_host_key, CommandError, CommandResult};
use crate::accounts::AccountsRepo;
use crate::cache::CacheRefresher;
use crate::db::{DbHandle, SharedConn};
use crate::event_log::{
    CompactionReport, OnboardingReport, OnboardingService, SyncOrchestrator, SyncPreview,
    SyncRoundReport, SyncScheduler, SyncStatus,
};
use crate::registry::AdapterRegistry;
use crate::secrets::{self, SecretSlot};
use crate::sftp_host_keys::UserPrefsHostKeyVerifier;
use crate::sync_log::{SyncLogEntry, SyncLogRepo, MAX_LOG_ROWS};
use crate::user_prefs::UserPrefsRepo;
use cal_adapter_local::LocalAdapter;
use cal_core::CalendarFeature;

// ── Plugin-id constants ──────────────────────────────────────
//
// String literals the bundled sync plugins advertise in their
// `aperio_plugin_create` descriptor + `plugin.json`. Centralised
// here so the per-kind plugin dispatch matches each plugin
// verbatim.

/// Look up the user-pinned SFTP host-key fingerprint (§19.5)
/// for the given `host:port` and surface it as the plugin's
/// `pinned_fingerprint` init field. An empty return means the
/// user hasn't yet gone through the trust dialog; the plugin's
/// verifier then silently TOFUs, which is only safe because the
/// frontend gates the connect path behind the trust step. Re-
/// pinning a changed fingerprint goes through the existing
/// `trust_sftp_host_key` command path; on the next build the
/// updated value flows in here automatically.
fn pinned_sftp_fingerprint(db: &SharedConn, host: &str, port: u16) -> String {
    let verifier = UserPrefsHostKeyVerifier::new(db.clone());
    let host_port = format!("{host}:{port}");
    verifier.peek(&host_port).unwrap_or_default()
}

/// Open an instance of the named sync plugin with the supplied
/// JSON config + wrap the result in a `Arc<dyn SyncAdapter>` the
/// orchestrator can store. Centralises the four error paths
/// (plugin missing, malformed config, instance open, missing
/// sync vtable) so each match arm in [`build_adapter`] /
/// [`build_adapter_from_prefs`] stays a single call.
fn open_sync_plugin(
    plugin_manager: &PluginManager,
    plugin_id: &str,
    config_json: String,
) -> CommandResult<Arc<dyn SyncAdapter>> {
    let plugin = plugin_manager.get(plugin_id).ok_or(CommandError {
        code: "plugin_missing",
        message: format!("plugin {plugin_id} is not loaded"),
    })?;
    let instance = plugin_manager
        .open_instance(plugin, &config_json)
        .map_err(|err| match err {
            plugin_core::error::PluginError::InstanceOpen { message, .. } => CommandError {
                code: "invalid_input",
                message,
            },
            other => CommandError {
                code: "internal",
                message: other.to_string(),
            },
        })?;
    let adapter = FfiSyncAdapter::new(instance).ok_or(CommandError {
        code: "internal",
        message: format!("plugin {plugin_id} doesn't expose a SyncAdapter vtable surface",),
    })?;
    Ok(Arc::new(adapter))
}

/// Whether this looks like a FRESH instance that should be offered the
/// first-launch wizard (§19.11): no EXTERNAL account configured, no sync
/// target set, and an empty local store (no task lists, no calendars).
///
/// DATA-based on purpose, not a stored flag: an established install that
/// already has real data — or that already set up sync/accounts — must never
/// be re-prompted. The implicit `local` account (migration 0003) is always
/// present and doesn't count. The frontend pairs this with a device-local
/// "already shown" marker so an empty instance that dismissed the wizard
/// isn't offered it again.
#[tauri::command]
pub async fn is_fresh_instance(
    db: State<'_, DbHandle>,
    adapter: State<'_, LocalAdapter>,
) -> CommandResult<bool> {
    let shared = db.shared();

    // Any non-local account → not fresh.
    let accounts = AccountsRepo::new(&shared).list()?;
    if accounts.iter().any(|a| a.adapter_kind != "local") {
        return Ok(false);
    }

    // A sync target configured (anything other than unset / empty / "none").
    let kind = UserPrefsRepo::new(&shared)
        .get(PREF_ADAPTER_KIND)
        .ok()
        .flatten();
    if matches!(kind, Some(k) if !k.is_empty() && k != "none") {
        return Ok(false);
    }

    // Any local task list or calendar → the user has already created data.
    if !adapter.list_task_lists_sync()?.is_empty() {
        return Ok(false);
    }
    if !adapter.list_calendars().await?.is_empty() {
        return Ok(false);
    }

    Ok(true)
}

/// `user_prefs` key flagging whether the current sync dataset is
/// E2E-encrypted. Now owned by `host_core::credential_sync` (the single
/// auditable home for the credential-sync gate, shared with the mobile
/// host); re-exported here so existing `commands::PREF_E2E_ENABLED`
/// references + the `pub use sync::*` surface keep resolving.
pub use host_core::credential_sync::PREF_E2E_ENABLED;

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
    /// Dropbox adapter (DESIGN.md §19.6 — Dropbox API v2 +
    /// OAuth 2.0). The user creates their own Dropbox app at
    /// dropbox.com/developers/apps and supplies the
    /// `client_id`. `client_secret` is optional — public apps
    /// use PKCE only. The OAuth dance happens via the dedicated
    /// `connect_dropbox_oauth` command before `configure_sync_adapter`
    /// is called with this variant; by the time the adapter
    /// is built the refresh token already lives in the
    /// keychain.
    Dropbox {
        client_id: String,
        #[serde(default)]
        client_secret: String,
        /// Remote folder, e.g. `/aperio`. Empty string = app
        /// root (for app-folder-scoped apps) / Dropbox root
        /// (for full-Dropbox apps).
        #[serde(default)]
        path: String,
    },
    /// Google Drive adapter (DESIGN.md §19.6 — Drive API v3 +
    /// OAuth 2.0). The user creates a Drive app at
    /// console.cloud.google.com and supplies both
    /// `client_id` and `client_secret` (Google requires the
    /// secret for installed apps; their docs say "in this
    /// context the secret is not treated as a secret").
    /// `folder_name` is the human-readable folder under My
    /// Drive that holds the dataset; the adapter creates it
    /// if missing. The OAuth dance runs through
    /// `connect_googledrive_oauth` before the regular
    /// `configure_sync_adapter` call.
    GoogleDrive {
        client_id: String,
        client_secret: String,
        #[serde(default)]
        folder_name: String,
    },
    /// FTPS adapter (DESIGN.md §19.6 — "FTP über TLS"). Plain
    /// FTP is not supported; the `mode` picks between
    /// `"explicit"` (AUTH TLS upgrade, port 21 default) and
    /// `"implicit"` (TLS-first handshake, port 990 default).
    /// Same `Option<String>` password reuse contract as the
    /// other adapters: empty or omitted means "reuse keychain".
    Ftp {
        host: String,
        #[serde(default = "default_ftp_port")]
        port: u16,
        user: String,
        path: String,
        #[serde(default = "default_ftp_mode")]
        mode: String,
        #[serde(default)]
        password: Option<String>,
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

fn default_ftp_port() -> u16 {
    // Explicit FTPS — the default mode — talks plain FTP on
    // port 21 then upgrades via AUTH TLS. Implicit mode (port
    // 990) is opt-in via the `mode` field; the frontend swaps
    // the default port when the user changes mode.
    21
}

fn default_ftp_mode() -> String {
    "explicit".to_string()
}

/// Build a fresh adapter instance from a [`SyncAdapterConfig`] —
/// validates the inputs, opens a per-instance handle on the
/// matching sync plugin via [`PluginManager::open_instance`],
/// and returns it wrapped in `Arc<dyn SyncAdapter>` ready to
/// hand to the orchestrator or the onboarding service.
///
/// The `None` variant returns Err: the caller is asking for an
/// adapter to operate on, and a disconnect has no adapter to
/// make.
fn build_adapter(
    config: &SyncAdapterConfig,
    db: &SharedConn,
    plugin_manager: &PluginManager,
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
            let cfg = serde_json::json!({ "remote_root": trimmed }).to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_LOCAL, cfg)
        }
        SyncAdapterConfig::Webdav {
            url,
            user,
            password,
        } => {
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
                _ => secrets::retrieve(WEBDAV_SECRET_ACCOUNT, SecretSlot::Password).ok(),
            };
            let cfg = serde_json::json!({
                "url": trimmed_url,
                "user": trimmed_user,
                "password": resolved_password.unwrap_or_default(),
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_WEBDAV, cfg)
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
            // Resolve the auth-method credentials BEFORE handing
            // off to the plugin so the same keychain-fallback
            // contract the WebDAV / FTP branches use applies here
            // too: Some+non-empty in the request body → use it;
            // None or empty → look the previously-stored value up
            // in the keychain so host/user edits don't require
            // re-typing.
            let (resolved_password, resolved_key_path, resolved_key_passphrase) =
                match auth_method.as_str() {
                    "password" => {
                        let pw = match password.as_deref().map(str::trim) {
                            Some(p) if !p.is_empty() => p.to_string(),
                            _ => secrets::retrieve(SFTP_SECRET_ACCOUNT, SecretSlot::Password)
                                .map_err(|err| CommandError {
                                    code: "auth",
                                    message: format!("no SFTP password configured: {err}",),
                                })?,
                        };
                        (pw, String::new(), String::new())
                    }
                    "key" => {
                        let kp = key_path
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .ok_or(CommandError {
                                code: "invalid_input",
                                message: "SSH key path must not be empty".into(),
                            })?
                            .to_string();
                        let pass = match key_passphrase.as_deref().map(str::trim) {
                            Some(p) if !p.is_empty() => p.to_string(),
                            _ => secrets::retrieve(SFTP_KEY_SECRET_ACCOUNT, SecretSlot::Password)
                                .ok()
                                .unwrap_or_default(),
                        };
                        (String::new(), kp, pass)
                    }
                    other => {
                        return Err(CommandError {
                            code: "invalid_input",
                            message: format!("unknown SFTP auth method: {other}",),
                        });
                    }
                };
            // Look up the user-pinned host fingerprint from the
            // §19.5 trust dialog state so the plugin's in-memory
            // verifier locks the handshake to that exact key.
            let pinned_fp = pinned_sftp_fingerprint(db, trimmed_host, *port);
            // Enforce §19.5 in the backend, not only the UI: an empty pin =
            // silent TOFU (accept any host key = MITM exposure). The frontend
            // always trusts the fingerprint before connecting, so this rejects
            // only an unsafe path (a caller reaching build_adapter without a pin).
            if pinned_fp.trim().is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "SFTP host key not trusted yet — verify + accept the \
                              host fingerprint first (§19.5)"
                        .into(),
                });
            }
            let cfg = serde_json::json!({
                "host": trimmed_host,
                "port": *port,
                "user": trimmed_user,
                "path": trimmed_path,
                "auth_method": auth_method,
                "password": resolved_password,
                "key_path": resolved_key_path,
                "key_passphrase": resolved_key_passphrase,
                "pinned_fingerprint": pinned_fp,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_SFTP, cfg)
        }
        SyncAdapterConfig::Dropbox {
            client_id,
            client_secret,
            path,
        } => {
            let trimmed_client_id = client_id.trim();
            if trimmed_client_id.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "Dropbox client_id must not be empty".into(),
                });
            }
            // OAuth must have completed before this build path
            // runs — the refresh token is the entry credential
            // we need to mint access tokens. Missing keychain
            // entry → user hasn't signed in yet.
            let refresh_token = secrets::retrieve(DROPBOX_SECRET_ACCOUNT, SecretSlot::RefreshToken)
                .map_err(|err| CommandError {
                    code: "auth",
                    message: format!("Dropbox sign-in required — no refresh token: {err}",),
                })?;
            let cfg = serde_json::json!({
                "client_id": trimmed_client_id,
                "client_secret": client_secret.trim(),
                "base_path": path.trim(),
                "refresh_token": refresh_token,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_DROPBOX, cfg)
        }
        SyncAdapterConfig::GoogleDrive {
            client_id,
            client_secret,
            folder_name,
        } => {
            let trimmed_id = client_id.trim();
            let trimmed_secret = client_secret.trim();
            if trimmed_id.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "Google Drive client_id must not be empty".into(),
                });
            }
            if trimmed_secret.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "Google Drive client_secret must not be empty".into(),
                });
            }
            let refresh_token = secrets::retrieve(
                GOOGLEDRIVE_SECRET_ACCOUNT,
                SecretSlot::RefreshToken,
            )
            .map_err(|err| CommandError {
                code: "auth",
                message: format!("Google Drive sign-in required — no refresh token: {err}",),
            })?;
            let cfg = serde_json::json!({
                "client_id": trimmed_id,
                "client_secret": trimmed_secret,
                "folder_name": folder_name.trim(),
                "refresh_token": refresh_token,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_GOOGLEDRIVE, cfg)
        }
        SyncAdapterConfig::Ftp {
            host,
            port,
            user,
            path,
            mode,
            password,
        } => {
            let trimmed_host = host.trim();
            let trimmed_user = user.trim();
            let trimmed_path = path.trim();
            if trimmed_host.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "FTP host must not be empty".into(),
                });
            }
            if trimmed_user.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "FTP user must not be empty".into(),
                });
            }
            // Same Option-reuse password contract as WebDAV /
            // SFTP: Some+non-empty → use the supplied value;
            // None or empty → re-fetch the keychain secret so
            // host/user edits don't require re-typing.
            let resolved_password =
                match password.as_deref().map(str::trim) {
                    Some(p) if !p.is_empty() => p.to_string(),
                    _ => secrets::retrieve(FTP_SECRET_ACCOUNT, SecretSlot::Password).map_err(
                        |err| CommandError {
                            code: "auth",
                            message: format!("no FTP password configured: {err}",),
                        },
                    )?,
                };
            // Plugin validates the `mode` string itself + falls
            // back to "explicit" on unknown values, but we still
            // catch the obviously-wrong cases here so the user
            // gets the same "Settings dialog" error the previous
            // direct path produced.
            if !matches!(mode.as_str(), "implicit" | "explicit" | "plain") {
                return Err(CommandError {
                    code: "invalid_input",
                    message: format!("unknown FTPS mode: {mode}",),
                });
            }
            let cfg = serde_json::json!({
                "host": trimmed_host,
                "port": *port,
                "user": trimmed_user,
                "password": resolved_password,
                "path": trimmed_path,
                "mode": mode,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_FTP, cfg)
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
fn persist_adapter_config(prefs: &UserPrefsRepo, config: &SyncAdapterConfig) -> CommandResult<()> {
    match config {
        SyncAdapterConfig::Local { path } => {
            let trimmed = path.trim();
            prefs.set(PREF_ADAPTER_KIND, "local").map_err(internal)?;
            prefs.set(PREF_LOCAL_PATH, trimmed).map_err(internal)?;
            Ok(())
        }
        SyncAdapterConfig::Webdav {
            url,
            user,
            password,
        } => {
            prefs.set(PREF_ADAPTER_KIND, "webdav").map_err(internal)?;
            prefs.set(PREF_WEBDAV_URL, url.trim()).map_err(internal)?;
            prefs.set(PREF_WEBDAV_USER, user.trim()).map_err(internal)?;
            // Only overwrite the keychain when the request body
            // explicitly carries a non-empty password. URL/user
            // edits that omit the password keep the prior secret.
            if let Some(pw) = password.as_deref().map(str::trim) {
                if !pw.is_empty() {
                    secrets::store(WEBDAV_SECRET_ACCOUNT, SecretSlot::Password, pw).map_err(
                        |err| CommandError {
                            code: "internal",
                            message: format!("keychain store: {err}"),
                        },
                    )?;
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
            prefs
                .set(PREF_SFTP_PORT, &port.to_string())
                .map_err(internal)?;
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
                    secrets::store(SFTP_SECRET_ACCOUNT, SecretSlot::Password, pw).map_err(
                        |err| CommandError {
                            code: "internal",
                            message: format!("keychain store: {err}"),
                        },
                    )?;
                }
            }
            if let Some(pp) = key_passphrase.as_deref().map(str::trim) {
                if !pp.is_empty() {
                    secrets::store(SFTP_KEY_SECRET_ACCOUNT, SecretSlot::Password, pp).map_err(
                        |err| CommandError {
                            code: "internal",
                            message: format!("keychain store: {err}"),
                        },
                    )?;
                }
            }
            Ok(())
        }
        SyncAdapterConfig::Dropbox {
            client_id,
            client_secret,
            path,
        } => {
            // Refresh token already in the keychain from the
            // OAuth dance — persist_adapter_config doesn't
            // touch it; we only mirror the non-secret app
            // config into user_prefs here.
            prefs.set(PREF_ADAPTER_KIND, "dropbox").map_err(internal)?;
            prefs
                .set(PREF_DROPBOX_CLIENT_ID, client_id.trim())
                .map_err(internal)?;
            prefs
                .set(PREF_DROPBOX_CLIENT_SECRET, client_secret.trim())
                .map_err(internal)?;
            prefs
                .set(PREF_DROPBOX_PATH, path.trim())
                .map_err(internal)?;
            Ok(())
        }
        SyncAdapterConfig::GoogleDrive {
            client_id,
            client_secret,
            folder_name,
        } => {
            // Same shape as the Dropbox branch — refresh token
            // already lives in the keychain from
            // `connect_googledrive_oauth`; we only persist the
            // non-secret app-config bits.
            prefs
                .set(PREF_ADAPTER_KIND, "googledrive")
                .map_err(internal)?;
            prefs
                .set(PREF_GOOGLEDRIVE_CLIENT_ID, client_id.trim())
                .map_err(internal)?;
            prefs
                .set(PREF_GOOGLEDRIVE_CLIENT_SECRET, client_secret.trim())
                .map_err(internal)?;
            prefs
                .set(PREF_GOOGLEDRIVE_FOLDER_NAME, folder_name.trim())
                .map_err(internal)?;
            Ok(())
        }
        SyncAdapterConfig::Ftp {
            host,
            port,
            user,
            path,
            mode,
            password,
        } => {
            prefs.set(PREF_ADAPTER_KIND, "ftp").map_err(internal)?;
            prefs.set(PREF_FTP_HOST, host.trim()).map_err(internal)?;
            prefs
                .set(PREF_FTP_PORT, &port.to_string())
                .map_err(internal)?;
            prefs.set(PREF_FTP_USER, user.trim()).map_err(internal)?;
            prefs.set(PREF_FTP_PATH, path.trim()).map_err(internal)?;
            prefs.set(PREF_FTP_MODE, mode.trim()).map_err(internal)?;
            // Only overwrite the keychain when the request
            // carries a non-empty password — same reuse
            // contract as WebDAV / SFTP.
            if let Some(pw) = password.as_deref().map(str::trim) {
                if !pw.is_empty() {
                    secrets::store(FTP_SECRET_ACCOUNT, SecretSlot::Password, pw).map_err(
                        |err| CommandError {
                            code: "internal",
                            message: format!("keychain store: {err}"),
                        },
                    )?;
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
    secrets::store(E2E_SECRET_ACCOUNT, SecretSlot::SyncEncryptionKey, &encoded).map_err(|err| {
        CommandError {
            code: "internal",
            message: format!("keychain store sync key: {err}"),
        }
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
    plugin_manager: State<'_, Arc<PluginManager>>,
    config: SyncAdapterConfig,
) -> CommandResult<()> {
    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);
    // A different backend invalidates every remote-missing sound verdict
    // (mirrors the orchestrator clearing its per-session pushed lengths).
    host_core::sound_assets::reset_missing_cache();
    match &config {
        SyncAdapterConfig::Local { .. }
        | SyncAdapterConfig::Webdav { .. }
        | SyncAdapterConfig::Sftp { .. }
        | SyncAdapterConfig::Ftp { .. }
        | SyncAdapterConfig::Dropbox { .. }
        | SyncAdapterConfig::GoogleDrive { .. } => {
            // Persist BEFORE building the adapter so the keychain
            // entry for the new WebDAV password is in place; the
            // adapter constructor then reads it back when the
            // request body omitted the password (e.g. URL-only
            // edit). Then we probe.
            persist_adapter_config(&prefs, &config)?;
            let plain = build_adapter(&config, &shared, plugin_manager.inner())?;
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
                sync_core::ensure_compatible(m, onboarding.app_version()).map_err(sync_err)?;
            }
            let e2e_target = target_meta.as_ref().map(|m| m.e2e_enabled).unwrap_or(false);
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
    scheduler
        .set_interval_minutes(minutes)
        .map_err(|err| CommandError {
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
    // A manual round applies foreign events (and can auto-resume a stale
    // device from a snapshot) exactly like the scheduled one, so it can
    // bring in ACCOUNTS whose adapters nobody has built yet. This command
    // bypasses `run_round`, so it has to do the same post-round work or
    // "Sync now" leaves the new accounts dead until the next app start.
    // Self-gating: a no-op when nothing new arrived.
    scheduler.register_synced_accounts(&app);
    // If new conflicts landed during this manual round, kick
    // the frontend's conflict-count refetch + notification
    // path. Same logic as the periodic scheduler's `run_round`.
    if report.conflicts > 0 {
        if let Err(err) = tauri::Emitter::emit(&app, "sync-conflicts-changed", ()) {
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

/// Non-secret summary of the persisted adapter configuration.
/// Used by the Settings → Sync panel to render a compact
/// "Verbunden mit X" card when the adapter is configured,
/// instead of leaving the full editable form visible (which
/// reads as "you can have multiple adapters" and prompts users
/// to type into fields that won't apply unless they hit
/// Configure again).
///
/// `detail` is intentionally a single display string — the
/// frontend doesn't need to switch on the kind to render it,
/// and we never put secrets (password, key passphrase,
/// client_secret, refresh token) in there.
#[derive(Debug, Serialize)]
pub struct SyncAdapterSummary {
    pub kind: String,
    pub detail: String,
}

/// Build a [`SyncAdapterSummary`] from the persisted user_prefs.
/// Returns `Ok(None)` when no adapter is configured (the form
/// should be visible) or when the kind is `"none"` (explicitly
/// disconnected).
#[tauri::command]
pub fn get_sync_adapter_summary(
    db: State<'_, DbHandle>,
) -> CommandResult<Option<SyncAdapterSummary>> {
    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);
    let Some(kind) = prefs.get(PREF_ADAPTER_KIND).map_err(internal)? else {
        return Ok(None);
    };
    let detail = match kind.as_str() {
        "local" => prefs
            .get(PREF_LOCAL_PATH)
            .map_err(internal)?
            .unwrap_or_default(),
        "webdav" => {
            let url = prefs
                .get(PREF_WEBDAV_URL)
                .map_err(internal)?
                .unwrap_or_default();
            let user = prefs
                .get(PREF_WEBDAV_USER)
                .map_err(internal)?
                .unwrap_or_default();
            if user.is_empty() {
                url
            } else {
                format!("{user}@{url}")
            }
        }
        "sftp" => {
            let host = prefs
                .get(PREF_SFTP_HOST)
                .map_err(internal)?
                .unwrap_or_default();
            let port = prefs
                .get(PREF_SFTP_PORT)
                .map_err(internal)?
                .unwrap_or_else(|| "22".into());
            let user = prefs
                .get(PREF_SFTP_USER)
                .map_err(internal)?
                .unwrap_or_default();
            let path = prefs
                .get(PREF_SFTP_PATH)
                .map_err(internal)?
                .unwrap_or_default();
            format!("{user}@{host}:{port}{path}")
        }
        "ftp" => {
            let host = prefs
                .get(PREF_FTP_HOST)
                .map_err(internal)?
                .unwrap_or_default();
            let port = prefs
                .get(PREF_FTP_PORT)
                .map_err(internal)?
                .unwrap_or_else(|| "21".into());
            let user = prefs
                .get(PREF_FTP_USER)
                .map_err(internal)?
                .unwrap_or_default();
            let path = prefs
                .get(PREF_FTP_PATH)
                .map_err(internal)?
                .unwrap_or_default();
            format!("{user}@{host}:{port}{path}")
        }
        "dropbox" => prefs
            .get(PREF_DROPBOX_PATH)
            .map_err(internal)?
            .unwrap_or_default(),
        "googledrive" => prefs
            .get(PREF_GOOGLEDRIVE_FOLDER_NAME)
            .map_err(internal)?
            .unwrap_or_default(),
        "none" => return Ok(None),
        _ => String::new(),
    };
    Ok(Some(SyncAdapterSummary { kind, detail }))
}

/// Manually trigger a compaction round (Phase Sg, §19.10). Snapshots
/// the current local state, pushes `snapshot.json`, advances
/// `meta.json.snapshot_timestamp`, and GCs log files below the GC
/// cutoff `max(lowest device held horizon, snapshot_ts - retention)`,
/// publishing the resulting monotonic `gc_horizon`.
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
    let result = orchestrator.compactor().compact_now(adapter.as_ref()).await;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    // §19.10 — surface the outcome in the Protokoll regardless of
    // success/failure so the user has an audit trail of every
    // compaction run. Mirrors the manual-sync_now bookkeeping.
    scheduler.record_compaction_outcome(&app, &result, duration_ms);
    result.map_err(sync_err)
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
    plugin_manager: State<'_, Arc<PluginManager>>,
    config: SyncAdapterConfig,
) -> CommandResult<SyncPreview> {
    let shared = db.shared();
    let adapter = build_adapter(&config, &shared, plugin_manager.inner())?;
    onboarding.preview(adapter.as_ref()).await.map_err(sync_err)
}

/// Bring up adapters for external accounts an onboarding pass just
/// materialised, then warm their cache.
///
/// Onboarding creates external accounts as ROWS: it applies the remote
/// snapshot + logs straight to SQLite and never passes through the
/// add-account commands that call `AdapterRegistry::register`. Without this
/// the accounts show up in the sidebar by name but list no calendars, task
/// lists or address books — and nothing fixes it until the next app start
/// runs `bootstrap` (exactly the "restart made it work" report).
///
/// Only ADAPTER-LESS accounts are registered — rebuilding a live account's
/// adapter would throw away its in-memory provider state and force a cold
/// re-drain. Called unconditionally rather than gated on `report.applied`:
/// a compacted dataset restores its accounts through the SNAPSHOT, which
/// that log-event counter doesn't see.
///
/// The warm pass runs either way: the boot pass already ran before these
/// accounts existed, so their containers would otherwise stay unfetched.
fn register_onboarded_accounts(
    shared: &SharedConn,
    registry: &AdapterRegistry,
    refresher: &CacheRefresher,
) {
    let registered = {
        let repo = AccountsRepo::new(shared);
        registry.register_missing(&repo)
    };
    if registered > 0 {
        tracing::info!(
            registered,
            "registered adapters for accounts restored by onboarding",
        );
    }
    // UN-forced, matching the mobile path: the restored accounts' first
    // warm is automatic bookkeeping, so a network blip during it must be
    // confirmed by a second attempt before it surfaces. A wrong/missing
    // credential is auth-shaped and still surfaces at once.
    refresher.trigger_background();
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
    plugin_manager: State<'_, Arc<PluginManager>>,
    refresher: State<'_, Arc<CacheRefresher>>,
    registry: State<'_, Arc<AdapterRegistry>>,
    config: SyncAdapterConfig,
    device_name: Option<String>,
    passphrase: Option<String>,
) -> CommandResult<OnboardingReport> {
    let shared = db.shared();
    let plain = build_adapter(&config, &shared, plugin_manager.inner())?;
    plain.test_connection().await.map_err(sync_err)?;

    // Phase Sk: peek at meta.json to see if the dataset is
    // encrypted. If it is, we must derive the key BEFORE the
    // accept_remote flow tries to read snapshots or logs — the
    // applier needs decrypted bytes.
    let meta = plain.fetch_meta().await.map_err(sync_err)?;
    let e2e_active = meta.as_ref().map(|m| m.e2e_enabled).unwrap_or(false);
    let key: Option<[u8; KEY_LEN]> = if e2e_active {
        let pp = passphrase
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let pp = pp.ok_or(CommandError {
            code: "encryption_required",
            message: "this dataset is encrypted; a passphrase is required".into(),
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
    let prefs = UserPrefsRepo::new(&shared);

    // Set the local E2E flag BEFORE applying the dataset. The credential
    // restore in the snapshot apply is gated on this device's
    // `PREF_E2E_ENABLED` (defense in depth: never write synced secrets on a
    // plaintext-mode device). Adopting an E2E dataset makes this an E2E
    // device, so the flag must already be true while `accept_remote` applies
    // the snapshot — otherwise the snapshot's credentials hit the "E2E is off
    // locally; ignoring them" branch and every account's password is silently
    // dropped, re-prompting the user for all of them. Revert on failure so a
    // botched adopt leaves no half-configured state.
    if e2e_active {
        prefs.set(PREF_E2E_ENABLED, "true").map_err(internal)?;
    }

    // Run the onboarding side. If it fails (e.g. remote has no meta.json or
    // the passphrase is wrong → applier fails to parse JSON), we haven't yet
    // altered the orchestrator's state — the next attempt can pick a
    // different path.
    let trimmed = device_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let report = match onboarding.accept_remote(adapter.as_ref(), trimmed).await {
        Ok(report) => report,
        Err(err) => {
            if e2e_active {
                let _ = prefs.delete(PREF_E2E_ENABLED);
            }
            return Err(sync_err(err));
        }
    };

    // Commit the rest of the choice now that onboarding has succeeded.
    orchestrator.configure(Arc::clone(&adapter));
    persist_adapter_config(&prefs, &config)?;
    // E2E key for the restore-on-boot path; `PREF_E2E_ENABLED` was set above.
    if let Some(k) = key {
        store_e2e_key(&k)?;
    } else {
        // Joining a non-E2E dataset wipes any stale flag from a previous
        // session.
        let _ = prefs.delete(PREF_E2E_ENABLED);
    }
    scheduler.kick();
    register_onboarded_accounts(&shared, &registry, &refresher);
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
    plugin_manager: State<'_, Arc<PluginManager>>,
    event_log: State<'_, Arc<crate::event_log::EventLogWriter>>,
    config: SyncAdapterConfig,
    device_name: Option<String>,
    passphrase: Option<String>,
) -> CommandResult<OnboardingReport> {
    let shared = db.shared();
    let plain = build_adapter(&config, &shared, plugin_manager.inner())?;
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

    let trimmed = device_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
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
    // Onboard this device's EXISTING local lists/tasks onto the freshly
    // created remote. `adopt_local` only writes meta.json, and the startup
    // backfill is one-shot-gated — on a clean install it runs before any
    // list exists, so lists created later (but still before sync was turned
    // on) would otherwise never be re-emitted. Force a replay now so a
    // second device actually receives them; receivers dedupe.
    crate::commands::force_backfill_local_task_events(db.inner(), event_log.inner());
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
    plugin_manager: State<'_, Arc<PluginManager>>,
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
    let verified_dek = resolve_data_key(pp, &current_params).map_err(sync_err)?;

    // 2b. Stop credential sync immediately by clearing the local E2E pref.
    //     This does three things at once: (1) no new `credential.*` event can
    //     be emitted for the rest of the downgrade — the emit gate reads this
    //     pref, so a concurrent account action no-ops instead of leaking;
    //     (2) a concurrent compaction dumps no credentials (same gate); and
    //     (3) the adapter we build in step 3 is genuinely unwrapped, because
    //     `build_adapter_from_prefs` wraps in `EncryptingAdapter` only while
    //     this pref says E2E is on. The orchestrator still holds the
    //     encrypting adapter until step 7, so the flush below stays encrypted.
    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);
    let _ = prefs.delete(PREF_E2E_ENABLED);

    // 2c. Flush the pending local log via the orchestrator's still-encrypting
    //     adapter, so any credential event emitted just before 2b goes up
    //     ENCRYPTED and is then caught by the log strip below — rather than
    //     being pushed plaintext after the step-7 swap.
    orchestrator.push_now().await.map_err(sync_err)?;

    // 3. Build the now-genuinely-PLAIN adapter from the persisted config —
    //    an unwrapped handle to push plaintext bytes (the pref was cleared in
    //    2b, so the builder doesn't wrap it). Same builder the app-start path
    //    uses, so the auth + config bits are guaranteed to match.
    let plain = build_adapter_from_prefs(&shared, plugin_manager.inner()).ok_or(CommandError {
        code: "not_configured",
        message: "couldn't rebuild the underlying plain adapter".into(),
    })?;

    let mut report = DisableE2eReport::default();

    // 4. Rewrite every log as stripped plaintext. We fetch the RAW bytes via
    //    the plain (unwrapped) adapter and decrypt each log ourselves, with a
    //    PLAINTEXT FALLBACK. This makes a retried disable idempotent: a run
    //    that was interrupted (e.g. a network blip) after rewriting some logs
    //    as plaintext can be re-run, whereas the strict
    //    `encrypting.fetch_new_logs` would choke trying to decrypt the
    //    already-plaintext ones and leave the dataset stuck half-converted.
    //    The fallback is safe — AES-GCM authenticates, so a plaintext (non-
    //    ciphertext) log fails to decrypt and is passed through verbatim,
    //    while a genuinely encrypted log decrypts. Normal-operation tamper
    //    detection is unaffected: that path still uses the strict encrypting
    //    fetch; only this downgrade, where a mixed state is expected, is
    //    lenient. `push_log` overwrites at the same path, so no orphans.
    let raw_logs = plain
        .fetch_new_logs(&sync_core::DeviceCursor::epoch())
        .await
        .map_err(sync_err)?;
    for raw in raw_logs {
        // Decrypt the log, tolerating ones a prior interrupted disable already
        // left as plaintext (see `downgrade_log_to_plaintext`).
        let log = crate::credential_sync::downgrade_log_to_plaintext(&verified_dek, raw);
        // SECURITY: strip any `credential.*` events before writing the
        // plaintext log. Those events only ever existed because E2E was on;
        // on the way down to plaintext their secrets must NOT reach the
        // remote. The secrets stay in this device's keychain — they are
        // simply purged from the (now plaintext) sync storage, which is the
        // "remove from remote on E2E off" behaviour by design.
        let stripped = crate::credential_sync::strip_credential_events(&log).map_err(sync_err)?;
        plain.push_log(&stripped).await.map_err(sync_err)?;
        report.logs_rewritten += 1;
    }

    // 5. Same for the snapshot, if one exists. Brand-new datasets that
    //    never compacted skip this branch. SECURITY: an E2E snapshot can
    //    carry account secrets in its `credentials` block — strip them
    //    before the plaintext re-upload, exactly like the log strip above.
    if let Some(mut snapshot) = encrypting.fetch_snapshot().await.map_err(sync_err)? {
        crate::credential_sync::strip_credentials_from_snapshot(&mut snapshot);
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

    // 7. Swap the orchestrator to the plain adapter and drop the keychain
    //    key. The E2E pref was already cleared back in step 2b, so for the
    //    whole body of this command "the orchestrator is plain" has implied
    //    "no credential was emitted or dumped" — closing the windows the
    //    security review flagged (concurrent compaction, in-flight emits).
    orchestrator.configure(Arc::clone(&plain));
    delete_e2e_key();

    Ok(report)
}

/// Outcome counters for [`enable_sync_encryption`]. Mirrors
/// [`DisableE2eReport`] so the UI can render the same "N logs
/// rewritten, snapshot rewritten" line in either direction.
#[derive(Debug, Default, serde::Serialize)]
pub struct EnableE2eReport {
    pub logs_rewritten: usize,
    pub snapshot_rewritten: bool,
}

/// §19.7 — turn on end-to-end encryption for an already-configured
/// dataset that was originally onboarded without it.
///
/// Mirror image of [`disable_sync_encryption`]: fetch every log
/// + snapshot via the plain adapter (they're plaintext on the
/// wire today), then push them back via an `EncryptingAdapter`
/// wrapping the same plain adapter (which AES-GCM-encrypts on
/// the way up, overwriting the plaintext originals at the same
/// paths). The meta.json update is the atomic commit: it lands
/// last with `e2e_enabled = true` + the v2 `e2e_params` (KEK
/// salt + wrapped DEK). A crash before that commit leaves the
/// remote half-encrypted but still flagged plaintext — the next
/// successful round on this device would push plaintext copies
/// back, restoring consistency. A crash after the commit means
/// the dataset is officially encrypted; the few remaining
/// plaintext logs (if any) get overwritten on the next sync
/// round.
///
/// **Other devices need to re-onboard with the new passphrase**
/// after this completes. They'll detect the flip on their next
/// preview (`e2e_enabled` flips false → true) and the standard
/// `E2ePassphrasePrompt` flow takes over; the UI also surfaces
/// a dedicated banner so they understand why the prompt
/// suddenly appeared.
///
/// Error mapping:
///   - empty passphrase → `invalid_input`
///   - adapter not configured → `not_configured`
///   - meta.json missing → `not_found`
///   - meta.json already says `e2e_enabled = true` → `conflict`
///   - network / IO errors during the bulk re-push → `io` /
///     `network`
#[tauri::command]
pub async fn enable_sync_encryption(
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<crate::event_log::EventLogWriter>>,
    new_passphrase: String,
) -> CommandResult<EnableE2eReport> {
    let pp = new_passphrase.trim();
    if pp.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "passphrase must not be empty".into(),
        });
    }

    // 1. Borrow the currently-configured (plain) adapter. Reads
    //    pass through unmodified — this is the plaintext source
    //    of truth right now.
    let plain = orchestrator.adapter_handle().ok_or(CommandError {
        code: "not_configured",
        message: "no sync adapter is configured".into(),
    })?;

    // 2. Fetch meta.json and refuse if encryption is already on.
    //    Two reasons it might say `e2e_enabled = true` already:
    //    (a) another device flipped it since our last round, in
    //    which case the user wants the `E2ePassphrasePrompt`
    //    flow, not this command; (b) local state is stale. Either
    //    way, surfacing as `conflict` makes the situation explicit
    //    instead of silently re-keying.
    let meta_before = plain
        .fetch_meta()
        .await
        .map_err(sync_err)?
        .ok_or(CommandError {
            code: "not_found",
            message: "remote has no meta.json — onboard first".into(),
        })?;
    if meta_before.e2e_enabled {
        return Err(CommandError {
            code: "conflict",
            message: "dataset is already encrypted; nothing to enable".into(),
        });
    }

    // 3. Mint v2 key material: fresh DEK + wrapping params, KEK
    //    derived from the passphrase, wrap the DEK. Same shape
    //    `adopt_local_dataset` produces for a brand-new dataset.
    let mut e2e_params = EncryptionParams::fresh();
    let kek = derive_key(pp, &e2e_params).map_err(sync_err)?;
    let dek = fresh_data_key();
    let wrapped = wrap_key(&kek, &dek).map_err(sync_err)?;
    e2e_params.wrapped_data_key = Some(wrapped);

    // 4. Build the encrypting wrapper around the plain adapter.
    //    Pushes through this re-encrypt blobs in flight; reads
    //    decrypt — but we read via `plain` (step 5) so the
    //    wrapper is only used for the write path here.
    let encrypting: Arc<dyn SyncAdapter> =
        Arc::new(EncryptingAdapter::new(Arc::clone(&plain), dek));

    let mut report = EnableE2eReport::default();

    // 5. Re-encrypt every log: fetch via plain (no decrypt
    //    needed — it's already plaintext), push via encrypting
    //    (writes ciphertext at the same path).
    let logs = plain
        .fetch_new_logs(&sync_core::DeviceCursor::epoch())
        .await
        .map_err(sync_err)?;
    for log in logs {
        encrypting.push_log(&log).await.map_err(sync_err)?;
        report.logs_rewritten += 1;
    }

    // 6. Same for the snapshot, if one exists.
    if let Some(snapshot) = plain.fetch_snapshot().await.map_err(sync_err)? {
        encrypting
            .push_snapshot(&snapshot)
            .await
            .map_err(sync_err)?;
        report.snapshot_rewritten = true;
    }

    // 7. Flip this device's local E2E state + swap the orchestrator onto the
    //    encrypting adapter BEFORE publishing the encrypted meta.json. Once the
    //    remote meta says e2e_enabled, a concurrent scheduler round
    //    (`sync_now`, which doesn't take this command's lock) must see both:
    //      (a) PREF_E2E_ENABLED already true — so the §19.7 sync_now encryption
    //          gate (`meta.e2e_enabled && !store.e2e_enabled()`) PASSES instead
    //          of mis-latching `encryption_required` on the very device that is
    //          enabling, which would pop the "adopt encryption" prompt for a
    //          passphrase the user just set; and
    //      (b) an already-encrypting orchestrator — so the round can't push
    //          plaintext into the now-encrypted dataset.
    //    The re-encrypt loop above ran while meta still said plaintext, so a
    //    round overlapping it pushes plaintext into a still-plaintext dataset
    //    (harmless). meta.json itself is always plaintext (§19.7), so it's
    //    pushed via the plain adapter (the wrapper would second-guess the bytes).
    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);
    store_e2e_key(&dek)?;
    prefs.set(PREF_E2E_ENABLED, "true").map_err(internal)?;
    orchestrator.configure(Arc::clone(&encrypting));

    // 8. Commit the enable: overwrite meta.json (the single atomic commit). If
    //    it fails, roll the local state back so this device isn't left wrapping
    //    against a still-plaintext remote (which would fail every later round).
    let mut updated = meta_before;
    updated.e2e_enabled = true;
    updated.e2e_params = Some(e2e_params);
    if let Err(err) = plain.push_meta(&updated).await {
        let _ = prefs.delete(PREF_E2E_ENABLED);
        orchestrator.configure(Arc::clone(&plain));
        return Err(sync_err(err));
    }

    // 10. E2E is now on. Push every existing local account secret into the
    //     (now-encrypted) log so the user's OTHER devices pick them up
    //     without re-entry — the late-enable counterpart to the disable
    //     strip. Routes through the same E2E + slot gate as live emits, so
    //     it's safe even if state shifted underneath us.
    crate::credential_sync::emit_all_local_credentials(
        &event_log,
        &shared,
        &crate::secrets::KeyringSecretStore,
    );

    Ok(report)
}

/// §19.7 — adopt encryption that was activated on another
/// device. Pure unlock flow: derive the dataset's DEK from the
/// passphrase + meta's `e2e_params`, stash it in the keychain,
/// flip the local pref, swap the orchestrator over to an
/// encrypting adapter. No re-encryption, no device registration
/// — those already ran on the device that called
/// [`enable_sync_encryption`].
///
/// Triggered from the UI banner that appears when `sync_now`
/// fails with `last_error_code = encryption_required` on a
/// dataset this device was previously syncing without
/// encryption.
///
/// Error mapping:
///   - empty passphrase → `invalid_input`
///   - adapter not configured → `not_configured`
///   - meta.json missing → `not_found`
///   - meta.json says `e2e_enabled = false` → `not_configured`
///     (the user clicked the banner from a stale state — the
///     remote is plaintext again)
///   - wrong passphrase → `auth` (via `resolve_data_key`)
#[tauri::command]
pub async fn adopt_remote_encryption(
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<crate::event_log::EventLogWriter>>,
    passphrase: String,
) -> CommandResult<()> {
    let pp = passphrase.trim();
    if pp.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "passphrase must not be empty".into(),
        });
    }

    // The currently-configured adapter is plain (we entered this
    // path precisely because local thinks e2e is off).
    let plain = orchestrator.adapter_handle().ok_or(CommandError {
        code: "not_configured",
        message: "no sync adapter is configured".into(),
    })?;

    let meta = plain
        .fetch_meta()
        .await
        .map_err(sync_err)?
        .ok_or(CommandError {
            code: "not_found",
            message: "remote has no meta.json".into(),
        })?;
    if !meta.e2e_enabled {
        return Err(CommandError {
            code: "not_configured",
            message: "remote is not encrypted; nothing to adopt".into(),
        });
    }
    let params = meta.e2e_params.clone().ok_or(CommandError {
        code: "protocol",
        message: "meta.json says e2e but carries no params".into(),
    })?;

    let dek = resolve_data_key(pp, &params).map_err(sync_err)?;
    let encrypting: Arc<dyn SyncAdapter> =
        Arc::new(EncryptingAdapter::new(Arc::clone(&plain), dek));

    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);
    store_e2e_key(&dek)?;
    prefs.set(PREF_E2E_ENABLED, "true").map_err(internal)?;
    orchestrator.configure(encrypting);

    // E2E is now on for this device too. Push any local account secret that
    // predates the encryption — created while syncing in plaintext, so it
    // never got a `credential.set` — into the now-encrypted log, so those
    // accounts reach the other devices without re-entry. Mirrors step 10 of
    // `enable_sync_encryption`; idempotent and routed through the same E2E +
    // slot gate as live emits.
    crate::credential_sync::emit_all_local_credentials(
        &event_log,
        &shared,
        &crate::secrets::KeyringSecretStore,
    );

    Ok(())
}

/// Helper used by `lib.rs::setup` to reconstruct the adapter
/// from the persisted prefs on app start. Returns `Ok(None)`
/// when no adapter was configured before — the orchestrator
/// stays in its initial unconfigured state.
///
/// Same plugin-routing shape as [`build_adapter`]: the persisted
/// pref values become a JSON config that's handed to the
/// matching sync plugin via `open_instance`.
pub fn build_adapter_from_prefs(
    db: &SharedConn,
    plugin_manager: &PluginManager,
) -> Option<Arc<dyn SyncAdapter>> {
    let prefs = UserPrefsRepo::new(db);
    let kind = prefs.get(PREF_ADAPTER_KIND).ok().flatten()?;
    let plain: Arc<dyn SyncAdapter> = match kind.as_str() {
        "local" => {
            let path = prefs.get(PREF_LOCAL_PATH).ok().flatten()?;
            if path.trim().is_empty() {
                return None;
            }
            let cfg = serde_json::json!({ "remote_root": path.trim() }).to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_LOCAL, cfg).ok()?
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
            let password = secrets::retrieve(WEBDAV_SECRET_ACCOUNT, SecretSlot::Password)
                .ok()
                .unwrap_or_default();
            let cfg = serde_json::json!({
                "url": url.trim(),
                "user": user.trim(),
                "password": password,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_WEBDAV, cfg).ok()?
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
            let (resolved_password, resolved_key_path, resolved_key_passphrase) = match auth_method
                .as_str()
            {
                "key" => {
                    let kp = prefs.get(PREF_SFTP_KEY_PATH).ok().flatten()?;
                    if kp.trim().is_empty() {
                        return None;
                    }
                    let pass = secrets::retrieve(SFTP_KEY_SECRET_ACCOUNT, SecretSlot::Password)
                        .ok()
                        .unwrap_or_default();
                    (String::new(), kp.trim().to_string(), pass)
                }
                // "password" + anything unknown both fall to
                // password auth — forward-compat for a future
                // auth method that an older Aperio doesn't know.
                _ => {
                    let pw = secrets::retrieve(SFTP_SECRET_ACCOUNT, SecretSlot::Password).ok()?;
                    (pw, String::new(), String::new())
                }
            };
            let pinned_fp = pinned_sftp_fingerprint(db, host.trim(), port);
            // §19.5: never restore an SFTP target with no pinned host key — that
            // would silently TOFU (accept whatever key the network presents) on
            // the next sync round. Leave sync unconfigured until the user
            // re-trusts via the trust dialog.
            if pinned_fp.trim().is_empty() {
                return None;
            }
            let cfg = serde_json::json!({
                "host": host.trim(),
                "port": port,
                "user": user.trim(),
                "path": path.trim(),
                "auth_method": auth_method,
                "password": resolved_password,
                "key_path": resolved_key_path,
                "key_passphrase": resolved_key_passphrase,
                "pinned_fingerprint": pinned_fp,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_SFTP, cfg).ok()?
        }
        "ftp" => {
            let host = prefs.get(PREF_FTP_HOST).ok().flatten()?;
            if host.trim().is_empty() {
                return None;
            }
            let port = prefs
                .get(PREF_FTP_PORT)
                .ok()
                .flatten()
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(21);
            let user = prefs.get(PREF_FTP_USER).ok().flatten()?;
            if user.trim().is_empty() {
                return None;
            }
            let path = prefs.get(PREF_FTP_PATH).ok().flatten().unwrap_or_default();
            let mode = prefs
                .get(PREF_FTP_MODE)
                .ok()
                .flatten()
                .unwrap_or_else(|| "explicit".to_string());
            let password = secrets::retrieve(FTP_SECRET_ACCOUNT, SecretSlot::Password).ok()?;
            let cfg = serde_json::json!({
                "host": host.trim(),
                "port": port,
                "user": user.trim(),
                "password": password,
                "path": path.trim(),
                "mode": mode,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_FTP, cfg).ok()?
        }
        "dropbox" => {
            let client_id = prefs.get(PREF_DROPBOX_CLIENT_ID).ok().flatten()?;
            if client_id.trim().is_empty() {
                return None;
            }
            let client_secret = prefs
                .get(PREF_DROPBOX_CLIENT_SECRET)
                .ok()
                .flatten()
                .unwrap_or_default();
            let path = prefs
                .get(PREF_DROPBOX_PATH)
                .ok()
                .flatten()
                .unwrap_or_default();
            let refresh_token =
                secrets::retrieve(DROPBOX_SECRET_ACCOUNT, SecretSlot::RefreshToken).ok()?;
            let cfg = serde_json::json!({
                "client_id": client_id.trim(),
                "client_secret": client_secret.trim(),
                "base_path": path.trim(),
                "refresh_token": refresh_token,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_DROPBOX, cfg).ok()?
        }
        "googledrive" => {
            let client_id = prefs.get(PREF_GOOGLEDRIVE_CLIENT_ID).ok().flatten()?;
            if client_id.trim().is_empty() {
                return None;
            }
            // Google requires both id + secret for installed apps,
            // so a missing secret here means the user never
            // finished the Settings form. Treat that as
            // "not configured" rather than booting a half-built
            // adapter.
            let client_secret = prefs.get(PREF_GOOGLEDRIVE_CLIENT_SECRET).ok().flatten()?;
            if client_secret.trim().is_empty() {
                return None;
            }
            let folder_name = prefs
                .get(PREF_GOOGLEDRIVE_FOLDER_NAME)
                .ok()
                .flatten()
                .unwrap_or_default();
            let refresh_token =
                secrets::retrieve(GOOGLEDRIVE_SECRET_ACCOUNT, SecretSlot::RefreshToken).ok()?;
            let cfg = serde_json::json!({
                "client_id": client_id.trim(),
                "client_secret": client_secret.trim(),
                "folder_name": folder_name.trim(),
                "refresh_token": refresh_token,
            })
            .to_string();
            open_sync_plugin(plugin_manager, PLUGIN_ID_GOOGLEDRIVE, cfg).ok()?
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
    let e2e_on = prefs.get(PREF_E2E_ENABLED).ok().flatten().as_deref() == Some("true");
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
// Dropbox OAuth dance.
// ---------------------------------------------------------------------------

/// Run the Dropbox OAuth authorisation-code flow against the
/// user's own Dropbox app (client_id from
/// dropbox.com/developers/apps). On success, stores the
/// refresh token in the keychain under
/// `DROPBOX_SECRET_ACCOUNT::RefreshToken` so subsequent
/// `configure_sync_adapter` calls can build the adapter without
/// re-running the dance.
///
/// Opens the system browser; blocks on the loopback listener
/// for up to 5 minutes waiting for the user to complete the
/// consent screen.
///
/// `client_secret` may be empty for public (PKCE-only) Dropbox
/// apps; confidential apps pass the secret as documented by
/// the Dropbox developer console.
#[tauri::command]
pub async fn connect_dropbox_oauth(
    plugin_manager: State<'_, Arc<PluginManager>>,
    client_id: String,
    client_secret: String,
) -> CommandResult<()> {
    let trimmed_id = client_id.trim();
    if trimmed_id.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "Dropbox client_id must not be empty".into(),
        });
    }
    let tokens = run_plugin_auth(
        plugin_manager.inner(),
        PLUGIN_ID_DROPBOX,
        serde_json::json!({
            "client_id": trimmed_id,
            "client_secret": client_secret.trim(),
        }),
    )
    .await?;
    let refresh_token =
        tokens
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .ok_or(CommandError {
                code: "protocol",
                message: "Dropbox returned no refresh token — the app config may \
                 be missing offline access"
                    .into(),
            })?;
    secrets::store(
        DROPBOX_SECRET_ACCOUNT,
        SecretSlot::RefreshToken,
        refresh_token,
    )
    .map_err(|err| CommandError {
        code: "internal",
        message: format!("keychain store Dropbox refresh: {err}"),
    })?;
    Ok(())
}

/// Returns `true` when a Dropbox refresh token is on file —
/// drives the "signed in" indicator next to the OAuth button
/// in the SyncPanel.
#[tauri::command]
pub async fn has_dropbox_refresh_token() -> CommandResult<bool> {
    Ok(secrets::retrieve(DROPBOX_SECRET_ACCOUNT, SecretSlot::RefreshToken).is_ok())
}

// ---------------------------------------------------------------------------
// Google Drive OAuth dance.
// ---------------------------------------------------------------------------

/// Run the Google Drive OAuth authorisation-code flow against
/// the user's own Drive app (client_id + client_secret from
/// console.cloud.google.com). On success, stores the refresh
/// token in the keychain under
/// `GOOGLEDRIVE_SECRET_ACCOUNT::RefreshToken` so subsequent
/// `configure_sync_adapter` calls can build the adapter without
/// re-running the dance.
///
/// Opens the system browser; blocks on the loopback listener
/// for up to 5 minutes waiting for the user to complete the
/// consent screen.
///
/// Unlike Dropbox, Google's installed-app flow requires
/// `client_secret` to be supplied (their docs explicitly note
/// that the secret isn't treated as a secret in this context
/// — it's still part of the token exchange).
#[tauri::command]
pub async fn connect_googledrive_oauth(
    plugin_manager: State<'_, Arc<PluginManager>>,
    client_id: String,
    client_secret: String,
) -> CommandResult<()> {
    let trimmed_id = client_id.trim();
    let trimmed_secret = client_secret.trim();
    if trimmed_id.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "Google Drive client_id must not be empty".into(),
        });
    }
    if trimmed_secret.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "Google Drive client_secret must not be empty".into(),
        });
    }
    let tokens = run_plugin_auth(
        plugin_manager.inner(),
        PLUGIN_ID_GOOGLEDRIVE,
        serde_json::json!({
            "client_id": trimmed_id,
            "client_secret": trimmed_secret,
        }),
    )
    .await?;
    let refresh_token =
        tokens
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .ok_or(CommandError {
                code: "protocol",
                message: "Google returned no refresh token — make sure the \
                 consent screen is configured for offline access"
                    .into(),
            })?;
    secrets::store(
        GOOGLEDRIVE_SECRET_ACCOUNT,
        SecretSlot::RefreshToken,
        refresh_token,
    )
    .map_err(|err| CommandError {
        code: "internal",
        message: format!("keychain store Google Drive refresh: {err}"),
    })?;
    Ok(())
}

/// Returns `true` when a Google Drive refresh token is on
/// file — drives the "signed in" indicator next to the OAuth
/// button in the SyncPanel.
#[tauri::command]
pub async fn has_googledrive_refresh_token() -> CommandResult<bool> {
    Ok(secrets::retrieve(GOOGLEDRIVE_SECRET_ACCOUNT, SecretSlot::RefreshToken).is_ok())
}

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
/// The TCP+SSH probe runs inside the SFTP plugin via
/// `aperio_plugin_probe_host_key`; the host then compares the
/// presented fingerprint against its own user_prefs-backed pin
/// store to decide between the three outcomes. This split keeps
/// the trust-store responsibility host-side (the plugin never
/// reads/writes user_prefs) while the network probe stays
/// adapter-local.
#[tauri::command]
pub async fn preview_sftp_host_key(
    db: State<'_, DbHandle>,
    plugin_manager: State<'_, Arc<PluginManager>>,
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
    let probe: HostKeyProbeResult = run_plugin_probe_host_key(
        plugin_manager.inner(),
        PLUGIN_ID_SFTP,
        serde_json::json!({ "host": trimmed_host, "port": port }),
    )
    .await?;
    let host_port = format!("{trimmed_host}:{port}");
    let verifier = UserPrefsHostKeyVerifier::new(db.shared());
    let status = match verifier.peek(&host_port) {
        None => HostKeyPreviewStatus::New,
        Some(s) if s == probe.fingerprint => HostKeyPreviewStatus::Unchanged,
        Some(s) => HostKeyPreviewStatus::Changed { stored: s },
    };
    Ok(HostKeyPreview {
        host_port,
        fingerprint: probe.fingerprint,
        status,
    })
}

/// JSON shape the SFTP plugin returns from
/// `aperio_plugin_probe_host_key`. Mirrors
/// `sync_adapter_sftp_plugin::ProbeResult`.
#[derive(Debug, Deserialize)]
struct HostKeyProbeResult {
    fingerprint: String,
}

/// Host-side mirror of the SFTP-adapter `HostKeyPreview` shape.
/// Kept stable so the frontend's existing
/// `{ host_port, fingerprint, status }` payload stays byte-
/// identical after the plugin-routing migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostKeyPreview {
    pub host_port: String,
    pub fingerprint: String,
    pub status: HostKeyPreviewStatus,
}

/// What the freshly-observed fingerprint means relative to
/// whatever the user_prefs trust store has pinned. Tagged JSON
/// shape that lines up with the adapter crate's enum so the
/// frontend's discriminator stays unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostKeyPreviewStatus {
    New,
    Unchanged,
    Changed { stored: String },
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
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    onboarding: State<'_, Arc<OnboardingService>>,
    registry: State<'_, Arc<AdapterRegistry>>,
    refresher: State<'_, Arc<CacheRefresher>>,
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
    // A stale-resume re-onboards from the target, so it can materialise
    // accounts this device never saw — same registration gap as a join.
    register_onboarded_accounts(&db.shared(), &registry, &refresher);
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
pub async fn forget_sftp_host_key(db: State<'_, DbHandle>, host_port: String) -> CommandResult<()> {
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
/// The sync-target names, owned by `host_core::sync_target` so this host and
/// the other one cannot drift apart. Aliased where the local spelling differed,
/// which keeps this commit to the declarations themselves.
use host_core::sync_target::{
    PLUGIN_ID_DROPBOX, PLUGIN_ID_FTP, PLUGIN_ID_GOOGLEDRIVE, PLUGIN_ID_LOCAL, PLUGIN_ID_SFTP,
    PLUGIN_ID_WEBDAV, PREF_ADAPTER_KIND, PREF_DROPBOX_CLIENT_ID, PREF_DROPBOX_CLIENT_SECRET,
    PREF_DROPBOX_PATH, PREF_FTP_HOST, PREF_FTP_MODE, PREF_FTP_PATH, PREF_FTP_PORT, PREF_FTP_USER,
    PREF_GOOGLEDRIVE_CLIENT_ID, PREF_GOOGLEDRIVE_CLIENT_SECRET, PREF_GOOGLEDRIVE_FOLDER_NAME,
    PREF_LOCAL_PATH, PREF_SFTP_AUTH_METHOD, PREF_SFTP_HOST, PREF_SFTP_KEY_PATH, PREF_SFTP_PATH,
    PREF_SFTP_PORT, PREF_SFTP_USER, PREF_WEBDAV_URL, PREF_WEBDAV_USER,
    SECRET_ACCOUNT_DROPBOX as DROPBOX_SECRET_ACCOUNT, SECRET_ACCOUNT_E2E as E2E_SECRET_ACCOUNT,
    SECRET_ACCOUNT_FTP as FTP_SECRET_ACCOUNT,
    SECRET_ACCOUNT_GOOGLEDRIVE as GOOGLEDRIVE_SECRET_ACCOUNT,
    SECRET_ACCOUNT_SFTP as SFTP_SECRET_ACCOUNT, SECRET_ACCOUNT_SFTP_KEY as SFTP_KEY_SECRET_ACCOUNT,
    SECRET_ACCOUNT_WEBDAV as WEBDAV_SECRET_ACCOUNT,
};
