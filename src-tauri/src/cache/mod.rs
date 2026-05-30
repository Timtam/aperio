//! Persistent snapshot cache for EXTERNAL adapters.
//!
//! This is the host-owned, adapter-independent store that backs the
//! stale-while-revalidate read path (CACHE-1+). It mirrors the last
//! snapshot an external provider handed us (events, tasks, contacts and
//! their container listings) into `cache_*` tables (migration 0019) so a
//! fresh app start can paint INSTANTLY instead of waiting on the network.
//!
//! It is deliberately a *cache*, not a source of truth:
//!   - Only external-account data lives here. The local adapter's
//!     `source='local'` tables and the event-log applier never touch it.
//!   - Rows carry `account_id` (FK → accounts, ON DELETE CASCADE), so a
//!     deleted account wipes its cache automatically and two accounts
//!     can never collide on a shared native id.
//!   - The full `cal_core` struct round-trips through a JSON `payload`
//!     column; only the fields we actually query (event start/end) are
//!     surfaced as columns. A cache-shape change is a drop+rewarm, never
//!     a data migration.
//!
//! CACHE-0 ships the store + primitives only; wiring into the command
//! read path is CACHE-1.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::Serialize;

use cal_core::{Calendar, Contact, ContactList, DateRange, Event, Task, TaskList};

use crate::db::{DbError, DbHandle, DbResult};

#[cfg(test)]
mod tests;

/// Host-owned snapshot cache over the main SQLite database.
#[derive(Clone)]
pub struct CacheStore {
    db: DbHandle,
}

/// Which logical container/item-set a [`SyncState`] row describes.
///
/// The three listing scopes are account-wide (`container_id == ""`); the
/// three item scopes are per-container (`container_id` is the calendar /
/// list id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncScope {
    Calendars,
    TaskLists,
    ContactLists,
    Events,
    Tasks,
    Contacts,
}

impl SyncScope {
    /// Stable wire string (also the `cache-updated` event's `scope`).
    pub fn as_str(self) -> &'static str {
        match self {
            SyncScope::Calendars => "calendars",
            SyncScope::TaskLists => "task_lists",
            SyncScope::ContactLists => "contact_lists",
            SyncScope::Events => "events",
            SyncScope::Tasks => "tasks",
            SyncScope::Contacts => "contacts",
        }
    }
}

/// Delta token + covered window + freshness diagnostics for one
/// (account, scope, container) tuple. All fields are optional: a row
/// that was only ever full-refreshed has no `sync_token`; only the
/// `Events` scope fills the window bounds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncState {
    pub sync_token: Option<String>,
    pub ctag: Option<String>,
    pub window_start: Option<DateTime<Utc>>,
    pub window_end: Option<DateTime<Utc>>,
    pub last_refreshed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Outcome of an incremental sync the host hands to [`CacheStore`].
#[derive(Debug, Clone, Default)]
pub struct Delta<T> {
    /// Created or updated rows to upsert.
    pub changes: Vec<T>,
    /// Native ids to remove from the cache.
    pub deletions: Vec<String>,
    /// New opaque token to persist for the next round.
    pub new_token: Option<String>,
}

/// Deduplication guard for background refreshes (CACHE-1+). Keyed by an
/// opaque string like `"events:{account}:{calendar}"` so two concurrent
/// reads of the same container don't stack redundant network refreshes.
#[derive(Default)]
pub struct RefreshCoordinator {
    in_flight: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl RefreshCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Claim `key`. Returns `true` if the caller now owns the refresh
    /// (and must call [`RefreshCoordinator::release`] when done), or
    /// `false` if a refresh for this key is already running.
    pub fn try_claim(&self, key: &str) -> bool {
        self.in_flight
            .lock()
            .expect("refresh coordinator poisoned")
            .insert(key.to_string())
    }

    pub fn release(&self, key: &str) {
        self.in_flight
            .lock()
            .expect("refresh coordinator poisoned")
            .remove(key);
    }
}

/// Payload for the `cache-updated` Tauri event. A background refresh
/// emits this after writing fresh data for one container so the
/// frontend can invalidate the matching view (stale-while-revalidate).
#[derive(Debug, Clone, Serialize)]
pub struct CacheUpdatedPayload {
    /// One of [`SyncScope::as_str`] — which kind of data changed.
    pub scope: String,
    pub account_id: String,
    /// Calendar / list id, or `""` for the account-wide listing scopes.
    pub container_id: String,
}

