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
- **An exception carries its own content.** A changed occurrence is an item of
  its own on the server, with its own subject, body, location and reminder. The
  read used to build its row from the SERIES and fetch the exception's item
  only to keep one boolean from it (`cancelled`), so the occurrence appeared
  under the series' subject and a save wrote that subject back (decision 58a,
  measured in live round 5). The per-occurrence `GetItem` now keeps the item
  (`ModifiedOccurrence::own`) and `mapping::override_event` builds the row from
  it through the same `to_event` every other row goes through; only what the
  SERIES owns — the calendar, the colour, the slot in the pattern — still comes
  from the master. An item that cannot be read, or that answers for another
  slot, leaves the row inheriting the series' content and says so in the log:
  an inherited value is wrong, a guessed one would be worse.
- **An all-day exception names its slot by its day.** EWS reports an
  all-day item's instants, an occurrence's `OriginalStart` among them, as
  midnight in the item's own zone — mostly: after a write by Aperio before
  PR 8a (UTC midnights, no zone) Exchange kept the old zone's midnights and
  labelled them UTC (live round 3). The read samples the day 13:45 into it,
  in the zone Exchange names for that boundary (`all_day_zone` for the
  start and the slots, `all_day_end_zone` for the end, decisions 217 and
  233): an id the CLDR table maps, an id that is tzdata's own name
  (`Europe/Berlin`, 236), or a zone Exchange defines in full (a custom one
  such as `Customized Time Zone`, read by `zone_definition`, 232 and 239);
  UTC for an item made without a zone. That is exact for a midnight the zone
  names, and the intended day for a label off by any offset in
  (−10:15, +13:45], New Zealand's summer included. It re-anchors the start, the series'
  exceptions and the override id's slot to this device's local midnight of
  that day (`all_day_anchor`, `override_slot`, decision 215). Where
  Exchange names no zone the adapter can read, the sample is taken in UTC
  with the same window. An exception's own copy, which
  an update compares with, is read in its series' zone like its row. With
  the raw instant, a device
  more than twelve hours from the mailbox's zone read the neighbouring day: the
  views hid it, and the reminders, which honour single changes since decision
  214, silenced it. Writing finds the exception by either spelling
  (`names_override`), so an id minted before still resolves. These slots are
  this device's local midnights, so the events token names the device's zone
  as well as the zone translation: a device that moved reads the folder again,
  and its ids follow (decision 216). Deleting one day of an all-day series
  reads the server's slot in the series' zone before it compares, so a device
  far from the mailbox's zone finds the day it names, and a neighbour, a whole
  day away, never comes within the tolerance. Without a zone it compares the
  raw instants as before and aborts rather than trust a sampled day.
- **Exceptions keep their rule field.** Editing one changed occurrence writes
  to the exception's own item, which the override id finds from the series
  head on every write. That update never sends `DeleteItemField
  calendar:Recurrence`: Exchange refuses it on an exception
  (`ErrorInvalidPropertyDelete`) and fails the whole update. The returned event
  keeps the override id.
- **An exception cannot pass its neighbours.** Exchange refuses to move an
  exception onto or past a neighbouring occurrence of its series
  (`ErrorOccurrenceCrossingBoundary`; Outlook has the same rule). The adapter
  then detaches it the way Aperio moves any unchanged occurrence on its own:
  it creates a single at the new time and deletes the exception's item
  without a cancellation. The event that comes back is the new single. If the
  delete fails, the move stands and the failure is logged; a duplicate may
  remain.
