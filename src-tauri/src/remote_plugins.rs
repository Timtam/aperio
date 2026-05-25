//! Local mirror of plugins that OTHER devices have announced
//! via the cross-device event log (DESIGN.md §20.8).
//!
//! When device B installs a community plugin, it appends a
//! `plugin.installed` event to the event log. Device A
//! receives the event at the next sync round + applies it
//! here: upsert a row in `remote_plugins` with the metadata
//! (id, name, version, plugin_type, source, who-announced,
//! when). Bundled plugins are never announced (they're
//! guaranteed present on every install).
//!
//! The Settings → Plugins panel renders these rows as the
//! "Plugin benötigt" section, letting the user pick the
//! corresponding `.aperio` file via the existing install
//! flow. Account rows whose adapter_kind maps to an
//! unrecognised plugin id check this table to render their
//! "Plugin fehlt" indicator.
//!
//! Removal: `plugin.uninstalled` events drop the row. The
//! local install command (`install_plugin_archive`) also
//! drops the row on success — once we have the binary, the
//! announcement is no longer pending.

use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use thiserror::Error;

use crate::db::SharedConn;

#[derive(Debug, Error)]
pub enum RemotePluginsError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type RemotePluginsResult<T> = Result<T, RemotePluginsError>;

/// Frontend-facing snapshot of one announcement. Mirrors the
/// columns in `remote_plugins`. Serialised straight to the
/// React side via the `list_remote_plugins` Tauri command.
#[derive(Debug, Clone, Serialize)]
pub struct RemotePluginAnnouncement {
    pub id: String,
    /// May be `None` for announcements that came from
    /// pre-iteration-21 Aperio devices (the PluginPayload's
    /// `name` field is optional for backward compat).
    pub name: Option<String>,
    pub version: String,
    pub plugin_type: Option<String>,
    pub source: Option<String>,
    pub announced_by_device: String,
    /// RFC 3339 timestamp.
    pub announced_at: String,
}

pub struct RemotePluginsRepo<'a> {
    db: &'a SharedConn,
}

impl<'a> RemotePluginsRepo<'a> {
    pub fn new(db: &'a SharedConn) -> Self {
        Self { db }
    }

    /// Upsert an announcement. Called from the event-log
    /// applier when it receives a `plugin.installed` or
    /// `plugin.updated` event from another device.
    pub fn upsert(
        &self,
        id: &str,
        name: Option<&str>,
        version: &str,
        plugin_type: Option<&str>,
        source: Option<&str>,
        announced_by_device: &str,
    ) -> RemotePluginsResult<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO remote_plugins
                (id, name, version, plugin_type, source,
                 announced_by_device, announced_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 name                = excluded.name,
                 version              = excluded.version,
                 plugin_type          = excluded.plugin_type,
                 source               = excluded.source,
                 announced_by_device  = excluded.announced_by_device,
                 announced_at         = excluded.announced_at",
            params![
                id,
                name,
                version,
                plugin_type,
                source,
                announced_by_device,
                now,
            ],
        )?;
        Ok(())
    }

    /// Drop the row for `id`. Called from the applier when a
    /// `plugin.uninstalled` event arrives, AND from the local
    /// install command after a successful install (the
    /// announcement isn't pending anymore once we have the
    /// binary).
    pub fn delete(&self, id: &str) -> RemotePluginsResult<()> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM remote_plugins WHERE id = ?", params![id])?;
        Ok(())
    }

    /// List every announcement, newest first. The Settings
    /// panel filters to "we don't have this id installed
    /// locally" on the host side before rendering — keeps
    /// the SQL trivial.
    pub fn list(&self) -> RemotePluginsResult<Vec<RemotePluginAnnouncement>> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, name, version, plugin_type, source,
                    announced_by_device, announced_at
               FROM remote_plugins
              ORDER BY announced_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RemotePluginAnnouncement {
                id: row.get(0)?,
                name: row.get(1)?,
                version: row.get(2)?,
                plugin_type: row.get(3)?,
                source: row.get(4)?,
                announced_by_device: row.get(5)?,
                announced_at: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::open(&dir.path().join("test.sqlite")).unwrap();
        (dir, db)
    }

    #[test]
    fn upsert_then_list_round_trips() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = RemotePluginsRepo::new(&shared);
        repo.upsert(
            "com.example.foo",
            Some("Foo Plugin"),
            "1.0.0",
            Some("calendar-adapter"),
            None,
            "device-alpha",
        )
        .unwrap();
        let list = repo.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "com.example.foo");
        assert_eq!(list[0].name.as_deref(), Some("Foo Plugin"));
        assert_eq!(list[0].announced_by_device, "device-alpha");
    }

    #[test]
    fn upsert_replaces_existing_row() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = RemotePluginsRepo::new(&shared);
        repo.upsert("com.example.foo", Some("Foo"), "1.0.0", None, None, "alpha")
            .unwrap();
        repo.upsert("com.example.foo", Some("Foo!"), "2.0.0", None, None, "beta")
            .unwrap();
        let list = repo.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].version, "2.0.0");
        assert_eq!(list[0].name.as_deref(), Some("Foo!"));
        assert_eq!(list[0].announced_by_device, "beta");
    }

    #[test]
    fn delete_drops_the_row() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = RemotePluginsRepo::new(&shared);
        repo.upsert("com.example.foo", None, "1.0.0", None, None, "alpha")
            .unwrap();
        repo.delete("com.example.foo").unwrap();
        assert!(repo.list().unwrap().is_empty());
    }

    #[test]
    fn delete_on_unknown_id_is_a_noop() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = RemotePluginsRepo::new(&shared);
        repo.delete("ghost").unwrap();
        assert!(repo.list().unwrap().is_empty());
    }

    #[test]
    fn list_orders_by_announced_at_desc() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = RemotePluginsRepo::new(&shared);
        repo.upsert("a", None, "1.0.0", None, None, "alpha").unwrap();
        // The sleep is overkill for production but the test
        // needs distinct RFC 3339 timestamps; chrono's
        // resolution is microseconds which the SQL ORDER BY
        // sees correctly without sleeping in 99.9% of cases,
        // but we sleep 2ms to be safe across CI runners.
        std::thread::sleep(std::time::Duration::from_millis(2));
        repo.upsert("b", None, "1.0.0", None, None, "alpha").unwrap();
        let list = repo.list().unwrap();
        assert_eq!(list[0].id, "b");
        assert_eq!(list[1].id, "a");
    }
}
