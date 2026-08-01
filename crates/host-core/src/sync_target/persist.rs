//! Writing a sync target down, and reading it back, in one implementation.
//!
//! Both hosts had their own copy of this: six `match` arms each, deciding for
//! every adapter kind which preference keys to write and which values were
//! secrets. The arms had already drifted apart in ways nobody noticed until
//! they were read side by side — one host stored credentials for the
//! authentication method the user had not chosen, the other wrote a sentinel
//! most readers did not recognise.
//!
//! Here the per-kind knowledge is a TABLE rather than code. Each field says
//! where its value lives, and `persist` is a loop over the table. That matters
//! beyond tidiness: the table is what Stage 4a deletes outright, once the
//! adapters declare their own account schemas and the schema answers the same
//! question. Deleting a table is a smaller act than unpicking six branches from
//! two files.
//!
//! ## Testability was the other reason
//!
//! The desktop's version reached the platform keychain through a module-level
//! function, so nothing about it could be tested without a real keychain on the
//! machine running the tests — which is why 2700 lines of sync commands carried
//! no test at all. Here the secret store arrives as a parameter, the same way
//! the mobile host already passed one, so a round trip can be exercised against
//! fakes.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use sync_engine::{SecretSlot, SecretStore};

use super::*;
use crate::user_prefs::UserPrefsRepo;