- **A range's dates carry a zone.** Exchange appends a zone to a series'
  `StartDate`. Measured is `Z`, both times `2026-10-19Z` on an all-day
  series: once on one Aperio created without a zone (live round 2), and once
  on an Outlook one whose start zone Aperio's update had set to Greenwich
  while its end zone stayed W. Europe (round 3). Both sit at offset 0, so `Z`
  cannot tell the request's UTC context from the series' own zone.
  A series in W. Europe, read without a `TimeZoneContext`, carries an offset
  on both dates instead: `2026-11-04+01:00` (the 8a live test, 2026-10-10).
  So `Z` does not come on every series. The reader keeps only the date
  (`range_date`), both when it parses `EndDate` and when it turns a cached
  range into `UNTIL`. Before READ_RULE 3 it cut only `T` and `+`, so
  `UNTIL=20261105ZT235959Z` came out and the expanders refused the rule: the
  series showed and reminded only at its master's start, its first slot,
  even where that occurrence had been changed or deleted. Changed
  occurrences are rows of their own and still appeared. The
  adapter's own InstanceIndex lookup (`nominal_occurrence_index`) read the
  same rule, so skipping one occurrence of such a series, as a group carry
  does, failed with "could not compute the InstanceIndex"; it reads a fresh
  GetItem, so the parse side alone mends it. EWS tasks
  share the reader, but their cached rows are not re-emitted (see TODO).
- **Occurrences are found by their slot.** Skipping one occurrence
  (`add_event_exdate`) probes the series' `InstanceIndex`es with `GetItem`.
  It matches an exception by its `OriginalStart`, the slot it fills, not by
  its `Start`, which moves when the exception is moved.
- **The organizer is listed as an attendee.** Exchange puts the organizer
  into `RequiredAttendees` with `ResponseType` "Organizer"; for an appointment
  made in Outlook it is the only row. The adapter drops that row by its flag
  (`cal_core::attendee::people_from_read`), so it also goes when
  `<t:Organizer>` names the organizer by an Exchange-internal (EX) address.
  `MyResponseType` "Organizer" says the mailbox organizes the item; any other
  answer makes the event `organized_elsewhere`, even without an organizer
  address, and only the organizer notifies. "Unknown", or no
  `MyResponseType`, is no answer: then an item with an organizer is
  `organized_elsewhere` and one without is the mailbox's own. An update whose invitees did not change
  (`keep_attendees`) sends no `calendar:RequiredAttendees`, so Exchange keeps
  its own list, the organizer's row included. One that removed every invitee
  (`clear_attendees`) deletes `RequiredAttendees` and `OptionalAttendees`,
  and with notifying on, Exchange is asked to send the removed a
  cancellation (not yet measured live). Without this an appointment
  from Outlook failed to save with `ErrorInvalidRecipients`: Aperio asked to
  notify, and the only recipient was the sender.

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
  repeats in UTC. An all-day boundary reads in more zones than a series'
  clock does: an id that is tzdata's own name and a full definition count
  there (see the all-day quirk above).
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
- **An update writes the zone first** (decisions 240, 241). A new zone keeps
  the item's stored wall clock and relabels it: Start and End written before
  it moved by the offset (round 1, B2), and so did an update that sent the
  zone without them — a single created in Aperio, Monday 00:30 in Berlin,
  made weekly landed on Sundays at 23:30 (the 8a live test). So the zone
  goes first, when the series' zone, its all-day flag or its slot changes.
  Where it names another clock than the stored one (compared as the read
  side maps an id, both boundaries, so `W. Europe Standard Time` and a stored
  `Europe/Berlin` are one clock, and `Romance Standard Time`, Paris, is
  another, though it shows the same time today), Start, End and
  IsAllDayEvent follow it even where they did not change; a slot written
  only for that is the server's,
  so a boundary another device moved is not put back (106). The rule comes
  last, on the new zone's day, and only where its built form changes (241):
  a zone change writes no rule while the series' first day and weekday stay
  the same on the new clock, and rewrites it where the switch moves them, as
  near midnight. That covers a single made a series (the zone-first live
  test, L1, L1b) and a zone switch on a stored series, which keeps the
  instant (L3a, L3b;
  no editor offers one yet, DESIGN stages 11 and 12). L4a to L4e are the
  counter-checks.
