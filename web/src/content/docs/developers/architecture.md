---
title: "Architecture"
---

## The three layers

```text
┌──────────────────────────────────────────────────────────────┐
│  Frontend  (React + TypeScript, src/)                        │
│  Accessible views, dialogs, the keyboard model. No provider  │
│  logic, no persistence — it only calls Tauri commands and    │
│  listens for events.                                         │
└───────────────▲───────────────────────────┬──────────────────┘
                │  invoke<T>(cmd, args)      │  emit(event)
                │  (request/response)        │  (host → UI push)
┌───────────────┴───────────────────────────▼──────────────────┐
│  Host  (Rust, src-tauri/, crate `aperio`)                    │
│  • Tauri command handlers (src-tauri/src/commands)           │
│  • SQLite database + migrations (src-tauri/src/db)           │
│  • Snapshot cache for external data (src-tauri/src/cache)    │
│  • Event-log sync engine (src-tauri/src/event_log, /sync)    │
│  • Plugin host: loads adapters over a C ABI                  │
└───────────────▲───────────────────────────┬──────────────────┘
                │  cal_core traits           │  C ABI vtables
                │  (in-process, local store) │  (FFI, plugins)
┌───────────────┴────────────┐  ┌────────────▼──────────────────┐
│  cal-core (crates/cal-core) │  │  Adapters (crates/adapter-│
│  Domain types + feature     │  │  *) — Google, CalDAV, Graph,  │
│  traits (CalendarFeature,   │  │  EWS, Vikunja, Todoist, local │
│  TasksFeature, …)           │  │  …, each behind a plugin.     │
└─────────────────────────────┘  └───────────────────────────────┘
```

## The Cargo workspace

The repository is one Cargo workspace. Key members:

- **`crates/cal-core`** — the shared vocabulary: `Event`, `Task`,
  `Calendar`, `TaskList`, `Contact`, colour types, and the feature traits
  (`Adapter`, `CalendarFeature`, `TasksFeature`, `ContactsFeature`). Every
  adapter implements a subset of these; the host speaks only `cal-core`.
- **`crates/adapter-*`** — one crate per provider (`-local`,
  `-google`, `-caldav`, `-microsoft-graph`, `-ews`, `-vikunja`,
  `-todoist`, `-ical`). Each is a normal Rust library implementing the
  relevant `cal-core` traits.
- **`crates/adapter-*-plugin`** — a thin wrapper that exposes an
  adapter over the C ABI as a loadable plugin (see below).
- **`crates/plugin-core`** — the ABI contract: the `#[repr(C)]` vtable
  layouts, the call/marshalling shim, and the C header.
- **`crates/plugin-sdk`** — helper macros so a plugin author writes safe
  Rust, not raw FFI.
- **`crates/sync-core`** — the sync event log types (`SyncEvent`,
  `EventEnvelope`) shared between host and the sync machinery.
- **`src-tauri`** (crate `aperio`) — the Tauri application/host.

## Frontend ↔ host communication

Two directions, both standard Tauri:

1. **Commands (request/response).** The UI calls
   `invoke<T>('command_name', { args })`; the host runs a
   `#[tauri::command]` handler and returns a serde-serialisable value.
   Wrappers live in `src/api/client.ts`; handlers in
   `src-tauri/src/commands/`.
2. **Events (host → UI push).** The host `emit`s named events (e.g.
   `cache-updated`, `sync-conflicts-changed`); the UI subscribes and
   reacts (refetch, show a dialog). This drives the
   stale-while-revalidate cache refresh and sync status updates.

### The domain types are generated, not mirrored

The shapes both frontends parse are **generated from the Rust types**, not
written twice. `cal-core`, `plugin-core` and `host-core` carry
`#[derive(ts_rs::TS)]` behind an off-by-default `ts-export` feature;
`cargo xtask ts-types` writes `shared/generated/`, and `shared/types.ts`
re-exports from there. Run it after changing any of those types — CI runs
`cargo xtask ts-types --check` and fails, naming the files, when the committed
declarations no longer match the Rust.

The feature is off by default on purpose: cargo unifies features across the
whole graph, so turning it on would pull `ts-rs` into every adapter crate.

### Rules the UI needs during render come from the core, synchronously

Some rules are needed *while React renders*, and `invoke` cannot serve them:
IPC is asynchronous, a render function must return finished UI in one turn,
and an `Array.prototype.sort` comparator cannot await at all. So the desktop
reaches those rules through **WebAssembly** — `crates/cal-core-wasm`, compiled
into the webview and instantiated once at startup (`main.tsx` awaits
`initCoreRules()` before rendering). Mobile reaches the same Rust through a
synchronous Expo `Function`, and a future native frontend links it directly.

