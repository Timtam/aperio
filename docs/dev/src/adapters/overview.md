# Adapters

An **adapter** turns one provider's API into Aperio's `cal-core` vocabulary.
Each lives in `crates/cal-adapter-<provider>` as a normal Rust library and
implements the `cal-core` feature traits it can support; a matching
`crates/cal-adapter-<provider>-plugin` exposes it over the plugin ABI.

## The capability model

A provider implements only the traits it can. Capabilities are declared in
the plugin's `plugin.json` and surfaced to the UI so it can hide
affordances a backend can't fulfil.

| Trait | Provides |
|---|---|
| `Adapter` | base: `authenticate`, `capabilities` |
| `CalendarFeature` | calendars + events |
| `TasksFeature` | task lists + tasks (+ optional sections, assignees, membership) |
| `ContactsFeature` | address books + contacts |

A "tasks-only" provider like Todoist or Vikunja declares only `["tasks"]`;
a full provider like Google or Microsoft Graph declares calendars, tasks
and contacts.

## How the host reads data: full vs. delta vs. range

Adapters differ in how they enumerate changes, which the host must respect
when caching:

- **Delta / sync-token** providers return *changes since a token*
  (Google `syncToken`, Graph `delta`, CalDAV `sync-collection`, EWS
  `SyncFolderItems`). A `410 Gone`/invalid token means "do a full resync".
- **Folder-complete** snapshots return the *entire* set (EWS events after
  the folder-sync rework, CalDAV/iCloud events via PROPFIND-enumeration +
  `sync-collection`, iCal feeds). These set `ChangeSet.complete = true` so
  the host stores an unbounded cache window and serves every later view
  range straight from the snapshot.
- **Range-scoped** reads return only what overlaps a time window
  (Google/Graph event reads, plus the legacy CalDAV ctag fallback for
  servers without `sync-collection`). The host keeps a bounded cache window
  for these and re-fetches when the view moves outside it.

> **Reads never block the first paint.** `get_events`/`get_tasks`/
> `get_contacts` serve whatever snapshot exists *right now* and run the
> refresh in the background (stale-while-revalidate). When it lands the host
> emits `cache-updated` and the view re-reads. A slow cold sync — e.g. a
> first iCloud full fetch — therefore fills in progressively instead of
> freezing startup for 20 s+.

> **Recurring events.** Most adapters return the recurring **master** with
> its `RRULE`; the frontend expands occurrences for the visible range via
> `rrule.js`. Adapters must therefore pass a master through even when its
> *first* occurrence falls outside the requested window (it may still
> recur into it). Microsoft Graph is the exception — it uses
> `/calendarView`, which expands occurrences server-side.

## The adapters

| Adapter | Crate | Capabilities | Protocol |
|---|---|---|---|
| [Local store](local.md) | `cal-adapter-local` | calendars, tasks, contacts | host SQLite (source of truth) |
| [CalDAV / iCloud](caldav.md) | `cal-adapter-caldav` | calendars, tasks, contacts | CalDAV / CardDAV |
| [Google](google.md) | `cal-adapter-google` | calendars, tasks, contacts | Google REST APIs (OAuth2) |
| [Microsoft Graph](microsoft.md) | `cal-adapter-microsoft-graph` | calendars, tasks, contacts | Microsoft Graph (OAuth2) |
| [Exchange (EWS)](ews.md) | `cal-adapter-ews` | calendars, tasks, contacts | Exchange Web Services (SOAP) |
| [Vikunja](vikunja.md) | `cal-adapter-vikunja` | tasks | Vikunja REST API |
| [Todoist](todoist.md) | `cal-adapter-todoist` | tasks | Todoist REST v2 (+ Sync API) |

There is also an iCal/ICS subscription adapter (`cal-adapter-ical`,
read-only calendar feeds).

Each page below covers the protocol, the authentication flow, provider
quirks worth knowing, and how to test the adapter.
