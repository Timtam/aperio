# Handoff: CACHE-8 part 2 — EWS host-driven delta (`get_events_delta`)

> Temporary session-handoff note. Delete once CACHE-8 is finished.
> You are on branch `feat/external-adapter-cache`. All prior work is committed.

## Goal

Implement `CalendarFeature::get_events_delta` for the **EWS** adapter so the
central snapshot cache refreshes EWS calendars incrementally (true
`ChangeSet`), instead of the full-fetch fallback. Wire it through the EWS
plugin FFI. (Tasks/contacts delta for EWS are optional follow-ups — do
events first.)

## What's already done (part 1, committed)

- **Delta trait surface (CACHE-4, commit a8c9d9d):**
  `cal_core::CalendarFeature::get_events_delta(&self, calendar_id, range, since_token: Option<&str>) -> Result<ChangeSet<Event>>`,
  default `Unsupported`. `ChangeSet<T> { changes: Vec<T>, deletions: Vec<String>, new_token: Option<String>, full_resync: bool }`.
  The host's `src-tauri/src/commands/cache_swr.rs::refresh_events` already
  calls `get_events_delta` first and falls back to a full `get_events` on
  `Unsupported`. So once EWS implements it (+ plugin slot), the host uses it
  automatically — no host changes needed.
- **Cache native-id foundation (CACHE-8 part 1, commit 6b8f707):**
  `cache_events` has a `native_id` column; `apply_events_delta` DELETEs by
  `native_id`. The host derives `native_id` universally:
  strip a leading one-char `X:` prefix, then take the substring before the
  first `|`. For EWS, `native_id("S:item|ck") == "item"` (the raw ItemId).
  **So your delta's `deletions` MUST be raw EWS ItemIds** (what
  `SyncChange::Delete` already gives you) — they’ll match the cached rows.
- **Reference implementation:** the CalDAV delta (CACHE-7, commit 8ab92fd):
  - adapter methods in `crates/cal-adapter-caldav/src/lib.rs`
  - plugin ffi + vtable slots in `crates/cal-adapter-caldav-plugin/src/lib.rs`
    (`ffi_get_events_delta` + `GetEventsDeltaArgs { calendar_id, range, since_token: Option<String> }` + `get_events_delta: Some(ffi_get_events_delta)` in `CALENDAR_VTABLE`).
  Mirror this shape for EWS.

## EWS-specific design (the hard part — read before coding)

Key files: `crates/cal-adapter-ews/src/api.rs`, `crates/cal-adapter-ews/src/lib.rs`,
`crates/cal-adapter-ews/src/mapping.rs`, plugin `crates/cal-adapter-ews-plugin/src/lib.rs`.

1. **EWS already does SyncFolderItems delta internally.**
   - `api::sync_events_to_completion(client, calendar_id, state) -> SyncedFolderState`
     (api.rs ~186) loops SyncFolderItems, applying `SyncChange::{Create, Update, Delete}`
     into `state.items: HashMap<ItemId, ParsedItem>` and advancing
     `state.sync_state: Option<String>` (the server cookie). It does NOT
     currently return which ids changed/were deleted (only tracing counts).
   - `lib.rs::refresh_and_read_events` (~378) is the caller: loads prior
     state from the `events_sync: Mutex<HashMap<calendar_id, SyncedFolderState>>`
     map, runs the drain, persists via `persist_events_sync` (→ `events_sync.json`
     when `with_state_dir` was set), then translates `state.items` → `Vec<Event>`.
   - The trait `get_events` (lib.rs ~581) calls `refresh_and_read_events`.

2. **Translation is one Event per item, PLUS synthetic override events.**
   In `refresh_and_read_events` (~410-470): each `ParsedItem` → one
   `to_event(...)` (master carries RRULE; the frontend expands). For each
   **modified occurrence** on a master it ALSO emits a synthetic standalone
   event at the moved time. **Check what `id`/`native_id` those synthetic
   events get** (look at the override-emit block). If their cal-core id is
   derived from the master's ItemId, the `native_id` deletion fan-out
   removes them when the master is deleted (good). If they carry their own
   ItemId, deleting the master won't remove them — handle explicitly (emit
   their native ids in `deletions`, or key them off the master).