`cal-core-wasm` is **pure marshalling**: strings in, values out, every body a
`match`. No rule may live there — it exists so rules can live in the core.

What crosses today:

- `priorityRank`, `isImportantPriority`, `normalPriority`;
- **text ordering**, `cal_core::collation` — `compare_names` for names of
  things (case- and accent-insensitive) and `compare_titles` for text the user
  wrote (digit runs by value: "Kapitel 2" before "Kapitel 10").

**Do not reach for `localeCompare`.** Use `compareNames` / `compareTitles`
from `@aperio/shared` for text a person reads — each surface installs its own
door into the core at startup (`installTextCollation`), bound to the language
the *user* chose rather than the one the operating system reports. For machine
strings (ISO day keys, RFC-3339 instants, ids) use `compareMachineStrings`,
which is a plain codepoint compare and needs no collation at all.

The collation data is a megabyte of baked CLDR, so `cal-core`'s `collation`
feature is off by default for the same reason `ts-export` is: only the two
frontend bindings switch it on.

**A module behind an optional feature needs `#[cfg(feature = "…")]` on both
the `pub mod` and its `pub use`.** `cargo build --workspace` will not tell you
otherwise: cargo unifies features across everything in one build, so a
frontend binding that switches a feature on switches it on for the whole
workspace — while an adapter repository compiling the crate alone fails on the
missing dependency. CI therefore also runs `cargo check -p cal-core`,
`-p plugin-core` and `-p plugin-sdk` on their own.

## The plugin host & the C ABI

Adapters are loaded as **plugins** over a stable C ABI rather than linked
in directly. The host talks to a plugin through a `#[repr(C)]` *vtable* of
function pointers (`Option<VtableMethodFn>`), one table per capability
(calendar, tasks, contacts). Domain data crosses the boundary as serde
JSON, so adding an optional `#[serde(default)]` field is ABI-transparent;
adding a *method* means appending a new slot to the end of the vtable
(never reordering — that preserves binary compatibility).

On the host side, `plugin_core::shim` adapts a loaded vtable back into an
ordinary `cal-core` trait object (`FfiCalendarAdapter`, `FfiTasksAdapter`,
…), so the rest of the host is plugin-agnostic. The full contract lives in
the [Plugin Developer docs](/plugins/abi-reference/).

## Data path: local vs. external

- **Local data** (the built-in `local` adapter) lives in the host's own
  SQLite tables and is the source of truth for user-created calendars,
  events, tasks, contacts and colour labels. It also participates in sync.
- **External data** (Google, iCloud, …) is *cached*, not owned. The host
  keeps a snapshot in `cache_*` tables (see `src-tauri/src/cache`) so the
  app paints instantly on launch, then revalidates in the background
  (stale-while-revalidate). Provider mutations go out through the adapter;
  the provider remains the source of truth.
  - Both the **item** reads (`get_events`/`get_tasks`/`get_contacts`) **and
    the catalog** reads (`list_calendars`/`list_task_lists`/
    `list_contact_lists`) are non-blocking: a cold snapshot is served as the
    (possibly empty) cached rows plus one deduplicated background refresh —
    never an inline `await` on the network. A slow provider therefore can't
    gate first paint. When the refresh lands it registers the
    container→account routes and emits `cache-updated`; `CacheSyncListener`
    then re-runs the affected catalog *and* invalidates the item hooks so
    rows that couldn't be routed on the cold paint fill in.
  - On the frontend, `CalendarStore` exposes a **per-source** loading flag
    (`calendarsLoading`/`taskListsLoading`/`contactListsLoading`), and each
    data hook gates on the one catalog it needs (events → calendars, tasks →
    task lists, contacts → contact lists). A slow calendar enumeration no
    longer holds the task or contacts view's first paint.

All persisted files go through `paths::resolve_data_dir()` so the app
stays portable (a `data/` next to the executable when present, otherwise
the OS data dir) — never hard-code an OS path.

## The sync engine (event log)

Cross-device sync is **multi-writer, peer-to-peer** — there is no
coordinating "primary device". Local mutations are appended to an **event
log** (`sync-core`'s `SyncEvent` variants like `calendar.created`,
`event.updated`, `color_label.deleted`). The log is exchanged through a
sync backend (WebDAV, Dropbox, …, themselves plugins) and an **applier**
(`src-tauri/src/event_log/applier.rs`) merges incoming events into the
local store with field-level, last-writer-wins conflict resolution;
genuine conflicts surface to the user. `DESIGN.md` § 19 is the full spec.
