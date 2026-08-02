---
title: "CalDAV / iCloud"
---

**Crate:** `adapter-caldav` · **Capabilities:** calendars, tasks, contacts

CalDAV/CardDAV is the open standard behind Apple iCloud and many
self-hosted servers (Nextcloud, Radicale, …). One adapter serves them all;
iCloud is just CalDAV with Apple's endpoints.

## Protocol

- **Discovery + listing:** `PROPFIND` to enumerate collections (calendars,
  task lists via `VTODO`, address books) and their properties
  (`displayname`, `calendar-color`, `getctag`, `sync-token`).
- **Incremental sync:** `REPORT` with `sync-collection` returns the
  resources changed since a `sync-token`, plus per-resource deletions. An
  invalid token triggers a full re-bootstrap.
- **Bootstrap / bulk read:** a depth-1 `PROPFIND` lists every resource href,
  then `calendar-multiget` / `addressbook-multiget` fetches their bodies in
  **chunks** (so a large iCloud calendar doesn't time out on one giant
  request).
- **Folder-complete caching:** because the bootstrap already enumerated the
  whole collection, the event sync multigets **all** dates (not just the
  view window) and marks the change set `complete`, so the host caches an
  unbounded window. Later views are served from cache and only a background
  `sync-collection` delta touches the network. Servers without
  `sync-collection` fall back to a windowed, range-scoped read.
- **Bodies** are iCalendar (`VEVENT`/`VTODO`) and vCard, parsed in
  `mapping.rs` into `cal-core` types. `RRULE`/`EXDATE` are carried through;
  occurrences are expanded on the frontend.

## Authentication

HTTP Basic over TLS with a username + password (for iCloud, an
**app-specific password**, not the Apple ID password). There is no OAuth.
The server base URL is user-supplied for self-hosted servers; iCloud uses
Apple's well-known endpoints.

## Quirks

- **Both homes are best-effort.** Discovery probes `calendar-home-set`
  AND `addressbook-home-set` independently and fails only when **neither**
  is found. A CalDAV-only server (no address books) and a CardDAV-only
  server (e.g. Synology Contacts — advertises an `addressbook-home-set`
  but no `calendar-home-set`) both work; the missing side's listings just
  come back empty. Well-known resolution tries `/.well-known/caldav` then
  `/.well-known/carddav`. (Before this, a contacts-only server failed
  account creation with a "not found" error because the calendar home was
  mandatory.)
- **Contacts read via multiget, never inline PROPFIND.** `get_contacts`
  does a Depth-1 PROPFIND for hrefs then an `addressbook-multiget` for the
  bodies — it does **not** ask for inline `<CR:address-data/>` in a plain
  PROPFIND. That shortcut is non-standard; iCloud and Synology Contacts
  silently return resources with no body, so the old inline read yielded
  zero contacts while persisting a sync token, leaving address books
  permanently empty (a one-time `cache.contactsMultigetHealV2` heal clears
  those poisoned tokens so books re-bootstrap).
- **Stable ids.** A resource is keyed by `{href}|{uid}` so renames/moves
  and per-resource deletions resolve correctly.
- **Subtasks ride `RELATED-TO`.** A VTODO's parameter-less `RELATED-TO`
  (RELTYPE defaults to PARENT; CHILD/SIBLING entries are ignored) carries
  the parent's **bare UID** on the wire; the read path resolves it to the
  composite `{href}|{uid}` task id against the fetched set. Because the
  `icalendar` crate keeps only one `RELATED-TO` per component when
  parsing, the link is scanned from the **raw iCal text** — several
  `RELATED-TO` lines (RFC-legal; e.g. jtx Board's reciprocal
  `RELTYPE=CHILD` entries) would otherwise drop the parent
  order-dependently. An incremental delta whose parent didn't itself
  change falls back to one tolerant `uid → id` listing; if that read
  fails the delta fails too (token not advanced) rather than caching a
  falsified flat parent. Writes strip the composite id back to the UID;
  removing the parent just regenerates the VTODO without the property.
- **Keep recurring masters.** The folder-complete sync keeps every event
  regardless of date; the legacy windowed fallback still keeps any event
  with a recurrence even when its first occurrence is outside the window
  (`event_in_window` returns true for `recurrence.is_some()`).
- **iCloud date sentinels / colours.** Colours arrive as `#RRGGBBAA`; the
  alpha is dropped to a plain hex.
- **`getctag` fast-path.** When the collection's ctag is unchanged, the
  adapter can skip a full enumeration.

## Testing

`mockito` serves canned `PROPFIND`/`REPORT`/multiget XML. Tests assert the
sync-collection token round-trip, per-resource deletions, the
`{href}|{uid}` id scheme, and the iCalendar → `cal-core` mapping. For a
live smoke test, an iCloud account with an app-specific password works.