3. **The cookie / token decision (avoid double persistence).**
   The host passes `since_token` and stores `new_token` per calendar in
   `cache_sync_state.sync_token`. EWS also persists its own cookie in
   `events_sync.json`. Pick ONE source of truth:
   - **Recommended:** make the host token authoritative. `get_events_delta`
     seeds `SyncedFolderState { sync_state: since_token, items: <empty or
     reloaded> }`, drains, returns `new_token = state.sync_state`. But the
     drain needs `state.items` for the deletion-removal + enrichment, so you
     still need the item set. Either keep `events_sync` in-memory as the
     working set (don't treat its cookie as authoritative — use the host's),
     or accept the internal map as a memo and just keep both cookies equal.
   - Simplest correct first cut: keep using the internal `events_sync` map
     as today, but ADD per-drain change/deletion collection, and return the
     internal cookie as `new_token`. `full_resync = cold_start` (no prior
     cookie). The host stores the cookie; next call passes it back; you can
     either trust the host token or your internal one (keep them in sync).

4. **Suggested implementation shape:**
   - In `api.rs`, add `sync_events_delta(client, calendar_id, state) ->
     EwsResult<(SyncedFolderState, Vec<String> /*changed item_ids*/, Vec<String> /*deleted item_ids*/)>`
     — a copy of `sync_events_to_completion` that also records, per drain,
     the ids touched by Create/Update (changed) and Delete (deleted), then
     runs `enrich_item_details` on the changed ones.
   - In `lib.rs`, `get_events_delta`:
     - load prior state (or seed from `since_token`),
     - call `sync_events_delta`,
     - translate the **changed** items → `Vec<Event>` (reuse the same
       per-item translation as `refresh_and_read_events`, incl. synthetic
       overrides for changed masters),
     - `deletions` = deleted ItemIds (these are native ids — they match
       `native_id` in the cache),
     - `new_token` = `state.sync_state`, `full_resync = cold_start`,
     - persist the state (as `refresh_and_read_events` does).
   - Watch the **range filter**: `get_events` filters singles by range;
     for the delta, decide whether to range-filter changes (the host’s
     cached window covers it; filtering avoids caching out-of-window
     singles — match existing `get_events` behaviour).

5. **Plugin FFI (mirror CalDAV plugin commit 8ab92fd):**
   In `crates/cal-adapter-ews-plugin/src/lib.rs` add `GetEventsDeltaArgs {
   calendar_id, range, since_token: Option<String> }`, `ffi_get_events_delta`
   (decode args → `dispatch(h, |p| p.get_events_delta(&args.calendar_id,
   args.range, args.since_token.as_deref()))`), and
   `get_events_delta: Some(ffi_get_events_delta)` in `CALENDAR_VTABLE`.

## Tests

- EWS adapter test with mockito: mock SyncFolderItems responses — a cold
  drain (only Creates) → `full_resync=true`, changes populated, token set;
  a warm drain with the prior token → one Update + one Delete → `changes`
  has the updated event, `deletions` has the deleted ItemId, `full_resync=false`.
  (See `crates/cal-adapter-ews/src/api.rs` + `tasks.rs` tests for the
  mockito `Server` + SOAP-body patterns.)
- Host side is already covered (`apply_events_delta` deletes by native_id —
  `delta_deletes_by_native_id` in `src-tauri/src/cache/tests.rs`).

## Verify before committing

```
cargo build --workspace
cargo test -p cal-adapter-ews
cargo clippy -p cal-adapter-ews -p cal-adapter-ews-plugin
cargo fmt --all -- --check
cargo test -p aperio cache::          # host cache still green
```

## Repo conventions (from memory)

- Dev-text (commits, comments, docs) in **English**; UI strings via i18n.
- Persisted files go through `paths::resolve_data_dir()`.
- Repo is mixed LF/CRLF; edit via the normal tools (they preserve EOL) —
  don't bulk-rewrite files in text mode.
- End commit messages with the `Co-Authored-By: Claude Opus 4.8 (1M context)` trailer.
- Don't merge to `main` / push unless asked.

## Branch state

`feat/external-adapter-cache` — latest commits (newest first):
`6b8f707` native_id foundation · `8ab92fd` CalDAV CTag delta · `333da32` C ABI
vtable header · `a8c9d9d` delta trait surface · `3f6228d` CacheRefresher ·
`accf64e` snapshot cache 0..2 · `4c2c2de` CalDAV+EWS list CRUD.