- **An all-day series given a time** (decision 244) writes IsAllDayEvent,
  Start and End before the zone, Start and End again after it, and the rule
  last, always. On a daily series the zone first is refused whole
  (`ErrorOccurrenceTimeSpanTooBig`: on the new zone's midnights each day
  is two days long, L2); a weekly one took it (round 2, B4), and gets the
  same order all the same. Without the rule, the series lands a day late:
  Exchange reads the range's StartDate again from the all-day day it stored
  in UTC (M1, M2). With it, it lands right (M7, M8), and so does Aperio's
  own request, at 10:00 and at 00:30 (N1, N1b), also on an all-day series
  Outlook stored in W. Europe, where the zone stays (O1 daily, O2 weekly).
- **A save that would drop a series' exceptions asks first** (decisions
  243-253). Exchange drops every changed and deleted occurrence of a series
  when an update writes its Start and End (the zone-first live test: L3a, L3b,
  M3, M6) or its pattern — its frequency, the days or the interval it repeats
  on (the days and the interval: round 6, P1, P2). Its range — the COUNT (M5),
  an end date instead, no end, an earlier end (round 8, U1-U3) —, a title (M4)
  and the same zone written again (N3) keep them. The update builder plans
  what it rewrites (`UpdatePlan::rewrite`: slot, zone, pattern) and where a
  deleted occurrence stands afterwards (`Placement`): moved as the first
  occurrence moved, so it keeps its place in the pattern, which is how
  Exchange numbers occurrences (a move, a zone switch, a rule shifted with its
  start by `cal_core::shift_series`, with or without a week start that changes
  no day, as the editor leaves it out; R1-R3); at its own instant under a new
  pattern; or nowhere it can tell, where the pattern and the slot change
  together, where `cal_core::shift_series` would not shift the rule, or where
  a clock it needs cannot be read. The write path counts from the copy just
  read what would be lost: the changed occurrences, and the deleted ones it
  cannot place. Without the user's consent (`Event::accepts_exception_loss`)
  it sends nothing and refuses `exceptions-would-be-lost:
  {rewrite}:{changed}:{deleted}`; both editors ask and send the same save
  again with the consent (`shared/exceptionsLoss.ts`) — only on the series'
  own save: a split's cut refused this way says the sentence, as a yes would
  write the new series twice. Afterwards it deletes each placeable deleted
  occurrence again: the index the new rule gives, that index and its
  neighbours read back, and only the one whose start is exactly where it
  should stand is deleted — with a cancellation where the update told the
  attendees, whose series has it again. Nothing came back where the series,
  read index by index, steps over the place — from an occurrence before it, or
  the series' start, to one after it, or its end: it now begins or ends beyond
  it, or a new pattern no longer has it — or where the index the rule gives is
  a deletion Exchange kept; an index the restore has just deleted again stands
  for the occurrence it held. An occurrence within half a day of the place
  that is not the one expected came back unconfirmed. One it cannot confirm,
  read or delete is named on the event that comes back
  (`Event::deletions_not_restored`), where it now stands — where the series
  cannot be read again, as the update moved it, a day if it left the series
  all-day —, and the editors, the drag and the carry dialogs say its day.
  Dragging a series and the carry dialogs do not ask yet: they say the
  refusal's sentence. Paris for a Berlin series counts as another clock
  (another Windows id). No editor picks a series' zone yet, so today the zone
  rewrite meets only a rule change where Aperio itself would write another
  zone — the repeat edited, which asks, or the old rule cut when a series is
  changed or deleted from a later occurrence on, which is refused with the
  sentence —: a series whose stored end zone is not its start zone, or a copy
  whose zone is stale. A copy that differs from the server's only in its
  exceptions changes no rule: an update never writes them, so it opens neither
  the zone nor the slot.
- **A series is never written blind** (decision 248). Without its copy, what
  a series holds, and so what a rewrite would drop, is unknown: an update of
  a series head whose copy cannot be read sends nothing and is refused as
  `copy-unreadable`, as an all-day day is (below). A single is still written
  without a comparison.
