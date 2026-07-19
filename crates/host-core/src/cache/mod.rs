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

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::Serialize;

use cal_core::{Calendar, Contact, ContactList, DateRange, Event, Section, Task, TaskList};
use tracing::debug;

use crate::db::{DbError, DbHandle, DbResult};

mod observer;
mod refresh;
mod search;
mod swr;

#[cfg(test)]
mod tests;

/// Bumped whenever the adapter event-mapping changes in a way that requires
/// RE-FETCHING already-cached external events — i.e. when the same provider data
/// would now map to a different `Event`. The first such bump is the
/// recurrence-timezone fix: existing cached payloads lack `recurrence.tzid`, and
/// a normal delta sync doesn't re-fetch unchanged events, so the fix would never
/// reach them. [`reconcile_cache_generation`] re-bootstraps every external
/// account once when this device's recorded generation is older.
pub const CACHE_GENERATION: u32 = 1;

/// `user_prefs` key holding the cache generation last applied on this device.
pub const CACHE_GENERATION_KEY: &str = "cache.generation";

/// One-time, idempotent cache-generation reconcile, run at startup by both the
/// desktop and mobile hosts right after the cache is opened. When this device
/// hasn't yet applied [`CACHE_GENERATION`], clear every EXTERNAL account's sync
/// state (token + window + freshness) so the next refresh re-bootstraps and
/// re-maps its events with the current adapter code — exactly what the per-account
/// "Re-sync from scratch" action does, but automatic on upgrade. Cached rows
/// survive as an offline fallback until the cold fetch replaces them.
///
/// Local + DeviceCalendar accounts own their data and have no provider to
/// re-fetch from (`plugin_id() == None`), so they're skipped. The new generation
/// is recorded only after every reset succeeds, so a mid-run failure simply
/// retries on the next start. Returns the number of sync-state rows reset (0 when
/// already up to date). Best-effort: callers log and continue on `Err`.
pub fn reconcile_cache_generation(
    cache: &CacheStore,
    accounts: &[crate::accounts::Account],
    prefs: &crate::user_prefs::UserPrefsRepo,
) -> Result<usize, String> {
    let applied = prefs
        .get(CACHE_GENERATION_KEY)
        .map_err(|e| e.to_string())?
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    if applied >= CACHE_GENERATION {
        return Ok(0);
    }
    let mut reset = 0;
    for acc in accounts {
        if acc.adapter_kind.plugin_id().is_none() {
            continue; // Local / DeviceCalendar — nothing to re-fetch.
        }
        reset += cache
            .reset_account_sync(&acc.id)
            .map_err(|e| e.to_string())?;
    }
    prefs
        .set(CACHE_GENERATION_KEY, &CACHE_GENERATION.to_string())
        .map_err(|e| e.to_string())?;
    Ok(reset)
}

pub use observer::{CacheObserver, CacheRefreshStatus};
pub use refresh::{
    CacheRefresher, PREF_CACHE_LAST_REFRESHED_AT, PREF_CACHE_REFRESH_INTERVAL_MINUTES,
};
pub use swr::{
    event_self_warm_needed, has_snapshot, is_stale, refresh_contacts, refresh_events,
    refresh_sections, refresh_tasks, spawn_item_refresh, spawn_refresh, SWR_TTL_SECS,
};

/// The "unbounded" snapshot window recorded for folder-complete containers
/// (their cache holds the WHOLE collection, so any view range is covered).
///
/// It must NOT use `DateTime::<Utc>::MIN_UTC`/`MAX_UTC`: those years
/// (−262143 / +262142) format to non-4-digit RFC 3339 strings
/// (`-262143-…`, `+262142-…`) that `parse_from_rfc3339` rejects, so the
/// window would fail to round-trip through the cache's text timestamps —
/// `get_sync_state` would error, `covers` would never be true, and the read
/// would fall into the cold path forever (a refresh → `cache-updated` →
/// re-read → refresh loop). Year 1 … 9999 spans every realistic event and
/// is valid RFC 3339.
pub fn unbounded_window() -> DateRange {
    DateRange::new(
        Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0)
            .single()
            .expect("year 1"),
        Utc.with_ymd_and_hms(9999, 12, 31, 23, 59, 59)
            .single()
            .expect("year 9999"),
    )
}

