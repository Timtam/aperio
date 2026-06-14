---
title: "Local store"
---

**Crate:** `cal-adapter-local` · **Capabilities:** calendars, tasks, contacts

The local adapter is special: it is the **source of truth** for
user-created data, not a cache of someone else's API. It reads and writes
the host's own SQLite database directly.

## Storage

Local containers and items live in the main database (`calendars`,
`events`, `task_lists`, `tasks`, `contact_lists`, `contacts`,
`color_labels`, …; schema in `src-tauri/src/db/sql/*.sql`). The adapter is
constructed with a shared connection handle and exposes both the
`cal-core` trait methods *and* inherent helpers for the things only a local
store can do (e.g. `create_calendar`, `add_event_exdate`).

## Sync participation

Unlike external adapters, the local store **syncs** across devices through
the event log. Two complementary paths:

- **Mutations emit events.** The host command performs the local write and
  appends a `SyncEvent` (e.g. `CalendarCreated`, `TaskListUpdated`) whose
  payload is the serialised entity.
- **The applier writes back.** Incoming events are applied via
  `sync_apply.rs`'s `upsert_*_from_sync` helpers, and a full-dataset
  snapshot is produced/consumed via `sync_snapshot.rs`.

Because the payloads are serde-serialised domain types, adding a
`#[serde(default)]` field rides along automatically; the field-level
merge in the applier treats it like any other column.

## Quirks worth knowing

- **Range reads must keep recurring masters.** The event range query is a
  half-open interval overlap *plus* a clause that keeps any recurring
  master whose series begins before the range end, so a weekly meeting
  created long ago still expands into the current view.
- **Test schema.** `cal_adapter_local::test_support::open_test_db()` builds
  an in-memory database by replaying the migration SQL. When a migration
  adds a column the adapter reads, add the new `SCHEMA_V<n>` const there.

## Testing

Pure SQLite, no network. Tests open an in-memory DB via `test_support`,
exercise create/read/update/delete, and assert round-trips (including the
recurring-master and EXDATE behaviour).
