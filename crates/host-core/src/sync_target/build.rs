//! Reconstructing the configured sync adapter at start-up, once.
//!
//! Both hosts carried this: read the kind, read that kind's preferences, pull
//! its secret out of the keychain, assemble an init config, open the plugin,
//! and wrap the result if this device encrypts. Two copies, six arms each.
//!
//! Merging them needed the shapes checked rather than assumed, and checking
//! turned up four things that would have been easy to get wrong and impossible
//! to notice:
//!
//! 1. **The init-config keys are not the preference keys.** The local adapter is
//!    opened with `remote_root` while its preference is `…local.path`; Dropbox
//!    wants `base_path` for the same idea. Five more preferences are camelCase
//!    against snake_case JSON.
//! 2. **`port` must be a JSON number.** It is `u16` in the SFTP and FTP plugins
//!    and serde will not coerce `"2222"` into one — while the persistence layer
//!    deliberately stores it as text. Legal in one direction, fatal in the
//!    other.
//! 3. **No plugin sets `deny_unknown_fields`.** A wrong key is not an error; it
//!    is ignored and the field silently takes its default.
//! 4. **Three values come from neither the preferences nor the form** — the SFTP
//!    pinned fingerprint, and the OAuth refresh tokens for Dropbox and Drive.
//!
//! Together those mean a mistake here does not fail loudly. It produces an
//! adapter pointed at the wrong place, or no adapter at all — and the callers
//! discard the error, so the symptom is sync quietly never running.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use sync_core::SyncAdapter;
use sync_engine::{SecretSlot, SecretStore};

use super::*;
use crate::sftp_host_keys::UserPrefsHostKeyVerifier;
use crate::user_prefs::{UserPrefsRepo, UserPrefsResult};

/// Why no adapter could be built.
///
/// Callers on both hosts historically threw this away — the restore path is an
/// `Option` — so it exists to be logged, and to let a future caller say
/// something better than nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unbuildable {
    /// No target chosen on this device.
    NotConfigured,
    /// This device points at an account row that is not there any more.
    ///
    /// Deliberately NOT [`Self::NotConfigured`], which is what it used to
    /// collapse into. "Nobody chose" is a device that may still fall back to the
    /// legacy preferences; this is a device that chose and whose choice was
    /// DELETED, and resuming the target those preferences describe would start
    /// uploading again to the place the user moved off. It reports instead.
    AccountMissing { account_id: String },
    /// A value the adapter cannot work without is missing or empty.
    Incomplete { field: &'static str },
    /// The credential is not in this device's keychain.
    MissingCredential { field: &'static str },
    /// SFTP with no trusted host key. Refused deliberately: see [`build`].
    HostKeyNotTrusted { host_port: String },
    /// A stored value is not one this build understands.
    Invalid { field: &'static str, value: String },
    /// The plugin is missing, disabled, or rejected the config.
    PluginRefused { message: String },
}

impl std::fmt::Display for Unbuildable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => write!(f, "no sync target configured"),
            Self::AccountMissing { account_id } => write!(
                f,
                "the account this device syncs through ({account_id}) no longer exists",
            ),
            Self::Incomplete { field } => write!(f, "sync target is missing {field}"),
            Self::MissingCredential { field } => {
                write!(f, "no {field} stored for the sync target")
            }
            Self::HostKeyNotTrusted { host_port } => write!(
                f,
                "the host key for {host_port} has not been trusted on this device",
            ),
            Self::Invalid { field, value } => {
                write!(
                    f,
                    "sync target field {field} holds an unusable value: {value}"
                )
            }
            Self::PluginRefused { message } => write!(f, "sync plugin refused: {message}"),
        }
    }
}

/// What a host must supply to open a plugin instance. Keeps this module free of
/// `plugin_core`'s manager type, which the two hosts reach differently.
pub trait PluginOpener {
    fn open(&self, plugin_id: &str, config_json: String) -> Result<Arc<dyn SyncAdapter>, String>;
}

