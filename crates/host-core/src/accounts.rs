//! Account model and persistence (DESIGN.md §6.2 + §6.4).
//!
//! An *account* is one user-configured instance of an adapter: the
//! adapter kind ("local", "caldav", "google", …), a human-readable
//! display name, and a small JSON config blob that holds the
//! adapter-specific non-secret configuration (server URL, OAuth
//! client id, calendar root path, …). Secrets live in the platform
//! keychain via the `secrets` module.
//!
//! This phase only models the storage layer + the account CRUD
//! commands. The actual `Adapter` instances per account, and the
//! aggregation of multiple adapters into the existing
//! calendar/event commands, arrive in Phase 6b when the first
//! external adapter (CalDAV) lands and there is a meaningful
//! "more than one adapter" scenario to route over.

use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::db::SharedConn;

/// The hard-coded account id of the implicit local adapter. Matches
/// `cal-adapter-local::SOURCE_ID` so existing rows that carry
/// `source = "local"` can be attached without rewriting them.
pub const LOCAL_ACCOUNT_ID: &str = "local";

/// Which adapter an account belongs to.
///
/// An **opaque string**, not an enumeration. It used to be the latter, and the
/// consequence was that the host had to be edited before any adapter could
/// exist: a variant here, an arm in the kind→plugin map, an arm in every match
/// that had to stay exhaustive. An adapter Aperio's authors had never seen
/// could not have an account at all.
///
/// Now the *adapter* declares which kind it serves, in its `plugin.json`
/// (`"adapter_kind": "caldav"`), and the host resolves kind → plugin by asking
/// the loaded plugins. Nothing in the host enumerates them.
///
/// ## Why this is safe for sync
///
/// The string is exactly what it always was. `accounts.adapter_kind` is a plain
/// `TEXT` column with no constraint; the sync payload has always carried a
/// `String`; the applier writes it through verbatim and has never parsed it.
/// So the persisted and on-the-wire representations are byte-identical before
/// and after this change, and a device on an older build reads a newer build's
/// rows exactly as it did yesterday. The serde form is `#[serde(transparent)]`
/// precisely to keep `"caldav"` serialising as `"caldav"` rather than as a
/// wrapper object.
///
/// What *does* improve: an unknown kind is no longer a parse failure. It is a
/// kind this build has no plugin for, which is a runtime fact about plugins
/// rather than a corrupt row, and it round-trips untouched.
///
/// ## The two host-internal kinds
///
/// [`Self::LOCAL`] and [`Self::DEVICE_CALENDAR`] are not plugins and never will
/// be: the first is the built-in store, the second is built in the cal-ffi
/// layer over a native bridge. They are recognised by value, and they are the
/// only two values this module knows by name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AdapterKind(String);

impl AdapterKind {
    /// The implicit local account's kind — the built-in store, no plugin.
    pub const LOCAL: &'static str = "local";
    /// The device's own calendar + reminders (iOS EventKit / Android
    /// CalendarProvider). Mobile-only, built in cal-ffi over a native bridge
    /// rather than by a plugin, and never written to the sync log.
    pub const DEVICE_CALENDAR: &'static str = "device_calendar";

    pub fn new(kind: impl Into<String>) -> Self {
        Self(kind.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The built-in local store.
    pub fn is_local(&self) -> bool {
        self.0 == Self::LOCAL
    }

    /// Built by the host itself rather than by a plugin, so there is no
    /// manifest to consult and nothing to register through the plugin path.
    pub fn is_host_internal(&self) -> bool {
        self.0 == Self::LOCAL || self.0 == Self::DEVICE_CALENDAR
    }

    /// Whether a kind string is well-formed enough to persist.
    ///
    /// Deliberately a *shape* check and not a whitelist: whether a kind is
    /// KNOWN depends on which plugins are loaded, which is a question for the
    /// plugin manager at the moment of use, not for a table here. This only
    /// keeps obvious junk out of a column that ends up in a sync payload.
    pub fn is_well_formed(&self) -> bool {
        !self.0.is_empty()
            && self.0.len() <= 128
            && self
                .0
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    }
}

impl std::fmt::Display for AdapterKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for AdapterKind {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for AdapterKind {
    fn from(kind: &str) -> Self {
        Self(kind.to_string())
    }
}

impl From<String> for AdapterKind {
    fn from(kind: String) -> Self {
        Self(kind)
    }
}

impl PartialEq<str> for AdapterKind {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for AdapterKind {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub adapter_kind: AdapterKind,
    pub display_name: String,
    /// Adapter-specific non-secret config (server URL, …). Stored
    /// verbatim as a JSON string so this module doesn't need to know
    /// the shape of every adapter's config.
    pub config_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Error)]
pub enum AccountsError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("account '{0}' not found")]
    NotFound(String),
    #[error("cannot delete the implicit local account")]
    DeleteLocalForbidden,
}

/// Read-side access to the `accounts` table. Stateless — every
/// operation acquires the connection lock from the shared handle.
pub struct AccountsRepo<'a> {
    pub(crate) db: &'a SharedConn,
}

