//! The half of an account that stays on the machine that entered it.
//!
//! An account row travels between a user's devices, and almost everything on it
//! travels well — a server address, a user name, a client id, the name of a
//! folder in someone's Drive. A few values do not. An SSH key lives at
//! `/home/anna/.ssh/id_ed25519` on one machine and somewhere else entirely on
//! the next; a folder on a local disk means nothing anywhere else.
//!
//! Those values are kept here instead, keyed by account id, and never leave.
//!
//! ## Why the adapter decides, and not this module
//!
//! Nothing here inspects a value to guess whether it looks like a path. The
//! adapter marks the field `device_local` in its schema, because the adapter is
//! the only party that knows: a host cannot tell a filesystem path from a URL by
//! looking, and being wrong is costly in both directions — too eager and the
//! user retypes settings on every device they own, too shy and one machine's
//! paths overwrite another's.
//!
//! ## Why preferences rather than a table
//!
//! `user_prefs` is already the device-local store, and the sync whitelist is
//! already the mechanism that keeps it that way. A new table would need its own
//! exclusion from the snapshot — another thing to remember, in the place where
//! forgetting is silent. The test at the bottom checks the namespace against the
//! whitelist rather than trusting that reasoning.

use serde_json::{Map, Value};

use crate::db::SharedConn;
use crate::user_prefs::{UserPrefsRepo, UserPrefsResult};

/// Preference keys look like `account.<id>.<field>`.
///
/// Deliberately NOT under `sync.`: these belong to an account, not to a sync
/// target, and an account may never sync at all.
const PREFIX: &str = "account.";

fn key_for(account_id: &str, field: &str) -> String {
    format!("{PREFIX}{account_id}.{field}")
}

/// Write this device's half of an account, replacing what was there.
///
/// Fields absent from `values` are removed rather than left behind: a user who
/// switches SFTP from key auth to password should not keep a stale key path
/// that a later switch back would silently resurrect as if they had re-entered
/// it.
pub fn store(
    prefs: &UserPrefsRepo<'_>,
    account_id: &str,
    known_fields: &[String],
    values: &Map<String, Value>,
) -> UserPrefsResult<()> {
    for field in known_fields {
        match values.get(field) {
            Some(Value::String(text)) => prefs.set(&key_for(account_id, field), text)?,
            Some(Value::Bool(flag)) => prefs.set(&key_for(account_id, field), &flag.to_string())?,
            // Absent, null, or a shape this store does not carry.
            _ => prefs.delete(&key_for(account_id, field))?,
        }
    }
    Ok(())
}

/// Read back this device's half.
///
/// Everything comes back as it was written — text as text, a bool as the string
/// it was stored as. The caller merges it into the adapter's init config, where
/// the schema says which shape each field wants.
pub fn load(
    prefs: &UserPrefsRepo<'_>,
    account_id: &str,
    known_fields: &[String],
) -> Map<String, Value> {
    let mut out = Map::new();
    for field in known_fields {
        if let Ok(Some(text)) = prefs.get(&key_for(account_id, field)) {
            out.insert(field.clone(), Value::String(text));
        }
    }
    out
}

/// Forget this device's half of an account.
///
/// Called when the account is deleted. The account row's own removal syncs; this
/// does not, and must not — another device deleting the account does not tell
/// this one where its key file was, and nothing else would ever clean it up.
pub fn forget(
    prefs: &UserPrefsRepo<'_>,
    account_id: &str,
    known_fields: &[String],
) -> UserPrefsResult<()> {
    for field in known_fields {
        prefs.delete(&key_for(account_id, field))?;
    }
    Ok(())
}

/// The registry's [`crate::registry::DeviceLocalRead`], backed by this module.
///
/// A separate type rather than an impl on some existing one, because the
/// registry deliberately knows nothing about databases: it holds the trait, and
/// this is the only thing in the tree that satisfies it. A test host could
/// satisfy it with a map and never touch SQLite.
pub struct PrefsDeviceLocal {
    db: SharedConn,
}

impl PrefsDeviceLocal {
    pub fn new(db: SharedConn) -> Self {
        Self { db }
    }
}

impl crate::registry::DeviceLocalRead for PrefsDeviceLocal {
    fn load(&self, account_id: &str, fields: &[String]) -> Map<String, Value> {
        load(&UserPrefsRepo::new(&self.db), account_id, fields)
    }
}