/// A preference as a value, or `None` for absent-or-blank — with a read that
/// FAILED kept apart from a value that is not there.
///
/// `pub(super)` because the migration reads the same preferences with the same
/// rule — a target whose host is three spaces is a target with no host, and two
/// modules disagreeing about that would migrate something the builder refuses.
/// The rule lives here once; what a caller does about an unanswered read is the
/// caller's own posture, and the two callers have opposite ones.
pub(super) fn text_result(prefs: &UserPrefsRepo<'_>, key: &str) -> UserPrefsResult<Option<String>> {
    Ok(prefs
        .get(key)?
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty()))
}

/// [`text_result`], with a failed read read as an absent value.
///
/// Sound HERE and nowhere else in this module's neighbourhood: [`build`]
/// persists nothing. A launch whose database will not answer builds no adapter,
/// or builds one missing an optional value, and the next launch does it again
/// from the same preferences — nothing has been decided and nothing recorded.
/// The migration cannot afford the same shrug, because it WRITES what it read
/// and then marks itself done; it uses [`text_result`].
pub(super) fn text(prefs: &UserPrefsRepo<'_>, key: &str) -> Option<String> {
    text_result(prefs, key).ok().flatten()
}

fn require(
    prefs: &UserPrefsRepo<'_>,
    key: &str,
    field: &'static str,
) -> Result<String, Unbuildable> {
    text(prefs, key).ok_or(Unbuildable::Incomplete { field })
}

fn credential(
    secrets: &dyn SecretStore,
    account: &str,
    slot: SecretSlot,
    field: &'static str,
) -> Result<String, Unbuildable> {
    secrets
        .retrieve(account, slot)
        .map_err(|_| Unbuildable::MissingCredential { field })
}

/// Ports are stored as text and must be handed over as numbers.
fn port_of(prefs: &UserPrefsRepo<'_>, key: &str, default: u16) -> u16 {
    text(prefs, key)
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(default)
}

