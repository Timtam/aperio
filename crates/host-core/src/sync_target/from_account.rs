//! Building the sync adapter from an account row.
//!
//! The other half of this module builds it from twenty preference keys and
//! seven keychain pseudo-accounts, with a six-arm match that knows what a
//! WebDAV URL is and what an SFTP port is. This one knows none of that. It
//! takes the account the user chose, asks the plugin's own schema which of its
//! values are secret and which stay on this device, merges them the same way
//! every other account is merged, and opens the instance.
//!
//! ## Why a pointer rather than a flag on the row
//!
//! Which target a device syncs through is that device's own business. Two
//! machines sharing one dataset may reach it differently — a laptop over the
//! internet, a desktop over a share on the same network — and a phone may not
//! sync at all while still holding every account. So the account rows travel
//! and the CHOICE does not: [`PREF_SELECTED_ACCOUNT`] is a device-local
//! preference holding an account id, which is exactly the shape the old
//! `sync.adapter.kind` had. Nothing about that idea changes; only what the
//! value points at.
//!
//! ## What is deliberately NOT here
//!
//! The end-to-end encryption key and the flag that says whether to use it stay
//! where they are, in the keychain and in this device's preferences. They are
//! properties of the DATASET and of this device's posture towards it, not of
//! the place it is stored — a user who moves from WebDAV to SFTP is not
//! changing their mind about encryption, and re-deriving the key from a
//! passphrase they were never asked for is not something a target switch may
//! do.
//!
//! Host-key pins likewise stay keyed by `host:port`. See
//! [`crate::registry::HostKeyPins`].

use std::sync::Arc;

use sync_core::SyncAdapter;
use sync_engine::{SecretSlot, SecretStore};

use super::build::Unbuildable;
use crate::accounts::Account;
use crate::registry::HostKeyPins;
use crate::user_prefs::{UserPrefsRepo, UserPrefsResult};

/// Which account this device syncs through, or absent for none.
///
/// Device-local by omission rather than by exception: the sync whitelist is an
/// allowlist, and the only sync key on it is the interval. The test at the
/// bottom of this file checks that rather than trusting it.
pub const PREF_SELECTED_ACCOUNT: &str = "sync.accountId";

/// What this module needs from the host's plugin manager.
///
/// Two calls rather than one, because resolving and opening happen at different
/// moments: the schema is needed to assemble the config, and opening is what
/// the config is for. Kept as a trait for the same reason [`super::PluginOpener`]
/// is one — the two hosts reach their manager differently, and this module
/// should not know either shape.
pub trait SyncPlugins {
    /// The plugin serving `adapter_kind`, as its id and its account schema.
    ///
    /// `None` when no loaded plugin declares the kind — the account came from
    /// another device that has a plugin this one does not.
    fn resolve(
        &self,
        adapter_kind: &str,
    ) -> Option<(String, plugin_core::account_schema::AccountSchema)>;

    /// Open an instance of `plugin_id` against `config_json`.
    fn open(&self, plugin_id: &str, config_json: String) -> Result<Arc<dyn SyncAdapter>, String>;
}

/// Read the chosen account id, with a read that FAILED kept apart from a device
/// that has not chosen.
///
/// The distinction only matters to a caller that WRITES on the strength of the
/// answer, which is why it is a second function rather than a change to the one
/// below. The migration is that caller: it writes the pointer when this device
/// has none, and a locked database read as "none" would move where an existing
/// device syncs — silently, and against a choice the user made.
pub(super) fn selected_account_id_result(
    prefs: &UserPrefsRepo<'_>,
) -> UserPrefsResult<Option<String>> {
    Ok(prefs
        .get(PREF_SELECTED_ACCOUNT)?
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty()))
}

/// Read the chosen account id.
///
/// A failed read is an unchosen device here, which is what every caller of this
/// one wants: they are about to build an adapter, and "no adapter this launch"
/// is the same outcome either way and is retried on the next one.
pub fn selected_account_id(prefs: &UserPrefsRepo<'_>) -> Option<String> {
    selected_account_id_result(prefs).ok().flatten()
}

/// Choose an account to sync through, or `None` to sync through none.
///
/// Deleting rather than writing an empty string, so "this device does not sync"
/// and "this device has not decided yet" are the same state — which they are.
pub fn select_account(prefs: &UserPrefsRepo<'_>, account_id: Option<&str>) -> UserPrefsResult<()> {
    match account_id.map(str::trim).filter(|v| !v.is_empty()) {
        Some(id) => prefs.set(PREF_SELECTED_ACCOUNT, id),
        None => prefs.delete(PREF_SELECTED_ACCOUNT),
    }
}

