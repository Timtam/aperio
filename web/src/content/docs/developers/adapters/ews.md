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
- **Reading.** A series created without a zone comes back with the start zone
  `Greenwich Standard Time` and the end zone `tzone://Microsoft/Utc`; that end
  zone means no zone. Otherwise the start zone's id reads as the zone of its
  default ("001") row, in tzdata's canonical spelling (`India Standard Time` →
  `Asia/Kolkata`). Exchange keeps one id for a group of cities on one clock, so
  a Vienna series reads back as Berlin. `UTC`, and an id the table does not
  know (custom definitions, registry-only ids), mean no zone; the series
  repeats in UTC.
- **Ids the server knows.** A server refuses a save naming an id it does not
  know (`ErrorTimeZone`, and the whole save fails); Exchange 2019 does not know
  `Sao Tome Standard Time`. The adapter asks each server once
  (`GetServerTimeZones`) and writes only ids it knows. An unknown one goes out
  without a zone and is logged. An update without a zone keeps the series' old
  zone in Exchange, so an update to an unknown zone leaves the old one there.
  If the server cannot be asked, or its answer cannot be read, the CLDR ids are
  written and the next save asks again.
- **Writing.** An all-day series writes no zone (`cal_core::written_series_zone`).
  A zone makes Exchange move an all-day series to that zone's midnights and
  stretch it over more days (live test). Any other series' stored zone goes
  through the core's rule for zones (`series_clock_zone`, then
  `canonical_zone`): no zone, a UTC name or an unknown name writes no zone; any
  spelling of a zone writes that zone's id.
- **Zones Exchange cannot store** are written without a zone: CLDR has no id
  for them (`Antarctica/Troll`), or the id runs another clock in the five
  years after the pinned release (`America/Scoresbysund`, `Antarctica/Casey`,
  `Antarctica/Vostok`). The generator's clock guard finds these; the table
  lists them.
- **Updating CLDR.** Take `windowsZones.xml` and `LICENSE` from one CLDR
  release tag, write that tag, its publication date, both URLs and the XML's
  sha256 into `SOURCE`, run `cargo xtask windows-zones`, commit.
- **Cached events follow the translation.** The events token the host keeps
  is `zt-{translation}:{cookie}`, where the translation is the generated
  `TABLE_ID` (hashed from the table's rows) plus `READ_RULE` in
  `windows_tz.rs`. Bump `READ_RULE` whenever an id becomes a series' zone
  differently without the table changing. A delta whose token names another
  translation emits every cached item again, without re-reading Exchange, so
  no view keeps a zone the old translation read.
- **Cached items follow the parser.** A zone that needs a field older builds
  did not read (the end zone) cannot be re-read from the cache. The folder
  state records `ITEM_PARSER` (`api.rs`); a state from an older parser is
  dropped and the folder drained again from scratch, once. Bump it whenever
  the item parser starts reading a field the read rule uses.

## Testing

`mockito` (or fixture XML) for the SOAP envelopes. Tests cover the
id-only folder-sync probe/drain, the count parsing, and the
folder-complete emit. The zone translation is pinned by
`fixtures/windowsZones.json`, whose rows are named in the test; the adapter
tests ask the mocked server for its zones once and drain an older parser's
state again. Live testing
needs an Exchange/365 mailbox that still exposes EWS.