/// Host-owned snapshot cache over the main SQLite database.
#[derive(Clone)]
pub struct CacheStore {
    db: DbHandle,
    /// In-memory per-(account, scope, container) refresh generation.
    /// [`Self::invalidate`] (and any local mutation that invalidates a container)
    /// bumps it; a background refresh captures it BEFORE its slow fetch and drops
    /// its write if it changed — so a warm pass whose fetch predates a local
    /// mutation can't overwrite that mutation by writing stale provider data back
    /// over the invalidation. In-memory (reset on restart) — the race is within a
    /// session. Keyed by `scope.as_str()` so the `Copy` enum needn't be hashed.
    /// `Arc`-shared so `CacheStore` clones (which share the DB) also share the
    /// counter — an invalidate on one handle is seen by a refresh on another.
    refresh_generations: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<(String, &'static str, String), u64>>,
    >,
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
    /// The sections (Vikunja buckets / Todoist sections) of one task list —
    /// per-container like `Tasks`, keyed by the same `list_id`.
    Sections,
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
            SyncScope::Sections => "sections",
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

/// One container whose most recent background refresh FAILED — the raw
/// material of the per-account error surface (a container that silently
/// serves stale cached data, e.g. after an iCloud app-password revoke,
/// is invisible to the user without this). `last_error` is written by
/// `mark_error` on every failed refresh and cleared by every successful
/// write, so presence == "the latest attempt failed".
#[derive(Debug, Clone, Serialize)]
pub struct ContainerRefreshError {
    /// The [`SyncScope`] wire string ("events", "tasks", "calendars", …).
    pub scope: String,
    /// Container id, or `""` for an account-level listing failure.
    pub container_id: String,
    /// Human-readable container name resolved from the cached listing,
    /// when the listing has the container. `None` for listing-scope
    /// failures and containers the listing doesn't (yet) know.
    pub container_name: Option<String>,
    /// The recorded provider error text.
    pub error: String,
    /// Last SUCCESSFUL refresh (RFC 3339) — how stale the data the user
    /// currently sees is. `None`: never refreshed successfully.
    pub last_success_at: Option<String>,
}

/// Every failing container of one account, plus whether any error looks
/// authentication-shaped (drives the "re-enter password" hint).
#[derive(Debug, Clone, Serialize)]
pub struct AccountRefreshErrors {
    pub account_id: String,
    pub auth_suspected: bool,
    pub errors: Vec<ContainerRefreshError>,
}

/// Heuristic: does a provider error text look like an AUTH failure (as
/// opposed to a network blip)? Substring match over the usual suspects —
/// conservative on purpose: a false "auth" only makes the UI suggest
/// re-checking the password. The OAuth needles matter because a revoked
/// Google/Graph grant surfaces as the TOKEN endpoint's HTTP 400 body
/// (`{"error":"invalid_grant",...}`) embedded in a protocol error, not
/// as a 401 — exactly the case where re-authenticating is the fix.
pub fn is_auth_shaped(error: &str) -> bool {
    let lower = error.to_lowercase();
    [
        "401",
        "403",
        "unauthorized",
        "unauthorised",
        "forbidden",
        "authentication",
        "invalid credentials",
        "password",
        "invalid_grant",
        "invalid_client",
        "expired or revoked",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
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
        Self {
            db,
            refresh_generations: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    /// Current refresh generation for a container (0 when never invalidated). A
    /// background refresh captures this before its fetch; if it differs at
    /// write-time a local mutation invalidated the container mid-fetch, so the
    /// fetched snapshot is stale and the write must be dropped.
    pub fn refresh_generation(&self, account: &str, scope: SyncScope, container: &str) -> u64 {
        *self
            .refresh_generations
            .lock()
            .expect("cache refresh-generation poisoned")
            .get(&(account.to_string(), scope.as_str(), container.to_string()))
            .unwrap_or(&0)
    }

    /// Bump a container's refresh generation. Called from [`Self::invalidate`] so
    /// any in-flight background refresh of that container drops its (now stale)
    /// write rather than clobbering the invalidation.
    fn bump_refresh_generation(&self, account: &str, scope: SyncScope, container: &str) {
        *self
            .refresh_generations
            .lock()
            .expect("cache refresh-generation poisoned")
            .entry((account.to_string(), scope.as_str(), container.to_string()))
            .or_insert(0) += 1;
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
        let events: Vec<Event> = self.db.with_read_conn(|c| {
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
        })?;
        // Diagnostic: how many suppressing `::rid::` cancelled overrides came back
        // for this calendar+range. A deleted recurring occurrence only stays hidden
        // if its override is here; if this logs 0 overrides while the occurrence
        // still ghosts, the override was dropped upstream (sync/cache write), not in
        // the frontend. Only logs when the result actually holds cancelled rows.
        let cancelled = events.iter().filter(|e| e.cancelled).count();
        if cancelled > 0 {
            let rid_overrides = events.iter().filter(|e| e.id.contains("::rid::")).count();
            debug!(
                calendar,
                total = events.len(),
                cancelled,
                rid_overrides,
                "read_events cache result"
            );
        }
        Ok(events)
    }

    /// Cached rows for `calendar` whose provider-native id is in
    /// `natives` — used by the full-resync path to PRESERVE resources
    /// the adapter reported as `unfetched` (enumerated but unservable,
    /// e.g. CalDAV multiget skips): their rows would otherwise be
    /// dropped by the wholesale replace even though nothing suggests
    /// they were deleted.
    pub fn read_events_by_native(
        &self,
        account: &str,
        calendar: &str,
        natives: &[String],
    ) -> DbResult<Vec<Event>> {
        let mut out = Vec::new();
        self.db.with_read_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT payload FROM cache_events
                 WHERE account_id = ?1 AND calendar_id = ?2 AND native_id = ?3",
            )?;
            for native in natives {
                let rows = stmt.query_map(params![account, calendar, native], |r| {
                    r.get::<_, String>(0)
                })?;
                out.extend(rows_to_structs::<Event, _>(rows, "cache_events")?);
            }
            Ok::<(), DbError>(())
        })?;
        Ok(out)
    }

    /// Every account's currently-failing containers (rows whose latest
    /// refresh attempt recorded `last_error`), grouped per account with
    /// container names resolved from the cached listings. Powers the
    /// per-account error surface on both platforms. Cheap: errors are
    /// rare, the scan is one indexed SELECT plus a name lookup per hit.
    pub fn refresh_errors(&self) -> DbResult<Vec<AccountRefreshErrors>> {
        struct Row {
            account: String,
            scope: String,
            container: String,
            error: String,
            last_success: Option<String>,
        }
        let rows: Vec<Row> = self.db.with_read_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT account_id, scope, container_id, last_error, last_refreshed_at
                 FROM cache_sync_state
                 WHERE last_error IS NOT NULL
                 ORDER BY account_id, scope, container_id",
            )?;
            let mapped = stmt.query_map([], |r| {
                Ok(Row {
                    account: r.get(0)?,
                    scope: r.get(1)?,
                    container: r.get(2)?,
                    error: r.get(3)?,
                    last_success: r.get(4)?,
                })
            })?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()
        })?;

        // Resolve container identity from the cached listings. `None`
        // means the row is ORPHANED: the listing authoritatively does
        // not contain this container (deleted server-side; the
        // sync-state row was re-created by a refresh against a stale
        // persisted selection) — surfacing it would be a permanent
        // unnamed warning the user can never clear. Authority means
        // either the listing has OTHER containers, or it is empty but
        // has succeeded at least once (last_refreshed_at set — its last
        // success returned the empty set). A cold listing keeps the row.
        // The name prefers the user's rename override — the name every
        // other surface shows — over the raw listing payload. Every
        // lookup degrades FAIL-OPEN (keep the row, name unknown): a
        // transient read error must dim the surface, never blank it,
        // and orphaning needs positive confirmation.
        let resolve = |scope: &str, account: &str, container: &str| -> Option<Option<String>> {
            if container.is_empty() {
                return Some(None);
            }
            let (table, kind, listing_scope) = match scope {
                "events" => ("cache_calendars", "calendar", "calendars"),
                "tasks" | "sections" => ("cache_task_lists", "task_list", "task_lists"),
                "contacts" => ("cache_contact_lists", "contact_list", "contact_lists"),
                _ => return Some(None),
            };
            self.db.with_read_conn(|c| {
                let payload: Option<String> = match c
                    .query_row(
                        &format!("SELECT payload FROM {table} WHERE account_id = ?1 AND id = ?2"),
                        params![account, container],
                        |r| r.get(0),
                    )
                    .optional()
                {
                    Ok(p) => p,
                    Err(_) => return Some(None),
                };
                if payload.is_none() {
                    let listed: i64 = match c.query_row(
                        &format!("SELECT COUNT(*) FROM {table} WHERE account_id = ?1"),
                        params![account],
                        |r| r.get(0),
                    ) {
                        Ok(n) => n,
                        Err(_) => return Some(None),
                    };
                    if listed > 0 {
                        return None;
                    }
                    let listing_succeeded = c
                        .query_row(
                            "SELECT 1 FROM cache_sync_state
                             WHERE account_id = ?1 AND scope = ?2 AND container_id = ''
                               AND last_refreshed_at IS NOT NULL",
                            params![account, listing_scope],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()
                        .ok()
                        .flatten()
                        .is_some();
                    if listing_succeeded {
                        return None;
                    }
                }
                let override_name: Option<String> = c
                    .query_row(
                        "SELECT name FROM container_name_overrides
                         WHERE container_id = ?1 AND kind = ?2",
                        params![container, kind],
                        |r| r.get(0),
                    )
                    .optional()
                    .ok()
                    .flatten();
                let name = override_name.or_else(|| {
                    payload
                        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                        .and_then(|v| v.get("name").and_then(|n| n.as_str().map(str::to_string)))
                });
                Some(name)
            })
        };

        let mut out: Vec<AccountRefreshErrors> = Vec::new();
        for row in rows {
            let Some(container_name) = resolve(&row.scope, &row.account, &row.container) else {
                continue;
            };
            let entry = ContainerRefreshError {
                container_name,
                scope: row.scope,
                container_id: row.container,
                error: row.error,
                last_success_at: row.last_success,
            };
            match out.last_mut() {
                Some(acc) if acc.account_id == row.account => {
                    acc.auth_suspected |= is_auth_shaped(&entry.error);
                    acc.errors.push(entry);
                }
                _ => out.push(AccountRefreshErrors {
                    account_id: row.account,
                    auth_suspected: is_auth_shaped(&entry.error),
                    errors: vec![entry],
                }),
            }
        }
        Ok(out)
    }

    /// Full-refresh write: replace the entire cached set for `calendar`
    /// with `events` and record `range` as the covered window. Keeps any
    /// existing delta token (a full fetch doesn't invalidate it).
    ///
    /// Returns whether the cached CONTENT actually changed. A refresh that
    /// re-fetches the identical set skips the row churn entirely (only the
    /// window/freshness bookkeeping is stamped) and reports `false`, so
    /// callers can suppress the `cache-updated` notification — a no-op
    /// warm pass must not trigger frontend reload waves.
    pub fn replace_calendar_events(
        &self,
        account: &str,
        calendar: &str,
        range: DateRange,
        events: &[Event],
    ) -> DbResult<bool> {
        let now = now_ts();
        let (ws, we) = (ts(&range.start), ts(&range.end));
        let mut incoming = HashMap::with_capacity(events.len());
        for ev in events {
            incoming.insert(ev.id.as_str(), to_json(ev, "cache_events")?);
        }
        self.db.with_tx(|tx| {
            let unchanged = rows_match(
                tx,
                "SELECT id, payload FROM cache_events
                 WHERE account_id = ?1 AND calendar_id = ?2",
                params![account, calendar],
                &incoming,
            )?;
            // Even when the content is byte-identical, REWRITE the rows if
            // this container has no recorded freshness — that is the state a
            // cache-generation reset (or "Re-sync from scratch") leaves
            // behind, and those resets exist precisely to recompute what
            // insert_event derives from the payload (start_utc/end_utc via
            // the recurrence-reach logic). Skipping the write there would
            // freeze the OLD derived columns forever for rows whose payload
            // the provider still serves unchanged. The `unchanged` VALUE
            // (what the UI is told) still reflects content equality.
            let force_rewrite = unchanged && !has_freshness(tx, account, "events", calendar)?;
            if !unchanged || force_rewrite {
                tx.execute(
                    "DELETE FROM cache_events WHERE account_id = ?1 AND calendar_id = ?2",
                    params![account, calendar],
                )?;
                for ev in events {
                    insert_event(tx, account, calendar, ev, &now)?;
                }
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
            Ok(!unchanged)
        })
    }

    /// Range-scoped refresh write for adapters WITHOUT delta support
    /// (device EventKit, iCal): replace only the cached rows overlapping
    /// `range` with `events`, leaving rows outside untouched, and record
    /// the window as the UNION of the existing window and `range` when
    /// they overlap or touch (a disjoint fetch records just `range` — a
    /// union across a gap would claim coverage of dates never fetched).
    ///
    /// This is what lets a view-sized refresh on a no-delta adapter stay
    /// view-sized: a full replace would clobber the warm −3…+12-month
    /// cache down to the view, and always fetching wide instead costs a
    /// 15-month provider expansion on every stale read. Out-of-range
    /// provider-side deletions are reconciled by the warm pass's wide
    /// fetch. Returns whether cached content changed (see
    /// [`Self::replace_calendar_events`]).
    pub fn replace_calendar_events_in_range(
        &self,
        account: &str,
        calendar: &str,
        range: DateRange,
        events: &[Event],
    ) -> DbResult<bool> {
        let now = now_ts();
        let (rs, re) = (ts(&range.start), ts(&range.end));
        let mut incoming = HashMap::with_capacity(events.len());
        for ev in events {
            incoming.insert(ev.id.as_str(), to_json(ev, "cache_events")?);
        }
        self.db.with_tx(|tx| {
            // Compare against the rows the same half-open overlap query the
            // reader uses would return for `range`.
            let unchanged = rows_match(
                tx,
                "SELECT id, payload FROM cache_events
                 WHERE account_id = ?1 AND calendar_id = ?2
                   AND start_utc < ?3 AND end_utc > ?4",
                params![account, calendar, re, rs],
                &incoming,
            )?;
            let force_rewrite = unchanged && !has_freshness(tx, account, "events", calendar)?;
            if !unchanged || force_rewrite {
                tx.execute(
                    "DELETE FROM cache_events
                     WHERE account_id = ?1 AND calendar_id = ?2
                       AND start_utc < ?3 AND end_utc > ?4",
                    params![account, calendar, re, rs],
                )?;
                for ev in events {
                    insert_event(tx, account, calendar, ev, &now)?;
                }
            }
            // Window union (or plain `range` when there is no existing /
            // an entirely disjoint window).
            let existing: Option<(Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT window_start, window_end FROM cache_sync_state
                     WHERE account_id = ?1 AND scope = 'events' AND container_id = ?2",
                    params![account, calendar],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (ws, we) = match existing {
                Some((Some(ews), Some(ewe))) if ews <= re && ewe >= rs => {
                    (ews.min(rs.clone()), ewe.max(re.clone()))
                }
                _ => (rs.clone(), re.clone()),
            };
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
            Ok(!unchanged)
        })
    }

    /// Incremental write: upsert `delta.changes`, remove
    /// `delta.deletions`, persist the new token. Window is untouched.
    ///
    /// Returns whether cached content changed: `false` for the routine
    /// empty delta (token advanced, nothing else — the dominant no-op a
    /// steady-state warm pass produces) and for deletions that matched no
    /// cached row; the token/freshness bookkeeping is stamped either way.
    pub fn apply_events_delta(
        &self,
        account: &str,
        calendar: &str,
        delta: &Delta<Event>,
    ) -> DbResult<bool> {
        let now = now_ts();
        self.db.with_tx(|tx| {
            let mut changed = !delta.changes.is_empty();
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
                let deleted = tx.execute(
                    "DELETE FROM cache_events
                     WHERE account_id = ?1 AND calendar_id = ?2 AND native_id = ?3",
                    params![account, calendar, native],
                )?;
                changed |= deleted > 0;
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
            Ok(changed)
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

    pub fn replace_list_tasks(&self, account: &str, list: &str, tasks: &[Task]) -> DbResult<bool> {
        self.replace_by_list("cache_tasks", SyncScope::Tasks, account, list, tasks, |t| {
            t.id.as_str()
        })
    }

    pub fn apply_tasks_delta(
        &self,
        account: &str,
        list: &str,
        delta: &Delta<Task>,
    ) -> DbResult<bool> {
        self.apply_by_list_delta("cache_tasks", SyncScope::Tasks, account, list, delta, |t| {
            t.id.as_str()
        })
    }

    // ── Sections ─────────────────────────────────────────────────────
    // A task list's sections are per-container like its tasks, but have no
    // provider delta path (`TasksFeature::list_sections` returns the full
    // set), so only the full-replace + read are cached — no delta apply.

    pub fn read_sections(&self, account: &str, list: &str) -> DbResult<Vec<Section>> {
        self.read_by_list("cache_sections", account, list)
    }

    pub fn replace_sections(
        &self,
        account: &str,
        list: &str,
        sections: &[Section],
    ) -> DbResult<bool> {
        self.replace_by_list(
            "cache_sections",
            SyncScope::Sections,
            account,
            list,
            sections,
            |s| s.id.as_str(),
        )
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
    ) -> DbResult<bool> {
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
    ) -> DbResult<bool> {
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

    pub fn replace_calendars(&self, account: &str, calendars: &[Calendar]) -> DbResult<bool> {
        self.replace_listing(
            "cache_calendars",
            SyncScope::Calendars,
            account,
            calendars,
            |c| c.id.as_str(),
            &[("cache_events", "calendar_id")],
        )
    }

    pub fn read_task_lists(&self, account: &str) -> DbResult<Vec<TaskList>> {
        self.read_listing("cache_task_lists", account)
    }

    pub fn replace_task_lists(&self, account: &str, lists: &[TaskList]) -> DbResult<bool> {
        self.replace_listing(
            "cache_task_lists",
            SyncScope::TaskLists,
            account,
            lists,
            |l| l.id.as_str(),
            &[("cache_tasks", "list_id"), ("cache_sections", "list_id")],
        )
    }

    pub fn read_contact_lists(&self, account: &str) -> DbResult<Vec<ContactList>> {
        self.read_listing("cache_contact_lists", account)
    }

    pub fn replace_contact_lists(&self, account: &str, lists: &[ContactList]) -> DbResult<bool> {
        self.replace_listing(
            "cache_contact_lists",
            SyncScope::ContactLists,
            account,
            lists,
            |l| l.id.as_str(),
            &[("cache_contacts", "list_id")],
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

    /// Write-through for a successful EXTERNAL task UPDATE: replace the
    /// provider's returned row in the snapshot, then mark the list stale.
    ///
    /// Invalidate-only left the RETAINED pre-write rows as what the SWR
    /// cold fallback served, so a check-off stayed visibly open until a
    /// background refresh landed — which on the device-reminders adapter
    /// lags long enough to read as "nothing happened".
    ///
    /// USE ONLY FOR UPDATES, not creates. The row must already exist in the
    /// snapshot under its READ id, because we key the replace on
    /// [`native_id`] (the stable underlying identity — the CalDAV `href`,
    /// the EWS `ItemId`), NOT the full composite id. That matters for
    /// adapters that ROTATE the composite id on write: EWS rotates the
    /// ChangeKey suffix on every edit, so a plain upsert would leave the
    /// pre-edit `item|ckA` row alongside the fresh `item|ckB` and show the
    /// task twice. Purging the native group first collapses them. For
    /// stable-id adapters (device reminders, Vikunja, Google, Todoist,
    /// Graph) native_id == id, so this is exactly the row a plain upsert
    /// would have replaced.
    ///
    /// Creates deliberately do NOT go through here: a created row's id may
    /// not match the id the READ path later assigns the same task (CalDAV's
    /// create returns the bare uid, reads return `{href}|{uid}` — different
    /// native ids), so upserting it would plant a row the next delta can't
    /// reconcile, producing a persistent duplicate. Creates stay
    /// invalidate-only (the new task surfaces on the next refresh, as before
    /// this whole write-through change).
    ///
    /// Skipped entirely when the list holds NO cached rows: a never-warmed
    /// list live-reads on the cold fallback anyway, and a lone row would
    /// masquerade as the whole list. (For a real update the list is always
    /// warm — the task being edited was read from it.)
    pub fn write_through_task(&self, account: &str, list: &str, task: &Task) -> DbResult<()> {
        let has_rows = self
            .read_tasks(account, list)
            .map(|rows| !rows.is_empty())
            .unwrap_or(false);
        if has_rows {
            let native = native_id(&task.id);
            self.db.with_conn(|c| {
                c.execute(
                    "DELETE FROM cache_tasks
                      WHERE account_id = ?1 AND list_id = ?2 AND native_id = ?3",
                    params![account, list, native],
                )?;
                Ok::<_, DbError>(())
            })?;
            self.upsert_task(account, list, task)?;
        }
        self.invalidate(account, SyncScope::Tasks, list)
    }

    /// Delete-side twin of [`Self::write_through_task`]: drop the row so the
    /// retained snapshot can't resurrect it, then mark the list stale.
    /// Callers pass the task's READ id (the composite the UI holds), which is
    /// exactly the snapshot row's id, so the keyed-on-`id` delete matches.
    pub fn write_through_task_removal(&self, account: &str, list: &str, id: &str) -> DbResult<()> {
        self.remove_task(account, list, id)?;
        self.invalidate(account, SyncScope::Tasks, list)
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
        self.db.with_read_conn(|c| {
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
        // Bump the generation first so any refresh whose fetch is already in
        // flight sees the change and drops its now-stale write (rather than
        // re-freshening the cache over the invalidation we're about to make).
        self.bump_refresh_generation(account, scope, container);
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

    /// Drop the delta cursor + window + freshness for EVERY event container
    /// owned by `account`, forcing the next refresh of each to do a full
    /// resync (the cached rows stay as an offline fallback). Unlike
    /// [`Self::invalidate`] this needs no container id — it fans out across
    /// the account's whole event scope. Used for the one-time EWS cursor
    /// heal: an older build let the reminder scan advance the provider cursor
    /// independently of the host's, so a delta could skip changes the host
    /// never cached; clearing the cursor makes the next warm pass re-pull the
    /// whole folder and recover them. Returns the number of containers reset.
    pub fn reset_event_sync(&self, account: &str) -> DbResult<usize> {
        self.db.with_conn(|c| {
            let n = c.execute(
                "UPDATE cache_sync_state
                    SET sync_token = NULL, window_start = NULL, window_end = NULL,
                        last_refreshed_at = NULL
                  WHERE account_id = ?1 AND scope = 'events'",
                params![account],
            )?;
            Ok(n)
        })
    }

    /// Same as [`Self::reset_event_sync`], but for the CONTACTS scope.
    /// Clears the delta token + freshness for every address book owned
    /// by `account`, forcing the next read of each to re-bootstrap (the
    /// cached rows stay as an offline fallback). Used by the one-time
    /// contacts heal: an older CardDAV read fetched zero contacts via a
    /// non-standard inline-`address-data` PROPFIND yet still persisted a
    /// sync token, so every subsequent delta reported "no changes" over
    /// an empty cache — clearing the token re-bootstraps with the
    /// multiget read and recovers the contacts. Returns the number of
    /// containers reset.
    pub fn reset_contacts_sync(&self, account: &str) -> DbResult<usize> {
        self.db.with_conn(|c| {
            let n = c.execute(
                "UPDATE cache_sync_state
                    SET sync_token = NULL, window_start = NULL, window_end = NULL,
                        last_refreshed_at = NULL
                  WHERE account_id = ?1 AND scope = 'contacts'",
                params![account],
            )?;
            Ok(n)
        })
    }

    /// Force a FULL cold re-sync of EVERY container owned by `account` —
    /// across all scopes (events, tasks, contacts + their listings): clear the
    /// delta token, the covered window and the freshness markers, so the next
    /// refresh of each re-bootstraps from the provider. The cached rows stay as
    /// an offline fallback until the cold fetch replaces them; credentials are
    /// untouched. Unlike [`Self::reset_event_sync`] (events-only) this fans out
    /// across every scope, powering the user-facing "force full re-sync"
    /// recovery action — e.g. a CalDAV bootstrap that enumerated an INCOMPLETE
    /// resource set yet persisted a sync-token, so later deltas reported "no
    /// changes" over permanently-missing events. Returns the number of
    /// containers reset.
    pub fn reset_account_sync(&self, account: &str) -> DbResult<usize> {
        self.db.with_conn(|c| {
            let n = c.execute(
                "UPDATE cache_sync_state
                    SET sync_token = NULL, window_start = NULL, window_end = NULL,
                        last_refreshed_at = NULL
                  WHERE account_id = ?1",
                params![account],
            )?;
            Ok(n)
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

    /// `children` = the per-container item tables hanging off this listing
    /// (`(table, container-fk-column)`). A container DROPPED from a
    /// successfully fetched listing is an authoritative removal, so its
    /// item rows (and sync-state rows) are pruned in the same transaction —
    /// otherwise a deleted calendar's cached events would keep being served
    /// to any code still holding the id (e.g. a persisted selection).
    fn replace_listing<T: Serialize>(
        &self,
        table: &str,
        scope: SyncScope,
        account: &str,
        items: &[T],
        id_of: impl Fn(&T) -> &str,
        children: &[(&str, &str)],
    ) -> DbResult<bool> {
        let now = now_ts();
        let sel = format!("SELECT id, payload FROM {table} WHERE account_id = ?1");
        let sel_ids = format!("SELECT id FROM {table} WHERE account_id = ?1");
        let del = format!("DELETE FROM {table} WHERE account_id = ?1");
        let ins = format!(
            "INSERT INTO {table} (account_id, id, payload, cached_at) VALUES (?1, ?2, ?3, ?4)"
        );
        let mut incoming = HashMap::with_capacity(items.len());
        for item in items {
            incoming.insert(id_of(item), to_json(item, table)?);
        }
        self.db.with_tx(|tx| {
            let unchanged = rows_match(tx, &sel, params![account], &incoming)?;
            if !unchanged {
                let dropped: Vec<String> = {
                    let mut stmt = tx.prepare(&sel_ids)?;
                    let rows = stmt.query_map(params![account], |r| r.get::<_, String>(0))?;
                    rows.filter_map(|r| r.ok())
                        .filter(|id| !incoming.contains_key(id.as_str()))
                        .collect()
                };
                tx.execute(&del, params![account])?;
                for item in items {
                    let json = to_json(item, table)?;
                    tx.execute(&ins, params![account, id_of(item), json, now])?;
                }
                for id in &dropped {
                    for (child, fk) in children {
                        tx.execute(
                            &format!("DELETE FROM {child} WHERE account_id = ?1 AND {fk} = ?2"),
                            params![account, id],
                        )?;
                    }
                    tx.execute(
                        "DELETE FROM cache_sync_state
                         WHERE account_id = ?1 AND container_id = ?2",
                        params![account, id],
                    )?;
                }
            }
            mark_refreshed(tx, account, scope, "", &now)?;
            Ok(!unchanged)
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
        self.db.with_read_conn(|c| {
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
    ) -> DbResult<bool> {
        let now = now_ts();
        let sel = format!("SELECT id, payload FROM {table} WHERE account_id = ?1 AND list_id = ?2");
        let del = format!("DELETE FROM {table} WHERE account_id = ?1 AND list_id = ?2");
        let ins = format!(
            "INSERT INTO {table} (account_id, list_id, id, native_id, payload, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        );
        let mut incoming = HashMap::with_capacity(items.len());
        for item in items {
            incoming.insert(id_of(item), to_json(item, table)?);
        }
        self.db.with_tx(|tx| {
            let unchanged = rows_match(tx, &sel, params![account, list], &incoming)?;
            if !unchanged {
                tx.execute(&del, params![account, list])?;
                for item in items {
                    let id = id_of(item);
                    let json = to_json(item, table)?;
                    tx.execute(&ins, params![account, list, id, native_id(id), json, now])?;
                }
            }
            mark_refreshed(tx, account, scope, list, &now)?;
            Ok(!unchanged)
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
    ) -> DbResult<bool> {
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
            let mut changed = !delta.changes.is_empty();
            for item in &delta.changes {
                let id = id_of(item);
                let json = to_json(item, table)?;
                tx.execute(
                    &upsert,
                    params![account, list, id, native_id(id), json, now],
                )?;
            }
            for native in &delta.deletions {
                let deleted = tx.execute(&del, params![account, list, native])?;
                changed |= deleted > 0;
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
            Ok(changed)
        })
    }
}

/// Tables wiped by [`CacheStore::prune_account`].
const CACHE_TABLES: &[&str] = &[
    "cache_events",
    "cache_tasks",
    "cache_sections",
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
            ts(&range_end_utc(ev)),
            ev.etag,
            json,
            now
        ],
    )?;
    Ok(())
}

/// The "latest time this event matters for a range query" — what goes in
/// the `end_utc` column the half-open overlap query in [`read_events`]
/// (and the window prune) test against.
///
/// For a one-off event that's simply its end. For a RECURRING master it's
/// the recurrence's *reach*: the parsed `UNTIL`, or a far-future sentinel
/// for open-ended / `COUNT`-based series. Without this, the column would
/// hold the master's FIRST occurrence end, so a weekly meeting that began
/// last year (`start`/`end` in the past) would be filtered out of this
/// month's view even though it recurs into it — the occurrences are
/// expanded on the frontend, which never sees the master. The `payload`
/// still stores the true event, so display is unaffected; only row
/// selection changes.
fn range_end_utc(ev: &Event) -> DateTime<Utc> {
    let Some(recurrence) = &ev.recurrence else {
        return ev.end;
    };
    recurrence_until(&recurrence.rrule)
        .unwrap_or_else(far_future_utc)
        // Guard against a degenerate `UNTIL` before the first occurrence.
        .max(ev.end)
}

/// Parse the `UNTIL=…` bound out of an RRULE string, if present. RRULE is
/// a `;`-separated list of `KEY=VALUE`; the value is an iCalendar date or
/// date-time (`YYYYMMDD`, `YYYYMMDDTHHMMSS`, or the UTC `…Z` form).
/// Returns `None` when there's no `UNTIL` (open-ended / `COUNT`-based) or
/// the value can't be parsed — the caller then keeps the series alive
/// with the far-future sentinel.
fn recurrence_until(rrule: &str) -> Option<DateTime<Utc>> {
    let value = rrule.split(';').find_map(|part| {
        let (key, val) = part.split_once('=')?;
        key.trim().eq_ignore_ascii_case("UNTIL").then(|| val.trim())
    })?;
    // UTC date-time (the common form): 20270101T100000Z
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ") {
        return Some(Utc.from_utc_datetime(&dt));
    }
    // Floating date-time without a zone — treat as UTC.
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S") {
        return Some(Utc.from_utc_datetime(&dt));
    }
    // Date-only UNTIL — include the whole day.
    if let Ok(d) = NaiveDate::parse_from_str(value, "%Y%m%d") {
        return Some(Utc.from_utc_datetime(&d.and_hms_opt(23, 59, 59)?));
    }
    None
}

/// Sentinel "end" for open-ended recurrences. Year 9999 keeps the stored
/// timestamp 4-digit so it stays lexicographically ordered against real
/// dates — the cache compares the RFC-3339 strings directly in SQL.
fn far_future_utc() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(9999, 12, 31, 23, 59, 59).unwrap()
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

/// Whether a container has a recorded freshness stamp. `false` right
/// after a cache-generation reset / "Re-sync from scratch" (those NULL
/// the whole sync-state row) — the replace writes use this to force a
/// row rewrite even for byte-identical content, so payload-derived
/// columns are recomputed with current code (see
/// [`CacheStore::replace_calendar_events`]).
fn has_freshness(tx: &Connection, account: &str, scope: &str, container: &str) -> DbResult<bool> {
    let stamp: Option<Option<String>> = tx
        .query_row(
            "SELECT last_refreshed_at FROM cache_sync_state
             WHERE account_id = ?1 AND scope = ?2 AND container_id = ?3",
            params![account, scope, container],
            |r| r.get(0),
        )
        .optional()?;
    Ok(matches!(stamp, Some(Some(_))))
}

/// Whether a container's cached rows are exactly the incoming
/// (id → payload) set — the change-detection behind the `replace_*`
/// writes. Compares the serialized payload byte-for-byte: identical
/// content produced by the same serializer matches, anything else
/// (including a payload written by an older schema) conservatively
/// counts as changed and gets rewritten.
fn rows_match(
    tx: &Connection,
    select: &str,
    scope_params: &[&dyn rusqlite::ToSql],
    incoming: &HashMap<&str, String>,
) -> DbResult<bool> {
    let mut stmt = tx.prepare(select)?;
    let mut rows = stmt.query(scope_params)?;
    let mut matched = 0usize;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let payload: String = row.get(1)?;
        match incoming.get(id.as_str()) {
            Some(want) if *want == payload => matched += 1,
            _ => return Ok(false),
        }
    }
    Ok(matched == incoming.len())
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
        match serde_json::from_str::<T>(&json) {
            Ok(item) => out.push(item),
            // One undeserialisable payload (schema drift, corruption) must
            // not blank the WHOLE container — failing the vector here made a
            // single bad row render its calendar/list as empty on every read
            // until a background refresh happened to rewrite it. Skip the row
            // loudly; the rest of the container stays served.
            Err(e) => {
                tracing::warn!(
                    target: "aperio::cache",
                    table,
                    error = %e,
                    "skipping undeserialisable cache payload row",
                );
            }
        }
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