/// Build the sync adapter for one account.
///
/// Everything provider-specific comes from the plugin's schema: which fields
/// are secrets and which slot each lives in, which stay on this device, and
/// whether the protocol pins host keys. This function knows none of it.
pub fn from_account(
    account: &Account,
    prefs: &UserPrefsRepo<'_>,
    secrets: &dyn SecretStore,
    pins: &dyn HostKeyPins,
    plugins: &dyn SyncPlugins,
) -> Result<Arc<dyn SyncAdapter>, Unbuildable> {
    let kind = account.adapter_kind.as_str();
    let (plugin_id, schema) = plugins
        .resolve(kind)
        .ok_or_else(|| Unbuildable::PluginRefused {
            message: format!("no loaded plugin serves `{kind}`"),
        })?;

    // This device's half. Secrets are excluded: they are device-local through
    // the keychain, and `init_config_with_local` reads them from there.
    let local_fields: Vec<String> = schema
        .fields
        .iter()
        .filter(|f| f.device_local && !f.is_secret())
        .map(|f| f.key.clone())
        .collect();
    let local = crate::account_local::load(prefs, &account.id, &local_fields);

    let mut config = crate::account_setup::init_config_with_local(
        &schema,
        &account.config_json,
        &local,
        |slot: SecretSlot| secrets.retrieve(&account.id, slot),
    )
    .map_err(|err| match err {
        // A credential the schema calls required and the keychain does not
        // hold. Distinguished from a broken config because the user can fix
        // one by typing and the other only by reconnecting.
        crate::account_setup::AccountSetupError::Secret(message) => {
            Unbuildable::PluginRefused { message }
        }
        other => Unbuildable::PluginRefused {
            message: other.to_string(),
        },
    })?;

    if let Some(pin) = schema.host_key_pin.as_ref() {
        config = merge_pin(&config, pin, pins)?;
    }

    plugins
        .open(&plugin_id, config)
        .map_err(|message| Unbuildable::PluginRefused { message })
}

/// The confirmed fingerprint, or a refusal.
///
/// Mirrors [`crate::registry::AdapterRegistry::apply_host_key_pin`] — same
/// rule, same reason, on the path that opens a sync adapter rather than a
/// calendar one. With an empty pin the plugin does not fail: it accepts
/// whatever key the network presents and remembers it, silently.
/// [`merge_pin`] for the path that has no account yet.
///
/// Same rule and the same refusal; only the error shape differs, because the
/// onboarding caller reports a `ConnectError` and has no business building an
/// `Unbuildable`. Returns the `host:port` that needs confirming.
pub(super) fn merge_pin_for_preview(
    config: &str,
    pin: &plugin_core::account_schema::AccountHostKeyPin,
    pins: &dyn HostKeyPins,
) -> Result<String, String> {
    merge_pin(config, pin, pins).map_err(|err| match err {
        Unbuildable::HostKeyNotTrusted { host_port } => host_port,
        // Anything else here means the config had no host or port to look one
        // up by, which for a form the user just filled in is a missing field.
        other => other.to_string(),
    })
}