impl CacheStore {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    // ── Events ───────────────────────────────────────────────────────

    /// Read cached events for `calendar` overlapping `range` (half-open).
    pub fn read_events(
        &self,
        account: &str,
        calendar: &str,
        range: DateRange,
    ) -> DbResult<Vec<Event>> {
        let start = ts(&range.start);
        let end = ts(&range.end);
        self.db.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT payload FROM cache_events
                 WHERE account_id = ?1 AND calendar_id = ?2
                   AND start_utc < ?3 AND end_utc > ?4
                 ORDER BY start_utc",
            )?;
            let rows = stmt.query_map(params![account, calendar, end, start], |r| {
                r.get::<_, String>(0)
            })?;
            rows_to_structs(rows, "cache_events")
        })
    }

    /// Full-refresh write: replace the entire cached set for `calendar`
    /// with `events` and record `range` as the covered window. Keeps any
    /// existing delta token (a full fetch doesn't invalidate it).
    pub fn replace_calendar_events(
        &self,
        account: &str,
        calendar: &str,
        range: DateRange,
        events: &[Event],
    ) -> DbResult<()> {
        let now = now_ts();
        let (ws, we) = (ts(&range.start), ts(&range.end));
        self.db.with_tx(|tx| {
            tx.execute(
                "DELETE FROM cache_events WHERE account_id = ?1 AND calendar_id = ?2",
                params![account, calendar],
            )?;
            for ev in events {
                insert_event(tx, account, calendar, ev, &now)?;
            }
            tx.execute(
                "INSERT INTO cache_sync_state
                   (account_id, scope, container_id, window_start, window_end, last_refreshed_at, last_error)
                 VALUES (?1, 'events', ?2, ?3, ?4, ?5, NULL)
                 ON CONFLICT(account_id, scope, container_id) DO UPDATE SET
                   window_start = excluded.window_start,
                   window_end = excluded.window_end,
                   last_refreshed_at = excluded.last_refreshed_at,
                   last_error = NULL",
                params![account, calendar, ws, we, now],
            )?;
            Ok(())
        })
    }

    /// Incremental write: upsert `delta.changes`, remove
    /// `delta.deletions`, persist the new token. Window is untouched.
    pub fn apply_events_delta(
        &self,
        account: &str,
        calendar: &str,
        delta: &Delta<Event>,
    ) -> DbResult<()> {
        let now = now_ts();
        self.db.with_tx(|tx| {
            // Clear the native group of every incoming change BEFORE
            // upserting. An updated provider resource keeps its native id
            // but can mint a different composite cal-core id — EWS rotates
            // the ChangeKey embedded in the id on every edit — so a plain
            // upsert-by-id would leave the stale pre-update row behind as a
            // duplicate. Purging the whole native group first also drops
            // occurrence overrides a recurring master no longer carries
            // (their cal-core id is derived from the master's native id, so
            // they share the group). The fresh `changes` for that resource
            // — master plus its current overrides — are re-inserted below.
            let mut purged: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for ev in &delta.changes {
                let native = native_id(&ev.id);
                if purged.insert(native) {
                    tx.execute(
                        "DELETE FROM cache_events
                         WHERE account_id = ?1 AND calendar_id = ?2 AND native_id = ?3",
                        params![account, calendar, native],
                    )?;
                }
            }
            for ev in &delta.changes {
                insert_event(tx, account, calendar, ev, &now)?;
            }
            for native in &delta.deletions {
                // A delta deletion carries the provider-native id, not
                // the composite cal-core id — match on `native_id`. This
                // also fans out correctly when several cached rows share
                // one native resource (e.g. a recurring master's
                // occurrences).
                tx.execute(
                    "DELETE FROM cache_events
                     WHERE account_id = ?1 AND calendar_id = ?2 AND native_id = ?3",
                    params![account, calendar, native],
                )?;
            }
            tx.execute(
                "INSERT INTO cache_sync_state
                   (account_id, scope, container_id, sync_token, last_refreshed_at, last_error)
                 VALUES (?1, 'events', ?2, ?3, ?4, NULL)
                 ON CONFLICT(account_id, scope, container_id) DO UPDATE SET
                   sync_token = excluded.sync_token,
                   last_refreshed_at = excluded.last_refreshed_at,
                   last_error = NULL",
                params![account, calendar, delta.new_token, now],
            )?;
            Ok(())
        })
    }

    /// The covered event window for `calendar`, if any has been recorded.
    pub fn event_window(
        &self,
        account: &str,
        calendar: &str,
    ) -> DbResult<Option<(DateTime<Utc>, DateTime<Utc>)>> {
        let state = self.get_sync_state(account, SyncScope::Events, calendar)?;
        Ok(match state {
            Some(SyncState {
                window_start: Some(s),
                window_end: Some(e),
                ..
            }) => Some((s, e)),
            _ => None,
        })
    }

    // ── Tasks ────────────────────────────────────────────────────────

    pub fn read_tasks(&self, account: &str, list: &str) -> DbResult<Vec<Task>> {
        self.read_by_list("cache_tasks", account, list)
    }

    pub fn replace_list_tasks(&self, account: &str, list: &str, tasks: &[Task]) -> DbResult<()> {
        self.replace_by_list("cache_tasks", SyncScope::Tasks, account, list, tasks, |t| {
            t.id.as_str()
        })
    }

    pub fn apply_tasks_delta(
        &self,
        account: &str,
        list: &str,
        delta: &Delta<Task>,
    ) -> DbResult<()> {
        self.apply_by_list_delta("cache_tasks", SyncScope::Tasks, account, list, delta, |t| {
            t.id.as_str()
        })
    }

    // ── Contacts ─────────────────────────────────────────────────────

    pub fn read_contacts(&self, account: &str, list: &str) -> DbResult<Vec<Contact>> {
        self.read_by_list("cache_contacts", account, list)
    }

    pub fn replace_list_contacts(
        &self,
        account: &str,
        list: &str,
        contacts: &[Contact],
    ) -> DbResult<()> {
        self.replace_by_list(
            "cache_contacts",
            SyncScope::Contacts,
            account,
            list,
            contacts,
            |c| c.id.as_str(),
        )
    }

    pub fn apply_contacts_delta(
        &self,
        account: &str,
        list: &str,
        delta: &Delta<Contact>,
    ) -> DbResult<()> {
        self.apply_by_list_delta(
            "cache_contacts",
            SyncScope::Contacts,
            account,
            list,
            delta,
            |c| c.id.as_str(),
        )
    }

    // ── Container listings ───────────────────────────────────────────

    pub fn read_calendars(&self, account: &str) -> DbResult<Vec<Calendar>> {
        self.read_listing("cache_calendars", account)
    }

    pub fn replace_calendars(&self, account: &str, calendars: &[Calendar]) -> DbResult<()> {
        self.replace_listing(
            "cache_calendars",
            SyncScope::Calendars,
            account,
            calendars,
            |c| c.id.as_str(),
        )
    }

    pub fn read_task_lists(&self, account: &str) -> DbResult<Vec<TaskList>> {
        self.read_listing("cache_task_lists", account)
    }

    pub fn replace_task_lists(&self, account: &str, lists: &[TaskList]) -> DbResult<()> {
        self.replace_listing(
            "cache_task_lists",
            SyncScope::TaskLists,
            account,
            lists,
            |l| l.id.as_str(),
        )
    }

    pub fn read_contact_lists(&self, account: &str) -> DbResult<Vec<ContactList>> {
        self.read_listing("cache_contact_lists", account)
    }

    pub fn replace_contact_lists(&self, account: &str, lists: &[ContactList]) -> DbResult<()> {
        self.replace_listing(
            "cache_contact_lists",
            SyncScope::ContactLists,
            account,
            lists,
            |l| l.id.as_str(),
        )
    }

    // ── Write-through (single row) ───────────────────────────────────
    // Used after an aperio-side mutation succeeds against an external
    // provider, so the snapshot reflects the change immediately instead
    // of showing stale data until the next background refresh. These
    // intentionally do NOT touch `cache_sync_state` (token/window/
    // freshness stay as the last full/delta refresh left them).

    pub fn upsert_event(&self, account: &str, calendar: &str, event: &Event) -> DbResult<()> {
        let now = now_ts();
        self.db
            .with_conn(|c| insert_event(c, account, calendar, event, &now))
    }

    pub fn remove_event(&self, account: &str, calendar: &str, id: &str) -> DbResult<()> {
        self.db.with_conn(|c| {
            c.execute(
                "DELETE FROM cache_events WHERE account_id = ?1 AND calendar_id = ?2 AND id = ?3",
                params![account, calendar, id],
            )?;
            Ok(())
        })
    }

    pub fn upsert_task(&self, account: &str, list: &str, task: &Task) -> DbResult<()> {
        self.upsert_by_list("cache_tasks", account, list, task, task.id.as_str())
    }

    pub fn remove_task(&self, account: &str, list: &str, id: &str) -> DbResult<()> {
        self.remove_by_list("cache_tasks", account, list, id)
    }

    pub fn upsert_contact(&self, account: &str, list: &str, contact: &Contact) -> DbResult<()> {
        self.upsert_by_list(
            "cache_contacts",
            account,
            list,
            contact,
            contact.id.as_str(),
        )
    }

    pub fn remove_contact(&self, account: &str, list: &str, id: &str) -> DbResult<()> {
        self.remove_by_list("cache_contacts", account, list, id)
    }

    pub fn upsert_task_list(&self, account: &str, list: &TaskList) -> DbResult<()> {
        self.upsert_listing("cache_task_lists", account, list, list.id.as_str())
    }

    pub fn remove_task_list(&self, account: &str, id: &str) -> DbResult<()> {
        self.remove_listing("cache_task_lists", account, id)
    }

    pub fn upsert_contact_list(&self, account: &str, list: &ContactList) -> DbResult<()> {
        self.upsert_listing("cache_contact_lists", account, list, list.id.as_str())
    }

    pub fn remove_contact_list(&self, account: &str, id: &str) -> DbResult<()> {
        self.remove_listing("cache_contact_lists", account, id)
    }

    fn upsert_by_list<T: Serialize>(
        &self,
        table: &str,
        account: &str,
        list: &str,
        item: &T,
        id: &str,
    ) -> DbResult<()> {
        let now = now_ts();
        let json = to_json(item, table)?;
        let sql = format!(
            "INSERT INTO {table} (account_id, list_id, id, native_id, payload, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(account_id, list_id, id) DO UPDATE SET
               native_id = excluded.native_id,
               payload = excluded.payload, cached_at = excluded.cached_at"
        );
        self.db.with_conn(|c| {
            c.execute(&sql, params![account, list, id, native_id(id), json, now])?;
            Ok(())
        })
    }

    fn remove_by_list(&self, table: &str, account: &str, list: &str, id: &str) -> DbResult<()> {
        let sql = format!("DELETE FROM {table} WHERE account_id = ?1 AND list_id = ?2 AND id = ?3");
        self.db.with_conn(|c| {
            c.execute(&sql, params![account, list, id])?;
            Ok(())
        })
    }

    fn upsert_listing<T: Serialize>(
        &self,
        table: &str,
        account: &str,
        item: &T,
        id: &str,
    ) -> DbResult<()> {
        let now = now_ts();
        let json = to_json(item, table)?;
        let sql = format!(
            "INSERT INTO {table} (account_id, id, payload, cached_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, id) DO UPDATE SET
               payload = excluded.payload, cached_at = excluded.cached_at"
        );
        self.db.with_conn(|c| {
            c.execute(&sql, params![account, id, json, now])?;
            Ok(())
        })
    }

    fn remove_listing(&self, table: &str, account: &str, id: &str) -> DbResult<()> {
        let sql = format!("DELETE FROM {table} WHERE account_id = ?1 AND id = ?2");
        self.db.with_conn(|c| {
            c.execute(&sql, params![account, id])?;
            Ok(())
        })
    }

    // ── Sync state ───────────────────────────────────────────────────

    pub fn get_sync_state(
        &self,
        account: &str,
        scope: SyncScope,
        container: &str,
    ) -> DbResult<Option<SyncState>> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT sync_token, ctag, window_start, window_end, last_refreshed_at, last_error
                   FROM cache_sync_state
                  WHERE account_id = ?1 AND scope = ?2 AND container_id = ?3",
                params![account, scope.as_str(), container],
                |r| {
                    Ok(SyncState {
                        sync_token: r.get(0)?,
                        ctag: r.get(1)?,
                        window_start: opt_ts(r.get::<_, Option<String>>(2)?)?,
                        window_end: opt_ts(r.get::<_, Option<String>>(3)?)?,
                        last_refreshed_at: opt_ts(r.get::<_, Option<String>>(4)?)?,
                        last_error: r.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
        })
    }

    /// Overwrite the full sync-state row for a tuple.
    pub fn set_sync_state(
        &self,
        account: &str,
        scope: SyncScope,
        container: &str,
        state: &SyncState,
    ) -> DbResult<()> {
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO cache_sync_state
                   (account_id, scope, container_id, sync_token, ctag,
                    window_start, window_end, last_refreshed_at, last_error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(account_id, scope, container_id) DO UPDATE SET
                   sync_token = excluded.sync_token,
                   ctag = excluded.ctag,
                   window_start = excluded.window_start,
                   window_end = excluded.window_end,
                   last_refreshed_at = excluded.last_refreshed_at,
                   last_error = excluded.last_error",
                params![
                    account,
                    scope.as_str(),
                    container,
                    state.sync_token,
                    state.ctag,
                    state.window_start.as_ref().map(ts),
                    state.window_end.as_ref().map(ts),
                    state.last_refreshed_at.as_ref().map(ts),
                    state.last_error,
                ],
            )?;
            Ok(())
        })
    }

    /// Persist a delta token + freshness for a container without touching
    /// its cached rows or event window. Used by the full-resync branch of
    /// a delta refresh (the rows were just replaced; this records the
    /// fresh token to continue incrementally next round).
    pub fn set_token(
        &self,
        account: &str,
        scope: SyncScope,
        container: &str,
        token: Option<&str>,
    ) -> DbResult<()> {
        let now = now_ts();
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO cache_sync_state
                   (account_id, scope, container_id, sync_token, last_refreshed_at, last_error)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL)
                 ON CONFLICT(account_id, scope, container_id) DO UPDATE SET
                   sync_token = excluded.sync_token,
                   last_refreshed_at = excluded.last_refreshed_at,
                   last_error = NULL",
                params![account, scope.as_str(), container, token, now],
            )?;
            Ok(())
        })
    }

    /// Force the next read of a container to go cold (re-fetch from the
    /// provider) by clearing the freshness markers: `last_refreshed_at`
    /// (gates tasks/contacts/listings) and the event window (gates
    /// events). The cached rows themselves stay as an offline fallback.
    /// Used after an aperio-side mutation whose exact row delta is
    /// awkward to apply surgically (e.g. a cross-container move).
    pub fn invalidate(&self, account: &str, scope: SyncScope, container: &str) -> DbResult<()> {
        self.db.with_conn(|c| {
            c.execute(
                "UPDATE cache_sync_state
                    SET window_start = NULL, window_end = NULL, last_refreshed_at = NULL
                  WHERE account_id = ?1 AND scope = ?2 AND container_id = ?3",
                params![account, scope.as_str(), container],
            )?;
            Ok(())
        })
    }

    /// Record a failed refresh: stamp `last_error`, leave the rest
    /// (including the still-valid cached data + window) intact.
    pub fn mark_error(
        &self,
        account: &str,
        scope: SyncScope,
        container: &str,
        message: &str,
    ) -> DbResult<()> {
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO cache_sync_state
                   (account_id, scope, container_id, last_error)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(account_id, scope, container_id) DO UPDATE SET
                   last_error = excluded.last_error",
                params![account, scope.as_str(), container, message],
            )?;
            Ok(())
        })
    }

    // ── Pruning ──────────────────────────────────────────────────────

    /// Drop every cached row + sync-state for one account (credential
    /// reset, manual "clear cache", or belt-and-braces on account
    /// removal — the FK cascade already handles the delete path).
    pub fn prune_account(&self, account: &str) -> DbResult<()> {
        self.db.with_tx(|tx| {
            for table in CACHE_TABLES {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE account_id = ?1"),
                    params![account],
                )?;
            }
            Ok(())
        })
    }

    /// Housekeeping: drop cached events for `calendar` that fall entirely
    /// outside `keep` (end ≤ keep.start or start ≥ keep.end). Used when
    /// the rolling window slides forward.
    pub fn prune_events_outside(
        &self,
        account: &str,
        calendar: &str,
        keep: DateRange,
    ) -> DbResult<()> {
        let start = ts(&keep.start);
        let end = ts(&keep.end);
        self.db.with_conn(|c| {
            c.execute(
                "DELETE FROM cache_events
                  WHERE account_id = ?1 AND calendar_id = ?2
                    AND (end_utc <= ?3 OR start_utc >= ?4)",
                params![account, calendar, start, end],
            )?;
            Ok(())
        })
    }

    // ── Generic helpers (JSON-blob tables) ───────────────────────────

    fn read_listing<T: DeserializeOwned>(&self, table: &str, account: &str) -> DbResult<Vec<T>> {
        let sql = format!("SELECT payload FROM {table} WHERE account_id = ?1 ORDER BY id");
        self.db.with_conn(|c| {
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map(params![account], |r| r.get::<_, String>(0))?;
            rows_to_structs(rows, table)
        })
    }

    fn replace_listing<T: Serialize>(
        &self,
        table: &str,
        scope: SyncScope,
        account: &str,
        items: &[T],
        id_of: impl Fn(&T) -> &str,
    ) -> DbResult<()> {
        let now = now_ts();
        let del = format!("DELETE FROM {table} WHERE account_id = ?1");
        let ins = format!(
            "INSERT INTO {table} (account_id, id, payload, cached_at) VALUES (?1, ?2, ?3, ?4)"
        );
        self.db.with_tx(|tx| {
            tx.execute(&del, params![account])?;
            for item in items {
                let json = to_json(item, table)?;
                tx.execute(&ins, params![account, id_of(item), json, now])?;
            }
            mark_refreshed(tx, account, scope, "", &now)?;
            Ok(())
        })
    }

    fn read_by_list<T: DeserializeOwned>(
        &self,
        table: &str,
        account: &str,
        list: &str,
    ) -> DbResult<Vec<T>> {
        let sql = format!(
            "SELECT payload FROM {table} WHERE account_id = ?1 AND list_id = ?2 ORDER BY id"
        );
        self.db.with_conn(|c| {
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map(params![account, list], |r| r.get::<_, String>(0))?;
            rows_to_structs(rows, table)
        })
    }

    fn replace_by_list<T: Serialize>(
        &self,
        table: &str,
        scope: SyncScope,
        account: &str,
        list: &str,
        items: &[T],
        id_of: impl Fn(&T) -> &str,
    ) -> DbResult<()> {
        let now = now_ts();
        let del = format!("DELETE FROM {table} WHERE account_id = ?1 AND list_id = ?2");
        let ins = format!(
            "INSERT INTO {table} (account_id, list_id, id, native_id, payload, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        );
        self.db.with_tx(|tx| {
            tx.execute(&del, params![account, list])?;
            for item in items {
                let id = id_of(item);
                let json = to_json(item, table)?;
                tx.execute(&ins, params![account, list, id, native_id(id), json, now])?;
            }
            mark_refreshed(tx, account, scope, list, &now)?;
            Ok(())
        })
    }

    fn apply_by_list_delta<T: Serialize>(
        &self,
        table: &str,
        scope: SyncScope,
        account: &str,
        list: &str,
        delta: &Delta<T>,
        id_of: impl Fn(&T) -> &str,
    ) -> DbResult<()> {
        let now = now_ts();
        let upsert = format!(
            "INSERT INTO {table} (account_id, list_id, id, native_id, payload, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(account_id, list_id, id) DO UPDATE SET
               native_id = excluded.native_id,
               payload = excluded.payload, cached_at = excluded.cached_at"
        );
        // A delta deletion matches either the provider-native id (CalDAV
        // href, EWS ItemId — `native_id`) OR the full cal-core id. The
        // latter covers composite ids whose `native_id` derives to the
        // *container* rather than the resource: Graph To Do tasks are
        // `{list}|{task}`, so `native_id` is the list and can't single out
        // one task — the adapter emits the full `{list}|{task}` id, which
        // matches the `id` column directly.
        let del = format!(
            "DELETE FROM {table}
             WHERE account_id = ?1 AND list_id = ?2 AND (native_id = ?3 OR id = ?3)"
        );
        self.db.with_tx(|tx| {
            for item in &delta.changes {
                let id = id_of(item);
                let json = to_json(item, table)?;
                tx.execute(
                    &upsert,
                    params![account, list, id, native_id(id), json, now],
                )?;
            }
            for native in &delta.deletions {
                tx.execute(&del, params![account, list, native])?;
            }
            tx.execute(
                "INSERT INTO cache_sync_state
                   (account_id, scope, container_id, sync_token, last_refreshed_at, last_error)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL)
                 ON CONFLICT(account_id, scope, container_id) DO UPDATE SET
                   sync_token = excluded.sync_token,
                   last_refreshed_at = excluded.last_refreshed_at,
                   last_error = NULL",
                params![account, scope.as_str(), list, delta.new_token, now],
            )?;
            Ok(())
        })
    }
}