/// Rebuild the configured adapter from this device's preferences.
///
/// Returns the plain adapter and whether this device encrypts; wrapping is the
/// caller's last step, because the wrapper type lives above the plugin boundary.
pub fn build(
    prefs: &UserPrefsRepo<'_>,
    secrets: &dyn SecretStore,
    opener: &dyn PluginOpener,
) -> Result<Arc<dyn SyncAdapter>, Unbuildable> {
    let kind = prefs
        .get(PREF_ADAPTER_KIND)
        .ok()
        .flatten()
        .filter(|stored| !is_unconfigured(Some(stored)))
        .ok_or(Unbuildable::NotConfigured)?;

    let plugin_id = plugin_id_for_kind(&kind).ok_or_else(|| Unbuildable::Invalid {
        field: "kind",
        value: kind.clone(),
    })?;

    let config: Map<String, Value> = match kind.as_str() {
        "local" => {
            // `remote_root`, not `path` — the one place the plugin's name for
            // this differs from both the form's and the preference's.
            let path = require(prefs, PREF_LOCAL_PATH, "path")?;
            json_map(json!({ "remote_root": path }))
        }
        "webdav" => {
            let url = require(prefs, PREF_WEBDAV_URL, "url")?;
            // Empty user means anonymous, which the adapter supports; absent is
            // therefore not an error here.
            let user = text(prefs, PREF_WEBDAV_USER).unwrap_or_default();
            let password = secrets
                .retrieve(SECRET_ACCOUNT_WEBDAV, SecretSlot::Password)
                .unwrap_or_default();
            json_map(json!({ "url": url, "user": user, "password": password }))
        }
        "sftp" => {
            let host = require(prefs, PREF_SFTP_HOST, "host")?;
            let port = port_of(prefs, PREF_SFTP_PORT, 22);
            let user = require(prefs, PREF_SFTP_USER, "user")?;
            let path = require(prefs, PREF_SFTP_PATH, "path")?;

            // Unknown methods resolve as password auth, and the value sent on
            // must be one the plugin accepts — it rejects anything else, and
            // echoing the unknown string is how this fallback used to fail.
            let stored = text(prefs, PREF_SFTP_AUTH_METHOD).unwrap_or_default();
            let use_key = stored == "key";

            let (password, key_path, key_passphrase) = if use_key {
                let kp = require(prefs, PREF_SFTP_KEY_PATH, "key_path")?;
                let pass = secrets
                    .retrieve(SECRET_ACCOUNT_SFTP_KEY, SecretSlot::Password)
                    .unwrap_or_default();
                (String::new(), kp, pass)
            } else {
                let pw = credential(
                    secrets,
                    SECRET_ACCOUNT_SFTP,
                    SecretSlot::Password,
                    "password",
                )?;
                (pw, String::new(), String::new())
            };

            // §19.5, and the reason this cannot become a warning: with an empty
            // pin the plugin does not fail — it builds a verifier that accepts
            // whatever key the network presents, and remembers it. There is no
            // error anywhere, and the first connection after that is
            // indistinguishable from a machine-in-the-middle.
            let host_port = format!("{host}:{port}");
            let pinned = UserPrefsHostKeyVerifier::new(prefs.db.clone())
                .peek(&host_port)
                .unwrap_or_default();
            if pinned.trim().is_empty() {
                return Err(Unbuildable::HostKeyNotTrusted { host_port });
            }

            json_map(json!({
                "host": host,
                "port": port,
                "user": user,
                "path": path,
                "auth_method": if use_key { "key" } else { "password" },
                "password": password,
                "key_path": key_path,
                "key_passphrase": key_passphrase,
                "pinned_fingerprint": pinned,
            }))
        }
        "ftp" => {
            let host = require(prefs, PREF_FTP_HOST, "host")?;
            let port = port_of(prefs, PREF_FTP_PORT, 21);
            let user = require(prefs, PREF_FTP_USER, "user")?;
            let path = text(prefs, PREF_FTP_PATH).unwrap_or_default();
            let mode = text(prefs, PREF_FTP_MODE).unwrap_or_else(|| "explicit".to_string());
            // Checked here as well as on connect: the plugin does not reject an
            // unknown mode, it falls through to explicit, so a stored value this
            // build does not know would quietly choose the transport.
            if !matches!(mode.as_str(), "implicit" | "explicit" | "plain") {
                return Err(Unbuildable::Invalid {
                    field: "mode",
                    value: mode,
                });
            }
            let password = credential(
                secrets,
                SECRET_ACCOUNT_FTP,
                SecretSlot::Password,
                "password",
            )?;
            json_map(json!({
                "host": host,
                "port": port,
                "user": user,
                "password": password,
                "path": path,
                "mode": mode,
            }))
        }
        "dropbox" => {
            let client_id = require(prefs, PREF_DROPBOX_CLIENT_ID, "client_id")?;
            let client_secret = text(prefs, PREF_DROPBOX_CLIENT_SECRET).unwrap_or_default();
            let base_path = text(prefs, PREF_DROPBOX_PATH).unwrap_or_default();
            let refresh_token = credential(
                secrets,
                SECRET_ACCOUNT_DROPBOX,
                SecretSlot::RefreshToken,
                "refresh_token",
            )?;
            // `base_path`, not `path`.
            json_map(json!({
                "client_id": client_id,
                "client_secret": client_secret,
                "base_path": base_path,
                "refresh_token": refresh_token,
            }))
        }
        "googledrive" => {
            let client_id = require(prefs, PREF_GOOGLEDRIVE_CLIENT_ID, "client_id")?;
            let client_secret = require(prefs, PREF_GOOGLEDRIVE_CLIENT_SECRET, "client_secret")?;
            let folder_name = text(prefs, PREF_GOOGLEDRIVE_FOLDER_NAME).unwrap_or_default();
            let refresh_token = credential(
                secrets,
                SECRET_ACCOUNT_GOOGLEDRIVE,
                SecretSlot::RefreshToken,
                "refresh_token",
            )?;
            json_map(json!({
                "client_id": client_id,
                "client_secret": client_secret,
                "folder_name": folder_name,
                "refresh_token": refresh_token,
            }))
        }
        other => {
            return Err(Unbuildable::Invalid {
                field: "kind",
                value: other.to_string(),
            })
        }
    };

    opener
        .open(plugin_id, Value::Object(config).to_string())
        .map_err(|message| Unbuildable::PluginRefused { message })
}

