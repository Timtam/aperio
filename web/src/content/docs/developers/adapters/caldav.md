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
- **A changed occurrence lives in its series' resource.** The server keeps it
  as a second VEVENT beside the master, with the same UID and a
  `RECURRENCE-ID` naming its slot. A PUT replaces the whole resource, so any
  write to one occurrence reads the resource first and puts the rest back:
  - Updating an override swaps its VEVENT for the rewritten one and puts
    everything else back byte for byte. The `RECURRENCE-ID` line is copied
    exactly as the server wrote it, because it has to match the master's
    `DTSTART` in kind and zone. If the resource cannot be read block by
    block, or the slot holds no override any more, nothing is written.
  - Skipping an occurrence (`add_event_exdate`, and `delete_event` with an
    override id) adds the `EXDATE` to the master and drops the override in
    that slot. The master is rebuilt from core fields, as for any master
    write, so properties Aperio does not model are lost there. The other
    overrides go back byte for byte, and so does every `VTIMEZONE` they name.
    A resource that holds overrides without their master (an invitation to
    single occurrences) loses the one block, and the resource itself goes
    with its last block.
- **Keep recurring masters.** The folder-complete sync keeps every event
  regardless of date; the legacy windowed fallback still keeps any event
  with a recurrence even when its first occurrence is outside the window
  (`event_in_window` returns true for `recurrence.is_some()`).
- **iCloud date sentinels / colours.** Colours arrive as `#RRGGBBAA`; the
  alpha is dropped to a plain hex.
- **`getctag` fast-path.** When the collection's ctag is unchanged, the
  adapter can skip a full enumeration.
- **The account is every href of its `calendar-user-address-set`.** A server
  names a calendar user by whichever href it likes. iCloud writes a principal
  path with the address in an `EMAIL` parameter, for ORGANIZER and for the
  account's own `ROLE=CHAIR` row (live measurement M1):
  `ORGANIZER;CN=…;EMAIL=toni@example.org:/aB1/principal/`. Discovery keeps
  every href (`identity::OwnIdentity`), and the account organizes an event
  whose ORGANIZER any of them names, each compared by its own rules (a
  `mailto:` by address in any case, a path resolved against the principal
  apart from one trailing slash, a `urn:` case-insensitively), or whose
  `EMAIL` equals one of its addresses. Any other ORGANIZER, or any ORGANIZER
  on a server that reports no address, makes the event `organized_elsewhere`;
  an event without ORGANIZER is the account's own. The read shows a
  non-mail calendar user by its `EMAIL`, and drops the row that names the
  same user as ORGANIZER from the invitees. When the probe for the addresses
  fails on a network or server error, it is repeated before the next read
  that needs it, and while it keeps failing that read fails: the host keeps
  its cache instead of storing a guess that a delta would never revisit.
- **A write carries a meeting's lines; it never rebuilds them.** On an RFC
  6638 server the organizer's copy is the invitation: the server mails the
  attendees whenever it changes, and a PUT without ORGANIZER is a "remove"
  that cancels the meeting for everyone (§3.2.3.1). Every update therefore
  reads the resource first and fails if it cannot (`scheduling::plan_block`):
  ORGANIZER, ATTENDEE, SEQUENCE and STATUS go back as the server wrote them,
  folding and parameter order included. Only a change to the invitees
  changes rows: the ones that stay verbatim, the removed ones dropped (the
  server cancels for them), new ones generated
  (`ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE`), and all of them
  gone when the host confirmed every invitee removed (`clear_attendees`,
  decision 74a). A guest change reaches the series' overrides too. On
  someone else's meeting a change to the invitees is refused. A master
  update puts every override of the resource back byte for byte; skipping
  an occurrence inserts one raw `EXDATE` line and leaves the rest of the
  resource untouched. A save that changes nothing the server stores (only a
  colour kept on this device or a reminder's sound, say) is not sent, nor is
  skipping an occurrence the series skips already, because every PUT of a
  meeting mails its guests. A 403 on a PUT or DELETE (a save, a delete, an
  answer to an invitation) is reported as the server's refusal (`Forbidden`,
  with the `DAV:error` precondition), not as a login problem.
- **Such a server always notifies.** Calendars on a scheduling server carry
  `always_notifies_attendees` (and `notifier_name` "iCloud" on iCloud): the
  editors show "iCloud informs the attendees of every change" instead of the
  checkbox, and the delete dialogs say who informs them instead of offering
  to remove silently (decisions 76a, 80a). A new event names its invitees,
  with the account as ORGANIZER, only when the user notifies; otherwise it is
  a plain appointment and the event returned names nobody.

## Testing

`mockito` serves canned `PROPFIND`/`REPORT`/multiget XML. Tests assert the
sync-collection token round-trip, per-resource deletions, the
`{href}|{uid}` id scheme, and the iCalendar → `cal-core` mapping. For a
live smoke test, an iCloud account with an app-specific password works.