/// Tables wiped by [`CacheStore::prune_account`].
const CACHE_TABLES: &[&str] = &[
    "cache_events",
    "cache_tasks",
    "cache_contacts",
    "cache_calendars",
    "cache_task_lists",
    "cache_contact_lists",
    "cache_sync_state",
];

// ── Free helpers ─────────────────────────────────────────────────────

fn insert_event(
    tx: &Connection,
    account: &str,
    calendar: &str,
    ev: &Event,
    now: &str,
) -> DbResult<()> {
    let json = to_json(ev, "cache_events")?;
    tx.execute(
        "INSERT INTO cache_events
           (account_id, calendar_id, id, native_id, start_utc, end_utc, etag, payload, cached_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(account_id, calendar_id, id) DO UPDATE SET
           native_id = excluded.native_id,
           start_utc = excluded.start_utc,
           end_utc = excluded.end_utc,
           etag = excluded.etag,
           payload = excluded.payload,
           cached_at = excluded.cached_at",
        params![
            account,
            calendar,
            ev.id,
            native_id(&ev.id),
            ts(&ev.start),
            ts(&ev.end),
            ev.etag,
            json,
            now
        ],
    )?;
    Ok(())
}

/// Derive the provider-native resource id from a cal-core id, so a delta
/// sync's deletions (which only carry the native id) can be applied.
///
/// Universal rule: strip a leading one-char `X:` kind prefix (EWS
/// `S:`/`O:`/`E:`/`M:`), then take the substring before the first `|`
/// (CalDAV `href|uid`, EWS `item_id|change_key`). Adapters whose id is
/// already the native resource id (Google, Graph, Vikunja, local) have
/// neither marker, so the id passes through unchanged.
fn native_id(id: &str) -> &str {
    let bytes = id.as_bytes();
    let stripped = if bytes.len() >= 2 && bytes[1] == b':' {
        &id[2..]
    } else {
        id
    };
    match stripped.split_once('|') {
        Some((native, _)) => native,
        None => stripped,
    }
}

/// Stamp last_refreshed + clear last_error for a listing/by-list scope
/// inside an existing transaction. Leaves token/window untouched.
fn mark_refreshed(
    tx: &Connection,
    account: &str,
    scope: SyncScope,
    container: &str,
    now: &str,
) -> DbResult<()> {
    tx.execute(
        "INSERT INTO cache_sync_state
           (account_id, scope, container_id, last_refreshed_at, last_error)
         VALUES (?1, ?2, ?3, ?4, NULL)
         ON CONFLICT(account_id, scope, container_id) DO UPDATE SET
           last_refreshed_at = excluded.last_refreshed_at,
           last_error = NULL",
        params![account, scope.as_str(), container, now],
    )?;
    Ok(())
}

fn rows_to_structs<T, F>(rows: rusqlite::MappedRows<'_, F>, table: &str) -> DbResult<Vec<T>>
where
    T: DeserializeOwned,
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>,
{
    let mut out = Vec::new();
    for row in rows {
        let json = row?;
        let item: T = serde_json::from_str(&json).map_err(|e| {
            DbError::Invariant(format!("cache {table} payload not deserialisable: {e}"))
        })?;
        out.push(item);
    }
    Ok(out)
}

fn to_json<T: Serialize>(value: &T, table: &str) -> DbResult<String> {
    serde_json::to_string(value)
        .map_err(|e| DbError::Invariant(format!("cache {table} payload not serialisable: {e}")))
}

/// Fixed-width RFC3339 (`…Z`, seconds precision) for the indexed
/// start/end/window columns so lexical comparison matches chronological
/// order.
fn ts(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Millisecond-precision stamp for the diagnostic `cached_at` /
/// `last_refreshed_at` columns (never range-compared).
fn now_ts() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn opt_ts(raw: Option<String>) -> rusqlite::Result<Option<DateTime<Utc>>> {
    match raw {
        None => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(&s)
            .map(|d| Some(d.with_timezone(&Utc)))
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            }),
    }
}