fn merge_pin(
    config: &str,
    pin: &plugin_core::account_schema::AccountHostKeyPin,
    pins: &dyn HostKeyPins,
) -> Result<String, Unbuildable> {
    let mut parsed: serde_json::Value =
        serde_json::from_str(config).map_err(|e| Unbuildable::PluginRefused {
            message: format!("malformed account config: {e}"),
        })?;
    let obj = parsed
        .as_object_mut()
        .ok_or_else(|| Unbuildable::PluginRefused {
            message: "account config is not a JSON object".into(),
        })?;
    // A port arrives as a JSON number from a `number` field and as a string
    // from anything older. Both have to produce the same lookup key, or a pin
    // the user already confirmed becomes invisible.
    let text_of = |obj: &serde_json::Map<String, serde_json::Value>, key: &str| match obj.get(key) {
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    let host = text_of(obj, &pin.host_field);
    let port = text_of(obj, &pin.port_field);
    if host.is_empty() || port.is_empty() {
        return Err(Unbuildable::Incomplete { field: "host" });
    }
    let host_port = format!("{host}:{port}");
    let fingerprint = pins.peek(&host_port).unwrap_or_default();
    if fingerprint.trim().is_empty() {
        return Err(Unbuildable::HostKeyNotTrusted { host_port });
    }
    obj.insert(
        pin.field.clone(),
        serde_json::Value::String(fingerprint.trim().to_string()),
    );
    Ok(parsed.to_string())
}

/// Build the adapter this device is configured to sync through, encryption and
/// all.
///
/// The E2E decision is read exactly where it was before — this device's own
/// preference and its own keychain — because it belongs to the dataset, not to
/// the target. A missing key with the flag set is refused rather than falling
/// back to plaintext: that combination means the keychain was wiped or the data
/// directory was carried to a fresh install, and the next round would publish
/// in the clear precisely what was asked to be encrypted.
pub fn build_selected(
    prefs: &UserPrefsRepo<'_>,
    accounts: &crate::accounts::AccountsRepo<'_>,
    secrets: &dyn SecretStore,
    pins: &dyn HostKeyPins,
    plugins: &dyn SyncPlugins,
) -> Result<Arc<dyn SyncAdapter>, Unbuildable> {
    let id = selected_account_id(prefs).ok_or(Unbuildable::NotConfigured)?;
    // A chosen account that no longer exists is REPORTED, not read as "this
    // device has not configured anything". The two used to be the same answer,
    // and that is what let a caller fall through to the legacy preferences: a
    // user who disconnected, whose account row was deleted and whose old
    // `sync.adapter.*` values were still on disk, came back up on the target
    // they had just moved off. The pointer is not stale here — it is a choice
    // whose subject was deleted, which is a fault worth naming.
    let account = accounts
        .get(&id)
        .ok()
        .flatten()
        .ok_or_else(|| Unbuildable::AccountMissing {
            account_id: id.clone(),
        })?;

    let plain = from_account(&account, prefs, secrets, pins, plugins)?;
    if !super::e2e_enabled(prefs) {
        return Ok(plain);
    }
    let key = super::build::e2e_key(secrets).ok_or(Unbuildable::MissingCredential {
        field: "encryption key",
    })?;
    Ok(Arc::new(sync_core::EncryptingAdapter::new(plain, key)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use plugin_core::account_schema::*;

    struct NoPins;
    impl HostKeyPins for NoPins {
        fn peek(&self, _host_port: &str) -> Option<String> {
            None
        }
    }
    struct Pinned(&'static str);
    impl HostKeyPins for Pinned {
        fn peek(&self, _host_port: &str) -> Option<String> {
            Some(self.0.to_string())
        }
    }

    /// Records the config it was opened with, so a test can assert on what the
    /// plugin would actually have received.
    #[derive(Default)]
    struct Opened {
        seen: std::sync::Mutex<Vec<(String, String)>>,
        schema: Option<AccountSchema>,
    }

    impl SyncPlugins for Opened {
        fn resolve(&self, adapter_kind: &str) -> Option<(String, AccountSchema)> {
            self.schema
                .clone()
                .map(|s| (format!("com.example.{adapter_kind}"), s))
        }
        fn open(
            &self,
            plugin_id: &str,
            config_json: String,
        ) -> Result<Arc<dyn SyncAdapter>, String> {
            self.seen
                .lock()
                .unwrap()
                .push((plugin_id.to_string(), config_json));
            // Nothing in these tests calls the adapter; they are about what
            // reaches `open`, and what refuses to get there.
            Err("not a real plugin".into())
        }
    }

    fn field(key: &str, kind: AccountFieldKind) -> AccountField {
        AccountField {
            key: key.to_string(),
            kind,
            label: key.to_string(),
            ..Default::default()
        }
    }

    fn sftp_schema() -> AccountSchema {
        let mut key_path = field("key_path", AccountFieldKind::File);
        key_path.device_local = true;
        let mut password = field("password", AccountFieldKind::Secret);
        password.secret_slot = Some(AccountSecretSlot::Password);
        AccountSchema {
            fields: vec![
                field("host", AccountFieldKind::Text),
                field("port", AccountFieldKind::Number),
                key_path,
                password,
            ],
            host_key_pin: Some(AccountHostKeyPin {
                field: "pinned_fingerprint".into(),
                host_field: "host".into(),
                port_field: "port".into(),
            }),
            ..Default::default()
        }
    }

    fn account(config: &str) -> Account {
        Account {
            id: "acc-1".into(),
            adapter_kind: crate::accounts::AdapterKind::new("sftp"),
            display_name: "Files".into(),
            config_json: config.to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    /// The refusal, on this path too. It is the whole reason the pin is
    /// declared rather than left to the plugin.
    #[test]
    fn an_unconfirmed_host_key_refuses_before_the_plugin_is_opened() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let plugins = Opened {
            schema: Some(sftp_schema()),
            ..Default::default()
        };
        let err = from_account(
            &account(r#"{"host":"files.example.com","port":22}"#),
            &prefs,
            &sync_engine::test_support::FakeSecrets::default(),
            &NoPins,
            &plugins,
        )
        .err()
        .expect("must refuse");
        match err {
            Unbuildable::HostKeyNotTrusted { host_port } => {
                assert_eq!(host_port, "files.example.com:22")
            }
            other => panic!("wrong refusal: {other}"),
        }
        assert!(
            plugins.seen.lock().unwrap().is_empty(),
            "the plugin must not be opened at all",
        );
    }

    /// The device-local half reaches the plugin, and does not come from the
    /// row — the row is what travels between devices.
    #[test]
    fn this_devices_half_is_merged_in() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let mut local = serde_json::Map::new();
        local.insert(
            "key_path".into(),
            serde_json::Value::String("/home/anna/.ssh/id_ed25519".into()),
        );
        crate::account_local::store(&prefs, "acc-1", &["key_path".to_string()], &local).unwrap();

        let plugins = Opened {
            schema: Some(sftp_schema()),
            ..Default::default()
        };
        let _ = from_account(
            &account(r#"{"host":"files.example.com","port":22}"#),
            &prefs,
            &sync_engine::test_support::FakeSecrets::default(),
            &Pinned("SHA256:abc"),
            &plugins,
        );
        let seen = plugins.seen.lock().unwrap();
        let (plugin_id, config) = seen.first().expect("the plugin was opened");
        assert_eq!(plugin_id, "com.example.sftp");
        let parsed: serde_json::Value = serde_json::from_str(config).unwrap();
        assert_eq!(
            parsed["key_path"].as_str(),
            Some("/home/anna/.ssh/id_ed25519"),
        );
        assert_eq!(parsed["pinned_fingerprint"].as_str(), Some("SHA256:abc"));
        // Still a number, as the plugin's own struct requires.
        assert_eq!(parsed["port"], serde_json::json!(22));
    }

    /// An account whose plugin this device does not have refuses with a message
    /// that names the kind, rather than looking like a broken config.
    #[test]
    fn an_account_whose_plugin_is_missing_says_so() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let err = from_account(
            &account("{}"),
            &prefs,
            &sync_engine::test_support::FakeSecrets::default(),
            &NoPins,
            &Opened::default(),
        )
        .err()
        .expect("must refuse");
        assert!(err.to_string().contains("sftp"), "{err}");
    }

    #[test]
    fn the_pointer_round_trips_and_clears() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        assert_eq!(selected_account_id(&prefs), None);
        select_account(&prefs, Some("acc-1")).unwrap();
        assert_eq!(selected_account_id(&prefs).as_deref(), Some("acc-1"));
        select_account(&prefs, None).unwrap();
        assert_eq!(selected_account_id(&prefs), None);
        // Blank is the same as none, not an account whose id is empty.
        select_account(&prefs, Some("   ")).unwrap();
        assert_eq!(selected_account_id(&prefs), None);
    }

    /// The two postures, on the same unreadable store.
    ///
    /// A caller that is about to BUILD may treat a failed read as "nothing
    /// chosen" — it writes nothing, and the next launch reads again. A caller
    /// that is about to WRITE may not: the migration decides whether to point
    /// this device somewhere on the strength of this answer, and "nobody chose"
    /// from a database that would not say moves a working sync onto another
    /// target with nothing to show for it.
    #[test]
    fn an_unreadable_pointer_is_an_error_for_a_caller_that_writes() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        select_account(&prefs, Some("acc-1")).unwrap();
        assert_eq!(
            selected_account_id_result(&prefs).unwrap().as_deref(),
            Some("acc-1"),
        );

        shared
            .lock()
            .unwrap()
            .execute("DROP TABLE user_prefs", [])
            .unwrap();
        assert!(
            selected_account_id_result(&prefs).is_err(),
            "a store that will not answer must not answer `nobody chose`",
        );
        assert_eq!(selected_account_id(&prefs), None);
    }

    /// A pointer that crossed devices would make a laptop start syncing through
    /// a folder on a desktop's disk.
    #[test]
    fn the_choice_never_crosses_devices() {
        assert!(!sync_engine::whitelist::is_synced_key(
            PREF_SELECTED_ACCOUNT
        ));
    }

    /// A pointer at a deleted account is its own answer.
    ///
    /// It must not read as "nothing configured": that is the answer a caller
    /// falls back to the legacy preferences on, and this device chose an
    /// account precisely to stop consulting them.
    #[test]
    fn a_pointer_to_a_deleted_account_reports_rather_than_reading_as_unconfigured() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = crate::accounts::AccountsRepo::new(&shared);
        select_account(&prefs, Some("gone")).unwrap();
        let err = build_selected(
            &prefs,
            &accounts,
            &sync_engine::test_support::FakeSecrets::default(),
            &NoPins,
            &Opened::default(),
        )
        .err()
        .expect("must not build");
        assert!(
            matches!(err, Unbuildable::AccountMissing { ref account_id } if account_id == "gone"),
            "{err}",
        );
    }
}