impl<'a> AccountsRepo<'a> {
    pub fn new(db: &'a SharedConn) -> Self {
        Self { db }
    }

    pub fn list(&self) -> Result<Vec<Account>, AccountsError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, adapter_kind, display_name, config_json,
                    created_at, updated_at
               FROM accounts
              ORDER BY adapter_kind, display_name COLLATE NOCASE",
        )?;
        // Every row, whatever kind it names. `adapter_kind` is written into
        // this table by the sync applier as an opaque string, so a device on an
        // older build holds rows for adapters it has no plugin for the moment a
        // newer device creates one. Those rows list like any other and simply
        // fail to register, which the Accounts panel already shows as "plugin
        // missing". Earlier versions failed the whole listing on one such row —
        // which also aborted `register_persisted`, so a single unknown account
        // meant no adapters at all — and then skipped it, which hid the account
        // from its owner. Neither is necessary once the kind is opaque.
        let rows = stmt.query_map([], row_to_account)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Option<Account>, AccountsError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, adapter_kind, display_name, config_json,
                    created_at, updated_at
               FROM accounts WHERE id = ?",
        )?;
        match stmt.query_row(params![id], row_to_account) {
            Ok(account) => Ok(Some(account)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(other) => Err(AccountsError::Sqlite(other)),
        }
    }

    pub fn create(
        &self,
        adapter_kind: AdapterKind,
        display_name: &str,
        config_json: &str,
    ) -> Result<Account, AccountsError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO accounts (id, adapter_kind, display_name,
                                   config_json, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                id,
                adapter_kind.as_str(),
                display_name,
                config_json,
                now,
                now
            ],
        )?;
        Ok(Account {
            id,
            adapter_kind,
            display_name: display_name.to_string(),
            config_json: config_json.to_string(),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    /// Change an account's user-visible `display_name`. Returns the
    /// updated row (re-read so the caller has the fresh `updated_at`
    /// for the sync payload). The local account can be renamed — only
    /// deletion is forbidden for it.
    pub fn rename(&self, id: &str, display_name: &str) -> Result<Account, AccountsError> {
        let now = Utc::now().to_rfc3339();
        {
            let conn = self.db.lock().expect("db mutex poisoned");
            let changed = conn.execute(
                "UPDATE accounts SET display_name = ?, updated_at = ? WHERE id = ?",
                params![display_name, now, id],
            )?;
            if changed == 0 {
                return Err(AccountsError::NotFound(id.to_string()));
            }
        }
        self.get(id)?
            .ok_or_else(|| AccountsError::NotFound(id.to_string()))
    }

    pub fn delete(&self, id: &str) -> Result<(), AccountsError> {
        if id == LOCAL_ACCOUNT_ID {
            return Err(AccountsError::DeleteLocalForbidden);
        }
        let conn = self.db.lock().expect("db mutex poisoned");
        let changed = conn.execute("DELETE FROM accounts WHERE id = ?", params![id])?;
        if changed == 0 {
            return Err(AccountsError::NotFound(id.to_string()));
        }
        Ok(())
    }
}

/// Every row reads back, whatever kind it names.
///
/// This used to reject a kind the build did not know, which was the wrong
/// question: whether an adapter EXISTS is a fact about which plugins are
/// loaded, not about whether the row parses. A row whose plugin is missing is
/// listed like any other and simply fails to register — visibly, with the
/// "plugin missing" indicator the Accounts panel already has — instead of
/// vanishing from the listing.
fn row_to_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        adapter_kind: AdapterKind::new(row.get::<_, String>(1)?),
        display_name: row.get(2)?,
        config_json: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.sqlite");
        let db = DbHandle::open(&path).unwrap();
        (dir, db)
    }

    #[test]
    fn local_account_is_seeded_by_migration() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let local = repo.get(LOCAL_ACCOUNT_ID).unwrap();
        let local = local.expect("local account should be seeded");
        assert_eq!(local.adapter_kind, AdapterKind::new("local"));
        assert_eq!(local.display_name, "Local");
    }

    #[test]
    fn create_list_delete_roundtrip() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);

        let before = repo.list().unwrap();
        let initial_count = before.len();

        let new = repo
            .create(AdapterKind::new("caldav"), "Nextcloud (home)", "{}")
            .unwrap();
        let after = repo.list().unwrap();
        assert_eq!(after.len(), initial_count + 1);
        assert!(after.iter().any(|a| a.id == new.id));

        repo.delete(&new.id).unwrap();
        assert!(repo.get(&new.id).unwrap().is_none());
    }

    #[test]
    fn rename_updates_display_name() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let new = repo
            .create(AdapterKind::new("caldav"), "Old name", "{}")
            .unwrap();
        let renamed = repo.rename(&new.id, "New name").unwrap();
        assert_eq!(renamed.display_name, "New name");
        assert_eq!(repo.get(&new.id).unwrap().unwrap().display_name, "New name");
    }

    #[test]
    fn renaming_missing_account_errors() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let err = repo.rename("does-not-exist", "X").unwrap_err();
        assert!(matches!(err, AccountsError::NotFound(_)));
    }

    #[test]
    fn local_account_can_be_renamed() {
        // Unlike delete, renaming the implicit local account is allowed.
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let renamed = repo.rename(LOCAL_ACCOUNT_ID, "My device").unwrap();
        assert_eq!(renamed.display_name, "My device");
    }

    #[test]
    fn a_kind_this_build_has_no_plugin_for_still_reads_back_intact() {
        // THE sync-compatibility guarantee, stated as a test.
        //
        // `adapter_kind` is written into this table by the sync applier as an
        // opaque string and has never been parsed on the way in. A device on an
        // older build therefore receives rows for adapters it has never heard
        // of the moment a newer device creates one — and those rows must
        // survive: be listed, be fetchable, keep their kind byte-for-byte, and
        // still be there after this device is updated.
        //
        // This used to be a skip. Skipping was already an improvement over the
        // failure before it (one unknown row took the whole listing down, and
        // with it `register_persisted`, so NO adapter registered at all), but it
        // still hid the account from its owner. Now the row is ordinary: it
        // lists, and it simply has no plugin to register through, which the
        // Accounts panel already renders as "plugin missing".
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let known = repo
            .create(AdapterKind::new("caldav"), "Nextcloud", "{}")
            .unwrap();

        let exotic = "quantum_teleconference";
        {
            let conn = shared.lock().unwrap();
            conn.execute(
                "INSERT INTO accounts (id, adapter_kind, display_name,
                                       config_json, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![
                    "from-the-future",
                    exotic,
                    "Something newer",
                    "{}",
                    "2026-07-28T00:00:00Z",
                    "2026-07-28T00:00:00Z"
                ],
            )
            .unwrap();
        }

        let listed = repo.list().expect("an unknown kind must not fail the list");
        assert!(listed.iter().any(|a| a.id == known.id));
        let future = listed
            .iter()
            .find(|a| a.id == "from-the-future")
            .expect("the row must be listed, not hidden from its owner");
        assert_eq!(
            future.adapter_kind.as_str(),
            exotic,
            "the kind has to round-trip byte-for-byte — it is what the newer \
             device will match on"
        );

        // And by id, which is what a repair flow would ask for.
        let fetched = repo
            .get("from-the-future")
            .expect("get must not fail")
            .expect("the row exists");
        assert_eq!(fetched.adapter_kind.as_str(), exotic);
    }

    #[test]
    fn a_kind_serialises_as_the_bare_string_it_always_was() {
        // The sync payload carries `adapter_kind` as a plain string. If this
        // newtype ever serialised as a wrapper object, every device on an older
        // build would stop understanding this one's account events — the exact
        // failure this change exists to avoid. `#[serde(transparent)]` is what
        // prevents it, and nothing else would notice if it were dropped.
        let kind = AdapterKind::new("caldav");
        assert_eq!(serde_json::to_string(&kind).unwrap(), "\"caldav\"");
        let back: AdapterKind = serde_json::from_str("\"caldav\"").unwrap();
        assert_eq!(back, kind);

        // And a whole account row, since that is the shape that actually
        // travels.
        let account = Account {
            id: "a1".into(),
            adapter_kind: AdapterKind::new("webex"),
            display_name: "Work".into(),
            config_json: "{}".into(),
            created_at: "2026-07-28T00:00:00Z".into(),
            updated_at: "2026-07-28T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&account).unwrap();
        assert!(
            json.contains("\"adapter_kind\":\"webex\""),
            "the wire form changed: {json}"
        );
    }

    #[test]
    fn the_two_host_internal_kinds_are_the_only_ones_named() {
        assert!(AdapterKind::new("local").is_local());
        assert!(AdapterKind::new("local").is_host_internal());
        assert!(AdapterKind::new("device_calendar").is_host_internal());
        assert!(!AdapterKind::new("device_calendar").is_local());
        for other in ["caldav", "webex", "quantum_teleconference"] {
            assert!(
                !AdapterKind::new(other).is_host_internal(),
                "{other} is served by a plugin, not by the host"
            );
        }
    }

    #[test]
    fn well_formedness_is_a_shape_check_and_not_a_whitelist() {
        // Whether a kind is KNOWN depends on which plugins are loaded. This
        // only keeps junk out of a column that ends up in a sync payload.
        for good in ["local", "caldav", "microsoft_graph", "com.example.thing-2"] {
            assert!(AdapterKind::new(good).is_well_formed(), "{good}");
        }
        for bad in ["", "has space", "quote\"inside", "sla/sh", &"x".repeat(129)] {
            assert!(!AdapterKind::new(bad).is_well_formed(), "{bad:?}");
        }
    }

    #[test]
    fn deleting_local_is_refused() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let err = repo.delete(LOCAL_ACCOUNT_ID).unwrap_err();
        assert!(matches!(err, AccountsError::DeleteLocalForbidden));
    }
}
