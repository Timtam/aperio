---
title: "Exchange (EWS)"
---

**Crate:** `adapter-ews` · **Capabilities:** calendars, tasks, contacts

Exchange Web Services is the older SOAP/XML API for on-premises Exchange and
older Microsoft 365 tenants — used where Graph isn't available.

## Protocol

SOAP over HTTPS. Requests are XML envelopes (`soap.rs` builds them,
`mapping.rs` parses the responses):

- **Autodiscover:** the endpoint can be discovered from an email address.
- **Sync:** `SyncFolderItems` returns changes for a folder with a sync
  state token. The adapter first does an **id-only probe** to learn the
  change counts cheaply, then fetches item bodies with `GetItem`.

## Authentication

Basic auth (username/password) over TLS, or NTLM depending on the server.
The endpoint is discovered or user-supplied.

## Quirks

- **Folder-complete events.** EWS keeps a per-folder in-memory view of
  *every* item it has seen, so its event read is **folder-complete**: it
  emits the full set with `ChangeSet.complete = true`, and the host stores
  an unbounded cache window. This is what fixed a class of "event in a new
  month doesn't appear" bugs — the sync cookie is folder-wide, so a
  range-filtered emit would miss unchanged items in newly-viewed ranges.
- **Recurring masters always pass** the folder filter; recurrence shapes
  are enriched via `GetItem`.
- **ChangeKey churn.** An edited item keeps its item id but rotates the
  `ChangeKey` embedded in the composite id, so the cache purges the whole
  native group before re-inserting (avoids stale duplicates).

## Time zones

EWS names a recurring series' zone by a **Windows id** (`W. Europe Standard
Time`); the rest of Aperio uses tzdata names. The translation lives in
`windows_tz.rs`, over a table generated from the Unicode CLDR
`windowsZones.xml`:

- **The data is pinned.** `crates/adapter-ews/cldr/` holds the XML, its
  Unicode License V3 and `SOURCE` (release tag, publication date, URLs,
  sha256). `cargo xtask windows-zones` generates
  `src/windows_tz/windows_zones.rs` from it; CI runs it with `--check`.
- **Reading.** An id reads as the zone of its default ("001") row, in tzdata's
  canonical spelling (`India Standard Time` → `Asia/Kolkata`). Exchange keeps
  one id per clock, so a Vienna series reads back as Berlin. `UTC`, and an id
  the table does not know (custom definitions, registry-only ids), mean no
  zone; the series repeats in UTC.
- **Writing.** A stored zone first goes through the core's rule for zones
  (`series_clock_zone`, then `canonical_zone`): no zone, a UTC name or an
  unknown name writes no zone; any spelling of a zone writes that zone's id.
- **Zones Exchange cannot store** are written without a zone: CLDR has no id
  for them (`Antarctica/Troll`), or the id runs another clock in the five
  years after the pinned release (`America/Scoresbysund`, `Antarctica/Casey`,
  `Antarctica/Vostok`). The generator's clock guard finds these; the table
  lists them.
- **Updating CLDR.** Replace `windowsZones.xml`, `LICENSE` and `SOURCE` from
  one CLDR release tag, run `cargo xtask windows-zones`, commit. A new table
  changes `TABLE_ID`, and the next delta sync emits every cached item again,
  so no view keeps a zone the old table read.

## Testing

`mockito` (or fixture XML) for the SOAP envelopes. Tests cover the
id-only folder-sync probe/drain, the count parsing, and the
folder-complete emit. The zone translation is pinned by
`fixtures/windowsZones.json`, whose rows are named in the test. Live testing
needs an Exchange/365 mailbox that still exposes EWS.