fn json_map(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        // Unreachable: every call site above passes an object literal.
        _ => Map::new(),
    }
}

/// Rebuild the adapter and, if this device encrypts, wrap it — the whole
/// restore, in one call.
///
/// The wrap used to sit in each host, one layer above its own copy of the
/// builder, which meant two places had to agree on when to encrypt and what to
/// do when the key is gone. They do not need to agree if there is only one.
///
/// A missing key with the flag set is refused rather than falling back to
/// plaintext: that combination means the keychain was wiped or the data
/// directory was carried to a fresh install, and uploading the next round
/// unencrypted would publish, in the clear, exactly what the user asked to be
/// encrypted — into a dataset every other device will then refuse to read.
pub fn build_configured(
    prefs: &UserPrefsRepo<'_>,
    secrets: &dyn SecretStore,
    opener: &dyn PluginOpener,
) -> Result<Arc<dyn SyncAdapter>, Unbuildable> {
    let plain = build(prefs, secrets, opener)?;
    if !e2e_enabled(prefs) {
        return Ok(plain);
    }
    let key = e2e_key(secrets).ok_or(Unbuildable::MissingCredential {
        field: "encryption key",
    })?;
    Ok(Arc::new(sync_core::EncryptingAdapter::new(plain, key)))
}

/// Build the adapter THIS DEVICE is configured to open, from whichever of the
/// two records answers — the account it points at, or the preferences it has
/// not moved off yet.
///
/// ## The pointer settles it, and nothing else gets a vote
///
/// A device with `sync.accountId` set syncs through that row and the legacy
/// `sync.adapter.*` preferences are not read at all: not as a fallback, not for
/// one field the row happens not to carry. Two records that can disagree is
/// exactly what this layer exists to end, and a reader that quietly prefers
/// whichever one is populated is how they get to disagree in the first place —
/// the row says SFTP, an old preference still says WebDAV, and which target the
/// device uploads to depends on which reader ran.
///
/// A pointer at a row that is GONE reports rather than recovers, for the reason
/// spelled out on [`Unbuildable::AccountMissing`].
pub fn build_for_device(
    prefs: &UserPrefsRepo<'_>,
    accounts: &crate::accounts::AccountsRepo<'_>,
    secrets: &dyn SecretStore,
    pins: &dyn crate::registry::HostKeyPins,
    plugins: &dyn super::SyncPlugins,
) -> Result<Arc<dyn SyncAdapter>, Unbuildable> {
    if selected_account_id(prefs).is_some() {
        return super::build_selected(prefs, accounts, secrets, pins, plugins);
    }
    build_configured(prefs, secrets, &AsOpener(plugins))
}

/// The account path needs a plugin resolver, the legacy path needs an opener,
/// and a host should have to supply one thing. `SyncPlugins` is the larger of
/// the two and already answers `open`.
struct AsOpener<'a>(&'a dyn super::SyncPlugins);

impl PluginOpener for AsOpener<'_> {
    fn open(&self, plugin_id: &str, config_json: String) -> Result<Arc<dyn SyncAdapter>, String> {
        self.0.open(plugin_id, config_json)
    }
}

/// The device-local data key, base64 in the keychain.
pub(super) fn e2e_key(secrets: &dyn SecretStore) -> Option<[u8; sync_core::crypto::KEY_LEN]> {
    use base64::Engine as _;
    let raw = secrets
        .retrieve(SECRET_ACCOUNT_E2E, SecretSlot::SyncEncryptionKey)
        .ok()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .ok()?;
    let out: [u8; sync_core::crypto::KEY_LEN] = bytes.try_into().ok()?;
    Some(out)
}