/// Forget everything stored for an account, whatever the schema says today.
///
/// Prefix-based rather than field-based on purpose. The caller reaches this
/// after the account row is already gone, so the adapter kind — and with it the
/// schema — is no longer available to ask; and a field the schema USED to
/// declare would otherwise be left behind forever, since nothing would ever
/// name it again.
pub fn forget_all(prefs: &UserPrefsRepo<'_>, account_id: &str) -> UserPrefsResult<()> {
    // `_` is a LIKE wildcard and account ids are free-form, so the pattern is
    // escaped rather than trusted.
    let escaped = account_id
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_");
    let pattern = format!("{PREFIX}{escaped}.%");
    let conn = prefs.db.lock().expect("db mutex poisoned");
    conn.execute(
        "DELETE FROM user_prefs WHERE key LIKE ?1 ESCAPE '!'",
        rusqlite::params![pattern],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;

    fn fields() -> Vec<String> {
        vec!["key_path".to_string(), "auth_method".to_string()]
    }

    #[test]
    fn a_round_trip_keeps_what_was_written() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);

        let mut values = Map::new();
        values.insert(
            "key_path".into(),
            Value::String("/home/anna/.ssh/id_ed25519".into()),
        );
        values.insert("auth_method".into(), Value::String("key".into()));
        store(&prefs, "acc-1", &fields(), &values).unwrap();

        let back = load(&prefs, "acc-1", &fields());
        assert_eq!(
            back.get("key_path").and_then(|v| v.as_str()),
            Some("/home/anna/.ssh/id_ed25519"),
        );
        assert_eq!(
            back.get("auth_method").and_then(|v| v.as_str()),
            Some("key")
        );
    }

    /// Two accounts must not read each other's paths.
    #[test]
    fn accounts_do_not_share() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);

        let mut a = Map::new();
        a.insert("key_path".into(), Value::String("/a".into()));
        store(&prefs, "acc-1", &fields(), &a).unwrap();

        let mut b = Map::new();
        b.insert("key_path".into(), Value::String("/b".into()));
        store(&prefs, "acc-2", &fields(), &b).unwrap();

        assert_eq!(
            load(&prefs, "acc-1", &fields())
                .get("key_path")
                .and_then(|v| v.as_str()),
            Some("/a"),
        );
        assert_eq!(
            load(&prefs, "acc-2", &fields())
                .get("key_path")
                .and_then(|v| v.as_str()),
            Some("/b"),
        );
    }

    /// A field the user cleared must not survive as a value they never re-entered.
    #[test]
    fn an_omitted_field_is_removed_rather_than_kept() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);

        let mut with_key = Map::new();
        with_key.insert("key_path".into(), Value::String("/home/anna/id".into()));
        with_key.insert("auth_method".into(), Value::String("key".into()));
        store(&prefs, "acc-1", &fields(), &with_key).unwrap();

        let mut with_password = Map::new();
        with_password.insert("auth_method".into(), Value::String("password".into()));
        store(&prefs, "acc-1", &fields(), &with_password).unwrap();

        let back = load(&prefs, "acc-1", &fields());
        assert!(
            back.get("key_path").is_none(),
            "a stale key path survived a switch away from key auth",
        );
    }

    #[test]
    fn forget_all_clears_fields_the_schema_no_longer_declares() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);

        let mut values = Map::new();
        values.insert("key_path".into(), Value::String("/x".into()));
        store(&prefs, "acc-1", &fields(), &values).unwrap();
        // A field from an older schema version, which nothing would name again.
        prefs
            .set("account.acc-1.retired_field", "leftover")
            .unwrap();
        // Another account, which must survive.
        prefs.set("account.acc-2.key_path", "/keep").unwrap();

        forget_all(&prefs, "acc-1").unwrap();

        assert!(load(&prefs, "acc-1", &fields()).is_empty());
        assert_eq!(prefs.get("account.acc-1.retired_field").unwrap(), None);
        assert_eq!(
            prefs.get("account.acc-2.key_path").unwrap().as_deref(),
            Some("/keep"),
        );
    }

    #[test]
    fn forgetting_an_account_leaves_nothing() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);

        let mut values = Map::new();
        values.insert("key_path".into(), Value::String("/x".into()));
        store(&prefs, "acc-1", &fields(), &values).unwrap();
        forget(&prefs, "acc-1", &fields()).unwrap();
        assert!(load(&prefs, "acc-1", &fields()).is_empty());
    }

    /// The invariant the whole idea rests on, checked rather than assumed.
    ///
    /// If these keys ever synced, the values that exist precisely BECAUSE they
    /// differ per machine would be copied between machines — which is worse than
    /// not having the feature, because it would look like it worked.
    #[test]
    fn these_keys_never_cross_devices() {
        for key in [
            key_for("acc-1", "key_path"),
            key_for("acc-1", "auth_method"),
            key_for("some-uuid-with-dots.in.it", "path"),
        ] {
            assert!(
                !sync_engine::whitelist::is_synced_key(&key),
                "{key} would be copied to another device",
            );
        }
    }
}
