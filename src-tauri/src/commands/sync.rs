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
//! (per §19.2.1) — they're device-local. The target is an account row
//! now, and a sync-only account is excluded from both the event path
//! and the snapshot (`accounts::travels_between_devices`); the pointer
//! naming it, and everything still under `sync.adapter.*`, is excluded
//! by the user_prefs whitelist.

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

    // A sync target configured.
    let kind = UserPrefsRepo::new(&shared)
        .get(PREF_ADAPTER_KIND)
        .ok()
        .flatten();
    if !is_unconfigured(kind.as_deref()) {
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
// `Serialize` is here for one internal purpose: turning this into the
// `{kind, values}` map `host_core::sync_target::connect` takes. Writing that
// conversion by hand would mean keeping the six-arm match this change removes.
//
// It carries passwords, so it must never be logged, returned to the frontend,
// or put in an error message. The one call is in `persist_adapter_config`.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
    ///
    /// `user` is optional for the same reason it is on the mobile host and in
    /// the adapter itself: an empty user means anonymous, which the WebDAV
    /// plugin models as `WebDavCredentials::None` and some servers genuinely
    /// serve. Requiring the field here made a payload without it a wire-level
    /// deserialisation failure, so the two hosts disagreed about whether an
    /// anonymous collection could be used at all.
    Webdav {
        url: String,
        #[serde(default)]
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
    // "The keychain" below is no longer one fixed pseudo-account per kind. Once
    // this device syncs through an account row the credential lives under that
    // row's id, so the lookup goes through `sync_target::stored_secret`, which
    // knows both places and which of them is the newer answer.
    let held = |kind: &str, slot: SecretSlot| {
        host_core::sync_target::stored_secret(
            &UserPrefsRepo::new(db),
            &AccountsRepo::new(db),
            &crate::secrets::KeyringSecretStore,
            kind,
            slot,
        )
    };
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
            //   - what this device already holds (set on a URL-only
            //     edit, or on app start when restoring)
            let resolved_password = match password.as_deref().map(str::trim) {
                Some(p) if !p.is_empty() => Some(p.to_string()),
                _ => held("webdav", SecretSlot::Password),
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
                            _ => held("sftp", SecretSlot::Password).ok_or(CommandError {
                                code: "auth",
                                message: "no SFTP password configured".into(),
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
                            // Its own slot, kept apart from the password so
                            // switching auth method never clobbers the other.
                            _ => held("sftp", SecretSlot::KeyPassphrase).unwrap_or_default(),
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
                // Normalised, not echoed — see the fallback arm above.
                "auth_method": if auth_method == "key" { "key" } else { "password" },
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
            // we need to mint access tokens. Nothing stored →
            // user hasn't signed in yet.
            let refresh_token = held("dropbox", SecretSlot::RefreshToken).ok_or(CommandError {
                code: "auth",
                message: "Dropbox sign-in required — no refresh token".into(),
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
            let refresh_token =
                held("googledrive", SecretSlot::RefreshToken).ok_or(CommandError {
                    code: "auth",
                    message: "Google Drive sign-in required — no refresh token".into(),
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
            let resolved_password = match password.as_deref().map(str::trim) {
                Some(p) if !p.is_empty() => p.to_string(),
                _ => held("ftp", SecretSlot::Password).ok_or(CommandError {
                    code: "auth",
                    message: "no FTP password configured".into(),
                })?,
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

/// How this host answers `host_core`'s two questions about a sync plugin:
/// which one serves a kind and what its account schema says, and how to open an
/// instance of it.
///
/// One trait rather than the bare opener the restore path used to take, because
/// the account path needs the schema — which of the form's values are secret,
/// which stay on this device — and a host should have to supply one thing.
struct HostSyncPlugins<'a>(&'a PluginManager);

impl host_core::sync_target::SyncPlugins for HostSyncPlugins<'_> {
    fn resolve(
        &self,
        adapter_kind: &str,
    ) -> Option<(String, plugin_core::account_schema::AccountSchema)> {
        let plugin = self.0.plugin_for_adapter_kind(adapter_kind)?;
        let schema = plugin.manifest.account.clone()?;
        Some((plugin.manifest.id.clone(), schema))
    }

    fn open(&self, plugin_id: &str, config_json: String) -> Result<Arc<dyn SyncAdapter>, String> {
        open_sync_plugin(self.0, plugin_id, config_json).map_err(|err| err.message)
    }
}

fn connect_err(err: host_core::sync_target::ConnectError) -> CommandError {
    use host_core::sync_target::ConnectError as E;
    match err {
        E::UnknownKind(_) | E::NoPlugin(_) | E::Invalid(_) => CommandError {
            code: "invalid_input",
            message: err.to_string(),
        },
        E::Prefs(_) | E::Accounts(_) | E::Secret(_) => CommandError {
            code: "internal",
            message: err.to_string(),
        },
        // Its own code, because the repair is a GESTURE rather than a message:
        // an unconfirmed host key is fixed by looking at a fingerprint and
        // accepting it, and printing the sentence alone leaves the user with
        // nothing to press. Same reasoning as `unbuildable_err`'s.
        E::HostKeyNotTrusted { .. } => CommandError {
            code: "host_key_not_trusted",
            message: err.to_string(),
        },
        // The plugin's own complaint, kept verbatim: it names what it disliked,
        // which is more than a code could, and NOT `plugin_missing` — a plugin
        // that rejected a config is installed, and telling someone to install
        // it would send them looking for something they have.
        E::PluginRefused(_) => CommandError {
            code: "invalid_input",
            message: err.to_string(),
        },
    }
}

/// Why an account could not be opened as this device's sync target, in a code
/// the frontend can act on.
///
/// One refusal gets a code of its own, because its repair is a GESTURE rather
/// than a message: an unconfirmed host key is fixed by looking at a fingerprint
/// and accepting it, and a settings panel that only printed the sentence would
/// leave the user with nothing to press.
///
/// The rest carry `Unbuildable`'s own text. It names the field or the plugin's
/// own complaint — "no password stored for the sync target", "sync plugin
/// refused: …" — which is more than a code could, and a translated stand-in
/// would say less. `plugin_missing` is NOT decided here: `PluginRefused` also
/// covers a plugin that is installed and rejected the config, and telling
/// someone to install a plugin they already have is worse than saying nothing.
/// [`select_sync_account`] asks that question directly instead.
fn unbuildable_err(err: host_core::sync_target::Unbuildable) -> CommandError {
    use host_core::sync_target::Unbuildable as U;
    let code: &'static str = match &err {
        U::HostKeyNotTrusted { .. } => "host_key_not_trusted",
        U::MissingCredential { .. } => "auth",
        U::AccountMissing { .. } => "not_found",
        U::NotConfigured | U::Incomplete { .. } | U::Invalid { .. } | U::PluginRefused { .. } => {
            "invalid_input"
        }
    };
    CommandError {
        code,
        message: err.to_string(),
    }
}

/// Write the chosen target down as the account row this device syncs through,
/// or disconnect from it.
///
/// The per-kind knowledge lives in `host_core::sync_target`, shared with the
/// mobile host, along with the tests that cover it. This function is the adapter
/// between the desktop's typed request and that module: it serialises the
/// request — which is internally tagged, so it already produces `{"kind": …,
/// <fields>}` — splits the tag off, and hands the rest over.
///
/// It writes nothing itself, which is the point. The two hosts used to carry a
/// six-arm match each, and those two copies had drifted.
fn persist_adapter_config(
    shared: &SharedConn,
    plugin_manager: &PluginManager,
    config: &SyncAdapterConfig,
) -> CommandResult<()> {
    let prefs = UserPrefsRepo::new(shared);
    let accounts = AccountsRepo::new(shared);
    if matches!(config, SyncAdapterConfig::None) {
        // Disconnect no longer means "delete the kind and leave everything
        // else": that is precisely why a disconnected device came back up
        // uploading to the target it had been told to stop using. Everything a
        // restore path could act on goes — except the dataset's encryption key,
        // which is not a property of the target.
        return host_core::sync_target::disconnect(
            &prefs,
            &accounts,
            &crate::secrets::KeyringSecretStore,
        )
        .map_err(connect_err);
    }

    let mut value = serde_json::to_value(config).map_err(|err| CommandError {
        code: "internal",
        message: format!("sync config: {err}"),
    })?;
    let object = value.as_object_mut().ok_or(CommandError {
        code: "internal",
        message: "sync config did not serialise to an object".into(),
    })?;
    let kind = object
        .remove("kind")
        .and_then(|k| k.as_str().map(str::to_string))
        .ok_or(CommandError {
            code: "internal",
            message: "sync config carried no kind".into(),
        })?;

    host_core::sync_target::connect(
        &prefs,
        &accounts,
        &crate::secrets::KeyringSecretStore,
        &HostSyncPlugins(plugin_manager),
        &kind,
        object,
    )
    .map(|_| ())
    .map_err(connect_err)
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
        E::DecryptionFailed(_) => "decryption_failed",
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

/// Whether a key out of this device's keychain actually opens the dataset on
/// the target.
///
/// The keychain outlives the data folder — it belongs to the operating system,
/// not to Aperio — so a device whose data was wiped, or one that used to sync
/// somewhere else entirely, still holds a key. Reusing it is right when it is
/// the same dataset and wrong when it is not, and nothing about `meta.json`
/// says which: there is no dataset identifier in it, and the salt cannot stand
/// in for one because it rotates on every passphrase change while the data key
/// stays put.
///
/// So the key is tried rather than reasoned about. AES-GCM authenticates, so a
/// wrong key fails to open the object rather than yielding rubbish.
///
/// Three answers, and the third is the honest one: a dataset that carries
/// neither a snapshot nor a log has nothing to try the key against. Saying so
/// beats claiming a match — the caller proceeds, and the failure, if there is
/// one, surfaces later where it always did.
enum StoredKeyVerdict {
    Opens,
    Mismatch,
    NothingToTry,
}

async fn verify_stored_key(plain: &Arc<dyn SyncAdapter>, key: [u8; KEY_LEN]) -> StoredKeyVerdict {
    let probe = EncryptingAdapter::new(Arc::clone(plain), key);
    match probe.fetch_snapshot().await {
        Ok(Some(_)) => return StoredKeyVerdict::Opens,
        Err(_) => return StoredKeyVerdict::Mismatch,
        // No snapshot yet — a young dataset. Its logs are encrypted too, so
        // they answer the same question.
        Ok(None) => {}
    }
    match probe
        .fetch_new_logs(&sync_core::DeviceCursor::epoch())
        .await
    {
        Ok(logs) if !logs.is_empty() => StoredKeyVerdict::Opens,
        Ok(_) => StoredKeyVerdict::NothingToTry,
        Err(_) => StoredKeyVerdict::Mismatch,
    }
}

/// Everything between "this device has an adapter" and "the scheduler is
/// running on it": probe, meta, the §19.13 compatibility gate, the E2E
/// decision, activation — and only once all of that has passed, `persist`.
///
/// Two commands reach the live orchestrator this way and they differ in exactly
/// one thing: what they write down afterwards. [`configure_sync_adapter`] writes
/// the connect form as an account row; [`select_sync_account`] writes a pointer
/// at a row that is already there. Everything else — including WHICH failures
/// leave the previous target running — has to be identical, so it is one
/// function rather than two that look alike today.
///
/// `persist` runs AFTER the probe and after `orchestrator.configure`, which is
/// the ordering the connect path fought for: a target that was rejected leaves
/// nothing written down, so the next restart cannot come up on a configuration
/// the user was told had failed.
async fn probe_activate_and_persist(
    plain: Arc<dyn SyncAdapter>,
    shared: &SharedConn,
    orchestrator: &SyncOrchestrator,
    scheduler: &SyncScheduler,
    onboarding: &OnboardingService,
    // Supplied only when the caller is answering an `encryption_key_mismatch`
    // — see the E2E block below.
    passphrase: Option<&str>,
    persist: impl FnOnce() -> CommandResult<()>,
) -> CommandResult<()> {
    // Probe the connection before keeping the adapter active —
    // misconfigurations should surface immediately at the settings dialog, not
    // hours later when the first sync_now runs.
    plain.test_connection().await.map_err(sync_err)?;
    // Phase Sk: inspect the target's `meta.json` to decide whether to wrap with
    // `EncryptingAdapter`. We don't re-derive the key here — that requires the
    // passphrase. If the target is E2E and we already have the key in our
    // keychain (same logical dataset across adapter swap), reuse it. Otherwise
    // refuse — the onboarding flow is the right path for "I'm joining a new
    // encrypted dataset".
    let target_meta = plain.fetch_meta().await.map_err(sync_err)?;
    // Phase Sl: refuse the swap if the target dataset requires a newer Aperio
    // than the running build. The Settings dialog gets the `schema_too_old`
    // error code and renders the §19.13 update prompt; the user can either
    // update or pick a different target.
    if let Some(m) = target_meta.as_ref() {
        sync_core::ensure_compatible(m, onboarding.app_version()).map_err(sync_err)?;
    }
    let e2e_target = target_meta.as_ref().map(|m| m.e2e_enabled).unwrap_or(false);
    let key = if e2e_target {
        let k = load_e2e_key().ok_or(CommandError {
            code: "encryption_required",
            message: "target dataset is encrypted; onboard via accept_remote_dataset first".into(),
        })?;
        // The key came out of the keychain, not out of a passphrase the user
        // just typed. Establish that it opens THIS dataset before handing it to
        // the orchestrator: used against the wrong one it does not fail here,
        // it fails on the next round with a message about an unreadable record,
        // and nothing connects that to a key left behind by an earlier install.
        match verify_stored_key(&plain, k).await {
            StoredKeyVerdict::Opens | StoredKeyVerdict::NothingToTry => Some(k),
            // The way out, and the only one that exists: the passphrase for
            // THIS dataset. Deriving from it replaces the stale key rather
            // than sitting beside it, because two keys for one slot is the
            // state that produced this refusal in the first place.
            StoredKeyVerdict::Mismatch => {
                let pp = passphrase
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or(CommandError {
                        code: "encryption_key_mismatch",
                        message: "this device holds an encryption key from an earlier setup,                                   and it does not open this dataset"
                            .into(),
                    })?;
                let params = target_meta
                    .as_ref()
                    .and_then(|m| m.e2e_params.clone())
                    .ok_or(CommandError {
                        code: "protocol",
                        message: "meta.json says e2e but carries no params".into(),
                    })?;
                let dek = resolve_data_key(pp, &params).map_err(sync_err)?;
                // Checked the same way the stored one was, so a passphrase that
                // is merely wrong is refused here rather than at the next round.
                if matches!(
                    verify_stored_key(&plain, dek).await,
                    StoredKeyVerdict::Mismatch
                ) {
                    return Err(CommandError {
                        code: "decryption_failed",
                        message: "that passphrase does not open this dataset".into(),
                    });
                }
                store_e2e_key(&dek)?;
                Some(dek)
            }
        }
    } else {
        None
    };
    let adapter = wrap_if_encrypted(plain, key);
    orchestrator.configure(adapter);
    // Now it is real: probed, wrapped and active.
    persist()?;
    // Keep PREF_E2E_ENABLED in sync with what we just discovered on the target
    // meta. The keychain key stays either way; the flag is the source of truth
    // for "should we wrap on next boot".
    let prefs = UserPrefsRepo::new(shared);
    if e2e_target {
        prefs.set(PREF_E2E_ENABLED, "true").map_err(internal)?;
    } else {
        let _ = prefs.delete(PREF_E2E_ENABLED);
    }
    // Kick the scheduler so the user sees data flow immediately instead of
    // waiting up to one interval for the periodic loop. The debounce window
    // swallows any pile of mutations the writer queued while the adapter was
    // unconfigured.
    scheduler.kick();
    Ok(())
}

/// Install / swap the active sync adapter. Persists the user's
/// choice so the next app start reconstructs the same adapter
/// in `lib.rs`'s setup phase.
///
/// **Note:** This is the "I already onboarded; just swap the
/// adapter" command. New users go through `preview_sync_target` +
/// `accept_remote_dataset` / `adopt_local_dataset` instead, which
/// configures the adapter as part of the onboarding flow.
///
/// It still takes a FORM, because the first-launch wizard still asks for one.
/// The settings panel no longer does: it calls [`select_sync_account`] with an
/// account the user already added.
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
            // Build first, probe, and only then write anything down — the
            // order the mobile host already uses.
            //
            // This used to persist first, on the stated grounds that the new
            // password had to be in the keychain for the constructor to read
            // back. It does not: `build_adapter` resolves each secret from the
            // REQUEST first and falls back to the stored one only when the
            // request omits it, which is what makes a URL-only edit work. The
            // rationale described a dependency that was not there.
            //
            // What the old order did do was leave a failed attempt written
            // down. Type a wrong password, watch the probe fail, and the
            // preferences and keychain already held the new values while the
            // orchestrator kept running the old target — so the next restart
            // came up on a configuration the user had been told was rejected.
            let plain = build_adapter(&config, &shared, plugin_manager.inner())?;
            probe_activate_and_persist(
                plain,
                &shared,
                &orchestrator,
                &scheduler,
                &onboarding,
                None,
                || persist_adapter_config(&shared, plugin_manager.inner(), &config),
            )
            .await?;
        }
        SyncAdapterConfig::None => {
            orchestrator.deconfigure();
            // A disconnect that leaves the address and the password behind is a
            // disconnect the next launch can undo by itself, which is what used
            // to happen. Reconnecting now means re-entering the target — the
            // price of "stopped means stopped".
            persist_adapter_config(&shared, plugin_manager.inner(), &config)?;
        }
    }
    Ok(())
}

/// Point this device at an account it ALREADY has, and sync through it.
///
/// The settings panel's whole question, in one call. The account was added
/// under Settings → Accounts, or arrived with a restored dataset; nothing here
/// takes a form, a host, or a password, because none of that is being decided —
/// the row already holds it.
///
/// ## What it refuses, and why each refusal has its own code
///
/// [`host_core::sync_target::from_account`] opens the row through the plugin's
/// own schema, so the three ways it can fail are the three the user can fix, and
/// each fix is different:
///
/// - `host_key_not_trusted` — the protocol pins host keys (§19.5) and this
///   device has not confirmed this server's fingerprint. The panel offers the
///   trust gesture through [`preview_sync_account_host_key`].
/// - `plugin_missing` — no loaded plugin serves the account's kind. The account
///   came from another device that has a plugin this one does not.
/// - `auth` — the credential is not in this device's keychain.
///
/// Nothing is written down until the target has been probed AND the
/// compatibility and encryption gates have passed, so a refusal leaves this
/// device syncing exactly where it did before.
#[tauri::command]
pub async fn select_sync_account(
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
    onboarding: State<'_, Arc<OnboardingService>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    account_id: String,
    // Only ever sent as the answer to an `encryption_key_mismatch`: the
    // passphrase for the dataset ON THE TARGET, which replaces the stale key
    // this device was holding. Absent on a first attempt.
    passphrase: Option<String>,
) -> CommandResult<()> {
    let account_id = account_id.trim().to_string();
    if account_id.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "no account id supplied".into(),
        });
    }
    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);
    let accounts = AccountsRepo::new(&shared);
    let account = accounts.get(&account_id)?.ok_or(CommandError {
        code: "not_found",
        message: format!("no account {account_id}"),
    })?;
    // Asked here rather than read off the builder's refusal: `PluginRefused`
    // is also what a plugin that IS installed says when it dislikes the
    // config, and "install the plugin" is the wrong instruction for that.
    let plugins = HostSyncPlugins(plugin_manager.inner());
    if host_core::sync_target::SyncPlugins::resolve(&plugins, account.adapter_kind.as_str())
        .is_none()
    {
        return Err(CommandError {
            code: "plugin_missing",
            message: format!(
                "no loaded plugin serves `{}`",
                account.adapter_kind.as_str()
            ),
        });
    }
    // A different backend invalidates every remote-missing sound verdict —
    // same reason as `configure_sync_adapter`.
    host_core::sound_assets::reset_missing_cache();
    let plain = host_core::sync_target::from_account(
        &account,
        &prefs,
        &crate::secrets::KeyringSecretStore,
        &UserPrefsHostKeyVerifier::new(shared.clone()),
        &plugins,
    )
    .map_err(unbuildable_err)?;

    probe_activate_and_persist(
        plain,
        &shared,
        &orchestrator,
        &scheduler,
        &onboarding,
        passphrase.as_deref(),
        || {
            // The one thing this command writes: which account this device
            // syncs through. The row itself is the user's, added elsewhere and
            // untouched here — moving off it must not disturb it, which is
            // exactly what separates this from the connect path.
            host_core::sync_target::select_account(&prefs, Some(&account.id)).map_err(internal)
        },
    )
    .await
}

/// Set the periodic sync interval (in minutes). Values below 1 are
/// clamped to 1 so a typo can't pin the scheduler into a hot loop.
/// Returns the value actually persisted so the Settings UI can echo
/// it back into its slider.
#[tauri::command]
pub async fn set_sync_interval(
    scheduler: State<'_, Arc<SyncScheduler>>,
    event_log: State<'_, Arc<crate::event_log::EventLogWriter>>,
    minutes: u32,
) -> CommandResult<u32> {
    let clamped = scheduler
        .set_interval_minutes(minutes)
        .map_err(|err| CommandError {
            code: "internal",
            message: err,
        })?;

    // `sync.intervalMinutes` is on the sync whitelist — it is meant to be one
    // setting across a user's devices, not a per-device one. But the scheduler
    // writes it straight through `UserPrefsRepo`, which persists the row and
    // appends nothing, so changing it here never reached the other devices. The
    // generic `set_user_pref` command does emit for whitelisted keys; this one
    // bypassed it to reach the scheduler's clamping and its wake-up.
    //
    // Emit the same event that path would. The value goes over as a JSON
    // number, which is what the generic path produces too — it parses the
    // stored string as JSON before falling back to a string — so both writers
    // put the same shape on the wire.
    event_log.append(sync_core::SyncEvent::SettingsUpdated(
        sync_core::SettingsPayload {
            key: sync_engine::PREF_SYNC_INTERVAL_MINUTES.to_string(),
            value: serde_json::Value::from(clamped),
        },
    ));

    Ok(clamped)
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
    /// The account row this device syncs through, when it syncs through one.
    ///
    /// `None` on a device still reading the legacy `sync.adapter.*`
    /// preferences. The settings panel needs the ID rather than the name: it
    /// renders a list of the accounts that could hold the dataset and has to
    /// mark the one that does — and two accounts may legitimately carry the
    /// same display name.
    pub account_id: Option<String>,
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
    // The account this device syncs through answers first, and answers alone:
    // once the pointer is set the preferences below are a record nothing
    // maintains, and rendering a card from them would show the target the user
    // moved off.
    match host_core::sync_target::summary(&prefs, &AccountsRepo::new(&shared)) {
        host_core::sync_target::SummaryOutcome::Chosen(kind, detail) => {
            return Ok(Some(SyncAdapterSummary {
                kind,
                detail,
                account_id: host_core::sync_target::selected_account_id(&prefs),
            }));
        }
        // The pointer names a row that is gone. Saying nothing is right: the
        // preferences below are still complete on a migrated device, so falling
        // through would announce the pre-migration target while the round is
        // running somewhere else.
        host_core::sync_target::SummaryOutcome::Missing => return Ok(None),
        host_core::sync_target::SummaryOutcome::NotChosen => {}
    }
    let kind = prefs
        .get(PREF_ADAPTER_KIND)
        .map_err(internal)?
        .filter(|stored| !is_unconfigured(Some(stored)));
    let Some(kind) = kind else {
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
    // The legacy reader answered, so there is no row to name.
    Ok(Some(SyncAdapterSummary {
        kind,
        detail,
        account_id: None,
    }))
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

/// The same question, asked with a kind and the form's values.
///
/// The twin of [`preview_sync_target`] for a caller rendering the shared
/// schema form rather than one hand-written per backend. Same answer, same
/// side effects — which is to say none: this reaches the target and reports
/// what is there, and a wizard that then does nothing has changed nothing.
///
/// It exists beside the typed one rather than replacing it because the two
/// frontends move one at a time; the typed half goes when the last per-kind
/// form does.
#[tauri::command]
pub async fn preview_sync_target_values(
    db: State<'_, DbHandle>,
    onboarding: State<'_, Arc<OnboardingService>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    kind: String,
    values: serde_json::Map<String, serde_json::Value>,
) -> CommandResult<SyncPreview> {
    let shared = db.shared();
    let adapter = host_core::sync_target::preview_adapter(
        &HostSyncPlugins(plugin_manager.inner()),
        &UserPrefsHostKeyVerifier::new(shared.clone()),
        &kind,
        &values,
    )
    .map_err(connect_err)?;
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
    persist_adapter_config(&shared, plugin_manager.inner(), &config)?;
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

/// "Datensatz übernehmen", asked with a kind and the shared form's values.
///
/// The twin of [`accept_remote_dataset`], and the same flow up to the last
/// step: reach the target, read `meta.json`, derive the key if the dataset is
/// encrypted, apply it, and only then commit. What differs is WHAT is
/// committed — the typed one writes the legacy preferences, this one writes an
/// account row and points this device at it, which is what every other path in
/// the app now does.
///
/// The commit runs after `accept_remote` has succeeded, deliberately: a join
/// that fails must leave a device exactly as it was, and a row written first
/// would survive it.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn accept_remote_dataset_values(
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
    onboarding: State<'_, Arc<OnboardingService>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    refresher: State<'_, Arc<CacheRefresher>>,
    registry: State<'_, Arc<AdapterRegistry>>,
    kind: String,
    values: serde_json::Map<String, serde_json::Value>,
    device_name: Option<String>,
    passphrase: Option<String>,
) -> CommandResult<OnboardingReport> {
    let shared = db.shared();
    let plugins = HostSyncPlugins(plugin_manager.inner());
    let plain = host_core::sync_target::preview_adapter(
        &plugins,
        &UserPrefsHostKeyVerifier::new(shared.clone()),
        &kind,
        &values,
    )
    .map_err(connect_err)?;
    plain.test_connection().await.map_err(sync_err)?;

    let meta = plain.fetch_meta().await.map_err(sync_err)?;
    let e2e_active = meta.as_ref().map(|m| m.e2e_enabled).unwrap_or(false);
    let key: Option<[u8; KEY_LEN]> = if e2e_active {
        let pp = passphrase
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or(CommandError {
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
        Some(resolve_data_key(pp, &params).map_err(sync_err)?)
    } else {
        None
    };
    let adapter = wrap_if_encrypted(Arc::clone(&plain), key);
    let prefs = UserPrefsRepo::new(&shared);

    // Set before applying, and reverted on failure. The snapshot's credential
    // restore is gated on this device's flag, so an E2E dataset applied while
    // it is still false drops every account's password on the floor and
    // re-prompts for all of them. Same reasoning as the typed twin's.
    if e2e_active {
        prefs.set(PREF_E2E_ENABLED, "true").map_err(internal)?;
    }

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

    orchestrator.configure(Arc::clone(&adapter));
    // The account row, its secrets and the pointer — the same write the
    // settings path makes, so a device onboarded here is indistinguishable
    // afterwards from one set up later.
    host_core::sync_target::connect(
        &prefs,
        &AccountsRepo::new(&shared),
        &crate::secrets::KeyringSecretStore,
        &plugins,
        &kind,
        &values,
    )
    .map_err(connect_err)?;
    if let Some(k) = key {
        store_e2e_key(&k)?;
    } else {
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
    persist_adapter_config(&shared, plugin_manager.inner(), &config)?;
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

/// "Neu beginnen", asked with a kind and the shared form's values.
///
/// The twin of [`adopt_local_dataset`], differing in the same one place as the
/// accept twin: the commit writes an account row and the pointer instead of
/// the legacy preferences.
///
/// Note what this does NOT do differently. It still mints a fresh v2 key pair
/// when a passphrase is given, still overwrites the remote `meta.json`, and
/// still forces the local task backfill afterwards — a device that turns sync
/// on after creating lists would otherwise never send them, and the one-shot
/// gate on the boot backfill has long since fired.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn adopt_local_dataset_values(
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
    onboarding: State<'_, Arc<OnboardingService>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    event_log: State<'_, Arc<crate::event_log::EventLogWriter>>,
    kind: String,
    values: serde_json::Map<String, serde_json::Value>,
    device_name: Option<String>,
    passphrase: Option<String>,
) -> CommandResult<OnboardingReport> {
    let shared = db.shared();
    let plugins = HostSyncPlugins(plugin_manager.inner());
    let plain = host_core::sync_target::preview_adapter(
        &plugins,
        &UserPrefsHostKeyVerifier::new(shared.clone()),
        &kind,
        &values,
    )
    .map_err(connect_err)?;
    plain.test_connection().await.map_err(sync_err)?;

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
    host_core::sync_target::connect(
        &prefs,
        &AccountsRepo::new(&shared),
        &crate::secrets::KeyringSecretStore,
        &plugins,
        &kind,
        &values,
    )
    .map_err(connect_err)?;
    if let Some(k) = key {
        store_e2e_key(&k)?;
        prefs.set(PREF_E2E_ENABLED, "true").map_err(internal)?;
    } else {
        let _ = prefs.delete(PREF_E2E_ENABLED);
    }
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
    plugin_manager: State<'_, Arc<PluginManager>>,
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
        plugin_manager.inner(),
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
    plugin_manager: State<'_, Arc<PluginManager>>,
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
        plugin_manager.inner(),
        &crate::secrets::KeyringSecretStore,
    );

    Ok(())
}

/// Restore what this device syncs through, for `lib.rs::setup` on app start.
///
/// The migration, the choice of reader and the log line are
/// `host_core::sync_target`'s, shared with the mobile host and tested there;
/// this host contributes the four arguments only it can produce — its database,
/// its keyring, its host-key pin store, and [`HostSyncPlugins`].
///
/// The returned adapter is already wrapped for encryption where this device
/// encrypts, so the caller configures it exactly as it stands.
pub fn restore_sync_adapter(
    db: &SharedConn,
    plugin_manager: &PluginManager,
) -> Option<Arc<dyn SyncAdapter>> {
    host_core::sync_target::restore_sync_target(
        &UserPrefsRepo::new(db),
        &AccountsRepo::new(db),
        &crate::secrets::KeyringSecretStore,
        &UserPrefsHostKeyVerifier::new(db.clone()),
        &HostSyncPlugins(plugin_manager),
    )
}

/// Rebuild the sync adapter this device is configured to open, WITHOUT the
/// migration and without the start-up log line.
///
/// Everything this used to do — the per-kind preference reads, the keychain
/// lookups, assembling the plugin's init config, the SFTP host-key refusal, and
/// the encryption wrap — now lives in `host_core::sync_target`, shared with the
/// mobile host and tested there. What remains here is how THIS host opens a
/// plugin and where it keeps its host-key pins.
///
/// Which of the two records answers is `build_for_device`'s decision, not this
/// function's: the account row when this device points at one, and only then
/// the `sync.adapter.*` preferences it has not moved off yet.
///
/// Start-up does not come through here any more — it calls
/// [`restore_sync_adapter`]. What is left is the encryption downgrade, which
/// needs a second handle to the target the orchestrator already holds, at a
/// moment where the migration has long since run and a second "restored the
/// sync target" line would be a lie about what just happened.
///
/// The reason it returns `None` rather than the reason it failed is that its
/// caller has nowhere to put one; the reason is logged instead, which is more
/// than either host did before.
pub fn build_adapter_from_prefs(
    db: &SharedConn,
    plugin_manager: &PluginManager,
) -> Option<Arc<dyn SyncAdapter>> {
    let prefs = UserPrefsRepo::new(db);
    match host_core::sync_target::build_for_device(
        &prefs,
        &AccountsRepo::new(db),
        &crate::secrets::KeyringSecretStore,
        &UserPrefsHostKeyVerifier::new(db.clone()),
        &HostSyncPlugins(plugin_manager),
    ) {
        Ok(adapter) => Some(adapter),
        Err(host_core::sync_target::Unbuildable::NotConfigured) => None,
        Err(err) => {
            // Not silent any more. Every one of these used to be a bare `?`
            // that left sync switched off with nothing said about it.
            tracing::warn!(%err, "could not restore the configured sync target");
            None
        }
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
///
/// Asked through `stored_secret` rather than of the pseudo-account directly:
/// once this device syncs through an account row the token lives under that
/// row's id, and a connected user would otherwise be told to sign in again.
#[tauri::command]
pub async fn has_dropbox_refresh_token(db: State<'_, DbHandle>) -> CommandResult<bool> {
    let shared = db.shared();
    Ok(host_core::sync_target::stored_secret(
        &UserPrefsRepo::new(&shared),
        &AccountsRepo::new(&shared),
        &crate::secrets::KeyringSecretStore,
        "dropbox",
        SecretSlot::RefreshToken,
    )
    .is_some())
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
/// button in the SyncPanel. See [`has_dropbox_refresh_token`] for why it asks
/// `stored_secret` rather than the pseudo-account.
#[tauri::command]
pub async fn has_googledrive_refresh_token(db: State<'_, DbHandle>) -> CommandResult<bool> {
    let shared = db.shared();
    Ok(host_core::sync_target::stored_secret(
        &UserPrefsRepo::new(&shared),
        &AccountsRepo::new(&shared),
        &crate::secrets::KeyringSecretStore,
        "googledrive",
        SecretSlot::RefreshToken,
    )
    .is_some())
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
    classify_host_key(
        &db.shared(),
        format!("{trimmed_host}:{port}"),
        probe.fingerprint,
    )
}

/// Compare a freshly-observed fingerprint against this device's pin store.
///
/// Split out so the account-based probe below reaches the SAME three-way
/// verdict. A second copy of this comparison is the one place a bug would be
/// invisible: it would classify a CHANGED key as first use, and the user would
/// be shown the benign prompt for what is the alarm case.
fn classify_host_key(
    db: &SharedConn,
    host_port: String,
    fingerprint: String,
) -> CommandResult<HostKeyPreview> {
    let verifier = UserPrefsHostKeyVerifier::new(db.clone());
    // `try_peek`, not `peek`: this is the one place the pin is COMPARED. A
    // read failure folded into `None` would classify a host key that CHANGED
    // as first use — the user sees the benign TOFU prompt instead of the
    // §19.5 alarm, confirms, and `trust_sftp_host_key` writes the presented
    // fingerprint over a pin we could not read. Refuse the preview instead;
    // nothing is pinned and nothing is connected until the user retries.
    let stored = verifier.try_peek(&host_port).map_err(|err| CommandError {
        code: "internal",
        message: format!("read the pinned host key for {host_port}: {err}"),
    })?;
    let status = match stored {
        None => HostKeyPreviewStatus::New,
        Some(s) if s == fingerprint => HostKeyPreviewStatus::Unchanged,
        Some(s) => HostKeyPreviewStatus::Changed { stored: s },
    };
    Ok(HostKeyPreview {
        host_port,
        fingerprint,
        status,
    })
}

/// The §19.5 trust gesture for an account the user is about to sync through.
///
/// [`select_sync_account`] refuses with `host_key_not_trusted` when the
/// account's protocol pins host keys and this device has never confirmed the
/// server's fingerprint — which is the normal state of an SFTP account added
/// under Settings → Accounts, because that path never probes. Without this
/// command the refusal is a dead end: the only other way to pin a fingerprint
/// is the connect form, and the settings panel no longer shows one.
///
/// Nothing here names a protocol. WHICH fields hold the host and the port, and
/// whether the account has a host key at all, come from the schema's
/// `host_key_pin` declaration — the same one
/// [`host_core::sync_target::from_account`] refuses on — so an adapter that
/// starts pinning host keys tomorrow is served by writing it into its own
/// manifest.
///
/// Reads the row's config with this device's half layered on top, in that
/// order, because a device-local field is this device's answer and the row's is
/// every other device's.
#[tauri::command]
pub async fn preview_sync_account_host_key(
    db: State<'_, DbHandle>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    account_id: String,
) -> CommandResult<HostKeyPreview> {
    let account_id = account_id.trim().to_string();
    let shared = db.shared();
    let account = AccountsRepo::new(&shared)
        .get(&account_id)?
        .ok_or(CommandError {
            code: "not_found",
            message: format!("no account {account_id}"),
        })?;
    let plugins = HostSyncPlugins(plugin_manager.inner());
    let (plugin_id, schema) =
        host_core::sync_target::SyncPlugins::resolve(&plugins, account.adapter_kind.as_str())
            .ok_or_else(|| CommandError {
                code: "plugin_missing",
                message: format!(
                    "no loaded plugin serves `{}`",
                    account.adapter_kind.as_str()
                ),
            })?;
    let pin = schema.host_key_pin.clone().ok_or(CommandError {
        code: "invalid_input",
        message: "this account's protocol does not pin host keys".into(),
    })?;
    // WHICH server, resolved the one way both hosts resolve it — row plus this
    // device's half, port as text either way. See `account_host_key_pin`.
    let info = host_core::sync_target::account_host_key_pin(
        &account,
        &UserPrefsRepo::new(&shared),
        &UserPrefsHostKeyVerifier::new(shared.clone()),
        &plugins,
    )
    .ok_or(CommandError {
        code: "invalid_input",
        message: "this account's protocol does not pin host keys".into(),
    })?;
    let parsed_port: u16 = info.port.parse().unwrap_or_default();
    if info.host.is_empty() || parsed_port == 0 {
        return Err(CommandError {
            code: "invalid_input",
            message: "this account does not say which server to probe".into(),
        });
    }
    let host = info.host.clone();
    let port = info.port.clone();
    // Under the plugin's own field names, and built by hand rather than through
    // `json!` because the keys are the declaration's, not literals.
    let mut args = serde_json::Map::new();
    args.insert(
        pin.host_field.clone(),
        serde_json::Value::String(host.clone()),
    );
    args.insert(pin.port_field.clone(), serde_json::Value::from(parsed_port));
    let probe: HostKeyProbeResult = run_plugin_probe_host_key(
        plugin_manager.inner(),
        &plugin_id,
        serde_json::Value::Object(args),
    )
    .await?;
    // The same key `merge_pin` looks the pin up under, or a fingerprint the
    // user confirms here stays invisible to the build that needs it.
    classify_host_key(&shared, format!("{host}:{port}"), probe.fingerprint)
}

/// What this device has confirmed about an account's server — no network.
///
/// The counterpart to [`preview_sync_account_host_key`], which dials the server
/// to see what it is presenting NOW. This one reports the decision the user
/// already made, so Settings can show it and offer to revoke it while the
/// server is unreachable — which is exactly when revoking matters.
///
/// `null` for an account whose adapter declares no `host_key_pin`; a
/// `host_port` of `null` inside a value means the row does not say which
/// server, which is a different thing and has a different repair.
#[tauri::command]
pub async fn sync_account_host_key_pin(
    db: State<'_, DbHandle>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    account_id: String,
) -> CommandResult<Option<host_core::sync_target::HostKeyPinInfo>> {
    let account_id = account_id.trim().to_string();
    let shared = db.shared();
    let Some(account) = AccountsRepo::new(&shared).get(&account_id)? else {
        return Ok(None);
    };
    Ok(host_core::sync_target::account_host_key_pin(
        &account,
        &UserPrefsRepo::new(&shared),
        &UserPrefsHostKeyVerifier::new(shared.clone()),
        &HostSyncPlugins(plugin_manager.inner()),
    ))
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
    is_unconfigured, PLUGIN_ID_DROPBOX, PLUGIN_ID_FTP, PLUGIN_ID_GOOGLEDRIVE, PLUGIN_ID_LOCAL,
    PLUGIN_ID_SFTP, PLUGIN_ID_WEBDAV, PREF_ADAPTER_KIND, PREF_DROPBOX_PATH, PREF_FTP_HOST,
    PREF_FTP_PATH, PREF_FTP_PORT, PREF_FTP_USER, PREF_GOOGLEDRIVE_FOLDER_NAME, PREF_LOCAL_PATH,
    PREF_SFTP_HOST, PREF_SFTP_PATH, PREF_SFTP_PORT, PREF_SFTP_USER, PREF_WEBDAV_URL,
    PREF_WEBDAV_USER, SECRET_ACCOUNT_DROPBOX as DROPBOX_SECRET_ACCOUNT,
    SECRET_ACCOUNT_E2E as E2E_SECRET_ACCOUNT,
    SECRET_ACCOUNT_GOOGLEDRIVE as GOOGLEDRIVE_SECRET_ACCOUNT,
};
// The four per-kind credential pseudo-accounts are gone from this file. Nothing
// here reads a credential by its legacy address any more: `stored_secret` knows
// where it lives, and only the two OAuth dances still WRITE to one — they run
// before there is an account row to write to.