/// Whether this device encrypts what it uploads.
pub fn e2e_enabled(prefs: &UserPrefsRepo<'_>) -> bool {
    prefs
        .get(sync_engine::whitelist::PREF_E2E_ENABLED)
        .ok()
        .flatten()
        .as_deref()
        == Some("true")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        opened: Mutex<Vec<(String, String)>>,
        refuse: bool,
    }

    impl PluginOpener for Recorder {
        fn open(
            &self,
            plugin_id: &str,
            config_json: String,
        ) -> Result<Arc<dyn SyncAdapter>, String> {
            self.opened
                .lock()
                .unwrap()
                .push((plugin_id.to_string(), config_json));
            Err(if self.refuse {
                "refused".to_string()
            } else {
                // No real adapter is needed: every assertion here is about the
                // config handed over, which is the part that was duplicated.
                "captured".to_string()
            })
        }
    }

    fn opened_config(rec: &Recorder) -> Map<String, Value> {
        let (_, json) = rec.opened.lock().unwrap().last().cloned().expect("opened");
        serde_json::from_str(&json).expect("config is JSON")
    }

    struct Fixture {
        db: DbHandle,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                db: DbHandle::open_in_memory().unwrap(),
            }
        }
    }

    /// The trap the audit found: `port` is `u16` in the plugin and serde will
    /// not coerce a string, while persistence stores it as text.
    #[test]
    fn the_port_is_handed_over_as_a_number() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let secrets = crate::sync_target::persist::FakeSecrets::default();

        prefs.set(PREF_ADAPTER_KIND, "ftp").unwrap();
        prefs.set(PREF_FTP_HOST, "ftp.example.test").unwrap();
        prefs.set(PREF_FTP_PORT, "2121").unwrap();
        prefs.set(PREF_FTP_USER, "anna").unwrap();
        prefs.set(PREF_FTP_MODE, "explicit").unwrap();
        secrets
            .store(SECRET_ACCOUNT_FTP, SecretSlot::Password, "pw")
            .unwrap();

        let rec = Recorder::default();
        let _ = build(&prefs, &secrets, &rec);
        let cfg = opened_config(&rec);
        assert_eq!(cfg.get("port"), Some(&Value::from(2121u16)));
        assert!(
            cfg.get("port").unwrap().is_number(),
            "port must not be a string"
        );
    }

    /// The names that differ between the preference, the form and the plugin.
    #[test]
    fn the_plugins_own_names_are_used() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let secrets = crate::sync_target::persist::FakeSecrets::default();

        prefs.set(PREF_ADAPTER_KIND, "local").unwrap();
        prefs.set(PREF_LOCAL_PATH, "/srv/aperio").unwrap();
        let rec = Recorder::default();
        let _ = build(&prefs, &secrets, &rec);
        let cfg = opened_config(&rec);
        assert_eq!(
            cfg.get("remote_root").and_then(|v| v.as_str()),
            Some("/srv/aperio")
        );
        assert!(cfg.get("path").is_none(), "the plugin does not read `path`");

        prefs.set(PREF_ADAPTER_KIND, "dropbox").unwrap();
        prefs.set(PREF_DROPBOX_CLIENT_ID, "abc").unwrap();
        prefs.set(PREF_DROPBOX_PATH, "/Apps/Aperio").unwrap();
        secrets
            .store(SECRET_ACCOUNT_DROPBOX, SecretSlot::RefreshToken, "tok")
            .unwrap();
        let rec = Recorder::default();
        let _ = build(&prefs, &secrets, &rec);
        let cfg = opened_config(&rec);
        assert_eq!(
            cfg.get("base_path").and_then(|v| v.as_str()),
            Some("/Apps/Aperio")
        );
        assert!(cfg.get("path").is_none(), "the plugin does not read `path`");
    }

    /// §19.5. An untrusted host key must stop the build, because the plugin
    /// would not — it would accept whatever the network presented.
    #[test]
    fn sftp_without_a_trusted_host_key_is_refused_before_the_plugin_is_opened() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let secrets = crate::sync_target::persist::FakeSecrets::default();

        prefs.set(PREF_ADAPTER_KIND, "sftp").unwrap();
        prefs.set(PREF_SFTP_HOST, "backup.example.test").unwrap();
        prefs.set(PREF_SFTP_PORT, "22").unwrap();
        prefs.set(PREF_SFTP_USER, "anna").unwrap();
        prefs.set(PREF_SFTP_PATH, "/srv/aperio").unwrap();
        prefs.set(PREF_SFTP_AUTH_METHOD, "password").unwrap();
        secrets
            .store(SECRET_ACCOUNT_SFTP, SecretSlot::Password, "pw")
            .unwrap();

        let rec = Recorder::default();
        let err = build(&prefs, &secrets, &rec).err().expect("refused");
        assert_eq!(
            err,
            Unbuildable::HostKeyNotTrusted {
                host_port: "backup.example.test:22".to_string()
            },
        );
        assert!(
            rec.opened.lock().unwrap().is_empty(),
            "the plugin was opened despite an untrusted host key",
        );
    }

    /// An unknown method resolves as password auth AND is sent as one.
    #[test]
    fn an_unknown_sftp_auth_method_is_normalised_not_echoed() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let secrets = crate::sync_target::persist::FakeSecrets::default();

        prefs.set(PREF_ADAPTER_KIND, "sftp").unwrap();
        prefs.set(PREF_SFTP_HOST, "h").unwrap();
        prefs.set(PREF_SFTP_PORT, "22").unwrap();
        prefs.set(PREF_SFTP_USER, "anna").unwrap();
        prefs.set(PREF_SFTP_PATH, "/p").unwrap();
        prefs
            .set(PREF_SFTP_AUTH_METHOD, "gssapi-from-the-future")
            .unwrap();
        prefs
            .set("sync.adapter.sftp.knownHosts.h:22", "SHA256:abc")
            .unwrap();
        secrets
            .store(SECRET_ACCOUNT_SFTP, SecretSlot::Password, "pw")
            .unwrap();

        let rec = Recorder::default();
        let _ = build(&prefs, &secrets, &rec);
        let cfg = opened_config(&rec);
        assert_eq!(
            cfg.get("auth_method").and_then(|v| v.as_str()),
            Some("password")
        );
    }

    #[test]
    fn an_unknown_ftps_mode_is_refused_rather_than_guessed() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let secrets = crate::sync_target::persist::FakeSecrets::default();

        prefs.set(PREF_ADAPTER_KIND, "ftp").unwrap();
        prefs.set(PREF_FTP_HOST, "h").unwrap();
        prefs.set(PREF_FTP_USER, "anna").unwrap();
        prefs.set(PREF_FTP_MODE, "implicid").unwrap();
        secrets
            .store(SECRET_ACCOUNT_FTP, SecretSlot::Password, "pw")
            .unwrap();

        let rec = Recorder::default();
        let err = build(&prefs, &secrets, &rec).err().expect("refused");
        assert_eq!(
            err,
            Unbuildable::Invalid {
                field: "mode",
                value: "implicid".to_string()
            },
        );
        assert!(rec.opened.lock().unwrap().is_empty());
    }

    #[test]
    fn anonymous_webdav_builds() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let secrets = crate::sync_target::persist::FakeSecrets::default();

        prefs.set(PREF_ADAPTER_KIND, "webdav").unwrap();
        prefs
            .set(PREF_WEBDAV_URL, "https://example.test/dav/")
            .unwrap();

        let rec = Recorder::default();
        let _ = build(&prefs, &secrets, &rec);
        let cfg = opened_config(&rec);
        assert_eq!(cfg.get("user").and_then(|v| v.as_str()), Some(""));
        assert_eq!(cfg.get("password").and_then(|v| v.as_str()), Some(""));
    }

    #[test]
    fn nothing_configured_is_not_an_error_worth_reporting() {
        let f = Fixture::new();
        let shared = f.db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let secrets = crate::sync_target::persist::FakeSecrets::default();
        let rec = Recorder::default();
        assert_eq!(
            build(&prefs, &secrets, &rec).err(),
            Some(Unbuildable::NotConfigured),
        );
    }
}