- **A series starts on its first day** (PR 8a). `rrule_to_ews_recurrence`
  takes the day the series starts on, on the clock Exchange repeats it on
  (`rule_first_day`): the device's day for an all-day series, the written
  zone's day on a create; on an update the stored start zone's day where the
  clock stays (234, inferred from live round 1, where Exchange applied an
  update's fields in order) and the written zone's where it moves (241,
  including a missing copy or a stored zone Aperio cannot read), UTC where
  none is known. The rule is read from the slot the server keeps after the
  update. The range's StartDate and the weekday, day of the month and
  month of a rule without BYDAY, BYMONTHDAY or BYMONTH come from that day.
  Read off the UTC date, as before, they named the day before east of UTC: a
  daily all-day series created in Berlin for Monday 19 October started on
  Sunday 18 (R3-6). A series already stored that way stays so (231); moving
  it and choosing its weekday or day again in the repeat picker mends it.
- **An all-day day is written on the stored zone's midnights** (47a). An
  update writes an all-day boundary as midnight, in the zone Exchange stores
  that boundary in, of the day the device names (`all_day_boundary`; each
  boundary in its own zone, 233). Exchange rounds an item with one stored
  zone in that zone (R3-5b-u), so the UTC midnights Aperio wrote before
  stretched an Outlook item over two days; that it rounds each boundary in
  its own zone when the two differ is the decision (233), which R3-2
  suggests and nobody has measured. `read_before` returns the zones with the copy
  (`StoredZones`); an exception takes its series' ids and definitions. A
  create still writes UTC midnights: Aperio creates without a zone, so
  Exchange stores the item in UTC. Where a boundary's zone cannot be read,
  the day is not written: the update is refused as `day-zone-unreadable`
  (237). Where the copy could not be read at all, it is refused as
  `copy-unreadable`, with the read's error as its detail. Both are carried
  as `Forbidden`, so the phone keeps the sentence and a split knows nothing
  landed. A read that failed on the sign-in or found the item gone keeps its
  own error (`Authentication`, `NotFound`; `names_sign_in_or_gone`), as the
  same edit of a timed appointment reports it: a retry mends neither. A
  save that leaves the day alone still goes out.
- **Zone definitions.** `StartTimeZone` and `EndTimeZone` carry the zone's
  full definition (periods with their bias, yearly changes, eras from
  absolute transitions). Both item parsers read it onto its own boundary
  (`ParsedItem::start_zone_definition`, `end_zone_definition`, ITEM_PARSER
  5), and `zone_definition::ZoneRules` turns it into a clock: the wall time
  at an instant, and a day's midnight (the first hour after it where a
  change skips it). A fifth weekday reads as the month's last (238).
  Exchange 2019 fills these elements in `SyncFolderItems` and `GetItem`, and
  a custom zone's day moved on its definition's midnight lands as one day
  (the 8a live test, on an item created through EWS with a definition). An
  item with a custom zone from an iPhone or an invitation is unmeasured.
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
  differently without the table changing, or a cached item otherwise turns
  into a different event (3: the range's date). A delta whose token names another
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
needs an Exchange/365 mailbox that still exposes EWS. The ignored tests
`live_test_requests` and `live_test_requests_round_3` write the requests of
the live zone tests, into the directory `APERIO_LIVE_TEST_DIR` names; each
file's header comment says what it is. Round 3's files are historical since
PR 8a: its "today's rule" files (R3-2, R3-4, R3-5b-u) put Start and End
where round 3's build did, rebuilt for an item read as stored in UTC, while
an 8a build writes an Outlook item's stored-zone midnights; its 47a
prototype (R3-1, R3-3, R3-7b) is Aperio's rule now. One field differs from
what round 3 sent: the series' rule in R3-1 and R3-2 starts on 8a's first
day (StartDate 2026-10-19 on a Berlin device; round 3 sent 2026-10-18).