/// Where one field of a target's configuration is kept.
#[derive(Debug, Clone, Copy)]
pub struct FieldSpec {
    /// The key this field arrives under, from the connect form.
    pub key: &'static str,
    /// The preference it persists to. `None` for secrets, which never go into
    /// preferences — those are readable by anything that can read the database,
    /// and the database travels.
    pub pref: Option<&'static str>,
    /// The keychain slot, for a secret: `(pseudo-account id, slot)`.
    pub secret: Option<(&'static str, SecretSlot)>,
    /// Only written when another field holds a particular value.
    ///
    /// SFTP is the whole reason this exists: the key path and its passphrase
    /// belong to `auth_method == "key"`, the password to the other branch.
    /// Writing regardless stored credentials for a method that will never be
    /// read, because the builder dispatches on `auth_method` alone.
    pub only_when: Option<(&'static str, &'static str)>,
}

const fn pref(key: &'static str, pref: &'static str) -> FieldSpec {
    FieldSpec {
        key,
        pref: Some(pref),
        secret: None,
        only_when: None,
    }
}

const fn secret(key: &'static str, account: &'static str, slot: SecretSlot) -> FieldSpec {
    FieldSpec {
        key,
        pref: None,
        secret: Some((account, slot)),
        only_when: None,
    }
}

const fn when(spec: FieldSpec, field: &'static str, equals: &'static str) -> FieldSpec {
    FieldSpec {
        only_when: Some((field, equals)),
        ..spec
    }
}

const LOCAL: &[FieldSpec] = &[pref("path", PREF_LOCAL_PATH)];

const WEBDAV: &[FieldSpec] = &[
    pref("url", PREF_WEBDAV_URL),
    pref("user", PREF_WEBDAV_USER),
    secret("password", SECRET_ACCOUNT_WEBDAV, SecretSlot::Password),
];

const SFTP: &[FieldSpec] = &[
    pref("host", PREF_SFTP_HOST),
    pref("port", PREF_SFTP_PORT),
    pref("user", PREF_SFTP_USER),
    pref("path", PREF_SFTP_PATH),
    pref("auth_method", PREF_SFTP_AUTH_METHOD),
    when(pref("key_path", PREF_SFTP_KEY_PATH), "auth_method", "key"),
    when(
        secret(
            "key_passphrase",
            SECRET_ACCOUNT_SFTP_KEY,
            SecretSlot::Password,
        ),
        "auth_method",
        "key",
    ),
    when(
        secret("password", SECRET_ACCOUNT_SFTP, SecretSlot::Password),
        "auth_method",
        "password",
    ),
];

const FTP: &[FieldSpec] = &[
    pref("host", PREF_FTP_HOST),
    pref("port", PREF_FTP_PORT),
    pref("user", PREF_FTP_USER),
    pref("path", PREF_FTP_PATH),
    pref("mode", PREF_FTP_MODE),
    secret("password", SECRET_ACCOUNT_FTP, SecretSlot::Password),
];

const DROPBOX: &[FieldSpec] = &[
    pref("client_id", PREF_DROPBOX_CLIENT_ID),
    pref("client_secret", PREF_DROPBOX_CLIENT_SECRET),
    pref("path", PREF_DROPBOX_PATH),
];

const GOOGLEDRIVE: &[FieldSpec] = &[
    pref("client_id", PREF_GOOGLEDRIVE_CLIENT_ID),
    pref("client_secret", PREF_GOOGLEDRIVE_CLIENT_SECRET),
    pref("folder_name", PREF_GOOGLEDRIVE_FOLDER_NAME),
];

/// The fields each kind persists.
///
/// Dropbox and Google Drive carry no secret here on purpose: their refresh
/// token is already in the keychain from the OAuth exchange, and this path must
/// not touch it.
pub fn fields_for(kind: &str) -> Option<&'static [FieldSpec]> {
    Some(match kind {
        "local" => LOCAL,
        "webdav" => WEBDAV,
        "sftp" => SFTP,
        "ftp" => FTP,
        "dropbox" => DROPBOX,
        "googledrive" => GOOGLEDRIVE,
        _ => return None,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("unknown sync adapter kind: {0}")]
    UnknownKind(String),
    #[error("preferences: {0}")]
    Prefs(String),
    #[error("keychain: {0}")]
    Secret(String),
}

/// Read a field as the string it will be stored as.
///
/// Numbers arrive as JSON numbers from one host and as strings from the other —
/// a port is `2222` or `"2222"` depending on which frontend asked — and both
/// mean the same thing, so both are accepted rather than one being an error the
/// user cannot act on.
fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Write a target's configuration.
///
/// Empty and absent values are skipped, which is what lets a user change a host
/// or a path without re-entering the password: the stored secret survives
/// because nothing overwrote it. It also means this cannot be used to CLEAR a
/// field — that is `disconnect`'s job, and deliberately not a side effect of
/// editing.
pub fn persist(
    prefs: &UserPrefsRepo<'_>,
    secrets: &dyn SecretStore,
    kind: &str,
    values: &Map<String, Value>,
) -> Result<(), PersistError> {
    let fields = fields_for(kind).ok_or_else(|| PersistError::UnknownKind(kind.to_string()))?;

    for field in fields {
        if let Some((other, expected)) = field.only_when {
            let actual = values.get(other).and_then(as_text);
            if actual.as_deref() != Some(expected) {
                continue;
            }
        }
        let Some(text) = values.get(field.key).and_then(as_text) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if let Some(key) = field.pref {
            prefs
                .set(key, &text)
                .map_err(|err| PersistError::Prefs(err.to_string()))?;
        }
        if let Some((account, slot)) = field.secret {
            secrets
                .store(account, slot, &text)
                .map_err(|err| PersistError::Secret(err.to_string()))?;
        }
    }

    // Last, so a failure above leaves the target unselected rather than
    // selected-but-half-configured.
    prefs
        .set(PREF_ADAPTER_KIND, kind)
        .map_err(|err| PersistError::Prefs(err.to_string()))?;
    Ok(())
}

/// Read back what [`persist`] wrote, as the same value map it was given.
///
/// Secrets come back too: the builder needs them, and it is the caller that
/// decides whether to hand them onwards. Returns `None` when no target is
/// configured.
pub fn restore(
    prefs: &UserPrefsRepo<'_>,
    secrets: &dyn SecretStore,
) -> Option<(String, Map<String, Value>)> {
    let kind = prefs.get(PREF_ADAPTER_KIND).ok().flatten()?;
    if is_unconfigured(Some(&kind)) {
        return None;
    }
    let fields = fields_for(&kind)?;

    let mut out = Map::new();
    for field in fields {
        if let Some(key) = field.pref {
            if let Some(text) = prefs.get(key).ok().flatten() {
                out.insert(field.key.to_string(), Value::String(text));
            }
        }
        if let Some((account, slot)) = field.secret {
            if let Ok(text) = secrets.retrieve(account, slot) {
                out.insert(field.key.to_string(), Value::String(text));
            }
        }
    }
    Some((kind, out))
}

/// Every preference key any kind can write, for callers that need to reason
/// about the whole surface — the credential exporter, and the test below that
/// asserts none of them syncs.
pub fn all_pref_keys() -> BTreeMap<&'static str, &'static str> {
    let mut out = BTreeMap::new();
    for (kind, _) in KINDS {
        for field in fields_for(kind).unwrap_or(&[]) {
            if let Some(key) = field.pref {
                out.insert(key, *kind);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::db::DbHandle;

    #[derive(Default)]
    struct FakeSecrets {
        // Keyed by the slot's wire name: SecretSlot is not hashable, and the
        // wire name is what identifies it everywhere else anyway.
        entries: Mutex<HashMap<(String, &'static str), String>>,
    }

    impl SecretStore for FakeSecrets {
        fn store(
            &self,
            account_id: &str,
            slot: SecretSlot,
            value: &str,
        ) -> Result<(), sync_engine::SecretError> {
            self.entries.lock().unwrap().insert(
                (account_id.to_string(), slot.wire_name()),
                value.to_string(),
            );
            Ok(())
        }

        fn retrieve(
            &self,
            account_id: &str,
            slot: SecretSlot,
        ) -> Result<String, sync_engine::SecretError> {
            self.entries
                .lock()
                .unwrap()
                .get(&(account_id.to_string(), slot.wire_name()))
                .cloned()
                .ok_or(sync_engine::SecretError::NotFound)
        }

        fn delete(
            &self,
            account_id: &str,
            slot: SecretSlot,
        ) -> Result<(), sync_engine::SecretError> {
            self.entries
                .lock()
                .unwrap()
                .remove(&(account_id.to_string(), slot.wire_name()));
            Ok(())
        }

        fn delete_all(&self, account_id: &str) -> Result<(), sync_engine::SecretError> {
            self.entries
                .lock()
                .unwrap()
                .retain(|(id, _), _| id != account_id);
            Ok(())
        }
    }

    fn values(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
            .collect()
    }

    /// The property that matters: what a user typed comes back unchanged, for
    /// every kind this build serves. Six adapters, no keychain, no network.
    #[test]
    fn every_kind_round_trips() {
        let cases: &[(&str, &[(&str, &str)])] = &[
            ("local", &[("path", "/srv/aperio")]),
            (
                "webdav",
                &[
                    ("url", "https://example.test/dav/"),
                    ("user", "anna"),
                    ("password", "hunter2"),
                ],
            ),
            (
                "sftp",
                &[
                    ("host", "backup.example.test"),
                    ("port", "22"),
                    ("user", "anna"),
                    ("path", "/srv/aperio"),
                    ("auth_method", "password"),
                    ("password", "hunter2"),
                ],
            ),
            (
                "ftp",
                &[
                    ("host", "ftp.example.test"),
                    ("port", "21"),
                    ("user", "anna"),
                    ("path", "/aperio"),
                    ("mode", "explicit"),
                    ("password", "hunter2"),
                ],
            ),
            (
                "dropbox",
                &[
                    ("client_id", "abc"),
                    ("client_secret", "def"),
                    ("path", "/Apps/Aperio"),
                ],
            ),
            (
                "googledrive",
                &[
                    ("client_id", "abc"),
                    ("client_secret", "def"),
                    ("folder_name", "Aperio"),
                ],
            ),
        ];

        for (kind, pairs) in cases {
            let db = DbHandle::open_in_memory().unwrap();
            let shared = db.shared();
            let prefs = UserPrefsRepo::new(&shared);
            let secrets = FakeSecrets::default();

            persist(&prefs, &secrets, kind, &values(pairs)).unwrap();
            let (back_kind, back) = restore(&prefs, &secrets).expect("configured");

            assert_eq!(&back_kind, kind);
            for (k, v) in *pairs {
                assert_eq!(
                    back.get(*k).and_then(|v| v.as_str()),
                    Some(*v),
                    "{kind}: field {k} did not survive the round trip",
                );
            }
        }
    }

    /// Switching authentication method must not carry the other one's
    /// credential, and must not destroy it either — the slots are separate so
    /// switching back does not mean retyping a passphrase.
    #[test]
    fn sftp_writes_only_the_chosen_methods_credential() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let secrets = FakeSecrets::default();

        let base = [
            ("host", "backup.example.test"),
            ("port", "22"),
            ("user", "anna"),
            ("path", "/srv/aperio"),
        ];

        let mut with_key: Vec<(&str, &str)> = base.to_vec();
        with_key.extend([
            ("auth_method", "key"),
            ("key_path", "/home/anna/.ssh/id_ed25519"),
            ("key_passphrase", "keypass"),
            ("password", "should-not-be-written"),
        ]);
        persist(&prefs, &secrets, "sftp", &values(&with_key)).unwrap();

        assert_eq!(
            secrets
                .retrieve(SECRET_ACCOUNT_SFTP_KEY, SecretSlot::Password)
                .ok(),
            Some("keypass".to_string()),
        );
        assert!(
            secrets
                .retrieve(SECRET_ACCOUNT_SFTP, SecretSlot::Password)
                .is_err(),
            "a password was stored for key auth, which never reads it",
        );

        let mut with_password: Vec<(&str, &str)> = base.to_vec();
        with_password.extend([("auth_method", "password"), ("password", "hunter2")]);
        persist(&prefs, &secrets, "sftp", &values(&with_password)).unwrap();

        assert_eq!(
            secrets
                .retrieve(SECRET_ACCOUNT_SFTP, SecretSlot::Password)
                .ok(),
            Some("hunter2".to_string()),
        );
        assert_eq!(
            secrets
                .retrieve(SECRET_ACCOUNT_SFTP_KEY, SecretSlot::Password)
                .ok(),
            Some("keypass".to_string()),
            "switching away cleared the key passphrase; switching back would need it retyped",
        );
    }

    /// Editing a host without re-entering the password keeps the stored one.
    #[test]
    fn an_omitted_secret_leaves_the_stored_one_alone() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let secrets = FakeSecrets::default();

        persist(
            &prefs,
            &secrets,
            "webdav",
            &values(&[
                ("url", "https://example.test/dav/"),
                ("user", "anna"),
                ("password", "hunter2"),
            ]),
        )
        .unwrap();
        persist(
            &prefs,
            &secrets,
            "webdav",
            &values(&[("url", "https://elsewhere.test/dav/"), ("user", "anna")]),
        )
        .unwrap();

        let (_, back) = restore(&prefs, &secrets).unwrap();
        assert_eq!(
            back.get("url").and_then(|v| v.as_str()),
            Some("https://elsewhere.test/dav/"),
        );
        assert_eq!(
            back.get("password").and_then(|v| v.as_str()),
            Some("hunter2")
        );
    }

    #[test]
    fn nothing_configured_restores_to_nothing() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let secrets = FakeSecrets::default();
        assert!(restore(&prefs, &secrets).is_none());

        // The sentinel the mobile host used to write is still on real devices.
        prefs.set(PREF_ADAPTER_KIND, "none").unwrap();
        assert!(restore(&prefs, &secrets).is_none());
    }

    #[test]
    fn a_port_survives_arriving_as_a_number() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let secrets = FakeSecrets::default();

        let mut map = Map::new();
        map.insert("host".into(), Value::String("h".into()));
        map.insert("port".into(), Value::Number(2222.into()));
        map.insert("user".into(), Value::String("anna".into()));
        map.insert("path".into(), Value::String("/p".into()));
        map.insert("auth_method".into(), Value::String("password".into()));
        map.insert("password".into(), Value::String("pw".into()));
        persist(&prefs, &secrets, "sftp", &map).unwrap();

        let (_, back) = restore(&prefs, &secrets).unwrap();
        assert_eq!(back.get("port").and_then(|v| v.as_str()), Some("2222"));
    }

    /// The table and the module's own key list must not drift apart.
    #[test]
    fn every_table_pref_key_is_declared_and_device_local() {
        for (key, kind) in all_pref_keys() {
            assert!(
                key.starts_with("sync.adapter."),
                "{kind} writes {key}, which is outside the sync-target namespace",
            );
            assert!(
                !sync_engine::whitelist::is_synced_key(key),
                "{key} would cross devices",
            );
        }
    }
}
