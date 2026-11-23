---
title: "Microsoft Graph"
---

**Crate:** `adapter-microsoft-graph` · **Capabilities:** calendars, contacts

The modern Microsoft API for Outlook/Microsoft 365 — calendars + events and
Outlook contacts. Microsoft To Do tasks were dropped in November 2026, when
Microsoft announced To Do's retirement; a Microsoft account brings no tasks.

## Authentication

OAuth2 (Microsoft identity platform). As with Google, the host owns the
flow and token refresh; the adapter sends a `Bearer` token.

## Reading data

- **Events:** the adapter uses **`/calendarView`** (with
  `startDateTime`/`endDateTime`), which expands recurring events
  **server-side** into individual instances for the range. This is unlike
  the master+frontend-expansion model the other adapters use — it's simpler
  here because Graph's structured recurrence doesn't map 1:1 to RRULE for
  every shape. Consequence: the adapter never returns a recurring *master*;
  each month's instances are fetched (and cached) per range. `iCalUId`
  differs per instance, so it is an event's `ical_uid` only for a
  `singleInstance`; `uid`, which a series shares, wins where Graph sends it
  (unmeasured whether the default property set carries it).
- **Delta:** `/calendarView/delta` and contacts delta, each with the same
  `410/invalid token → full resync` rule.

## Quirks

- **Colours** arrive as a named enum (`auto`, `lightBlue`, …) and are mapped
  to hex.
- **Recurrence model.** Because `/calendarView` expands server-side, the
  "recurring master missing from a future view" class of bug doesn't apply
  here — but each range needs its own fetch (no master reuse across
  months).
- **The organizer answers "organizer".** Graph lists the organizer among the
  attendees with `status.response` "organizer"; the adapter drops that row by
  its response. `isOrganizer` says whether the signed-in user organizes the
  event, and that answer counts even without an organizer address; when it
  says no, or when it is missing and an organizer is named, the event is
  `organized_elsewhere` and only the organizer notifies. An
  update whose invitees did not change (`keep_attendees`) sends no
  `attendees`, so Graph keeps its own list. One that removed every invitee
  (`clear_attendees`) sends an empty list, but only when notifying: Graph
  mails whenever `attendees` is in the body, so a silent removal cannot
  reach it.
- **No repeat is a `null`, and only on the host's word** (decision 280). A
  missing pattern leaves `recurrence` out of the PATCH, since a series whose
  pattern the adapter cannot read has none either. Only `clear_recurrence`,
  which `guard_update` sets when the cached copy repeats, the edit does not,
  and the edit was made from that very version (same ETag), sends
  `"recurrence": null`, as Graph's update reference shows it. Not where the
  event read just before the PATCH holds no pattern (an occurrence, or a
  series another client made single). Unmeasured against a live mailbox,
  and dormant today: the adapter reads only `/calendarView`, so no cached
  row repeats and `guard_update` never sets the flag for a Microsoft 365
  event. It takes effect once the adapter reads series masters.

- **A series' dates on its own clock** (DESIGN-series-time-zone.md, stage
  5). The adapter writes no `recurrenceTimeZone`, so Graph reads a repeat
  range on the clock of the event's start, and `startDate` must be the
  start's date. The range therefore starts on the day the start is written
  on: an all-day series on the device's day, a zoned one on its zone's,
  anything else on UTC's. A rule without BYDAY, BYMONTHDAY or BYMONTH takes
  its weekday, day and month from that day. Read off the UTC instant, as
  before, the range and its default weekday of a Berlin series at 00:30 were
  a day early, and those of a New York series at 20:00 a day late.
- **A series' end is a day on that clock** (DESIGN-series-time-zone.md,
  140a). Every write of an end, a create, an update, a copy, goes through
  `cal_core::written_end_day` on the clock the start is written on: the last
  day whose occurrence, at the master's wall time, starts at or before the
  `UNTIL`, as the Exchange adapter writes it. An unflagged `…T235959Z`
  counts as its own date, the day the repeat field wrote it for; a cut
  (`truncate_tail_overrides`) is counted as the instant it is, and one the
  clock cannot place is refused. An all-day series keeps the date it names.
  The read-back of a POST or PATCH response spells `endDate` D with
  `cal_core::until_through` on the clock the views read the zone on, with no
  exact-case gate: `DT235959Z` where that reads through D, else the last
  instant of D there (19:00 New York gives `…T045959Z` of the next UTC day),
  so an untouched save writes D again. The manifest declares
  `recurrence.ends_by_date`. Whether Graph counts `endDate` whole on the
  series' clock, as Exchange does, and which zone name it sends back with a
  master, are not measured (TODO.md). The cut is dormant today, like
  `clear_recurrence`, because the adapter reads no series masters.
- **Relative patterns are not written.** `rrule_to_recurrence` has no
  `relativeMonthly` or `relativeYearly`. Only a monthly rule with BYDAY and
  BYSETPOS and no BYMONTHDAY is refused. Any other relative rule goes out as
  an absolute pattern on the start's day: an ordinal BYDAY (`BYDAY=2SU`) as
  `absoluteMonthly`, a yearly rule with BYDAY, with or without BYSETPOS, as
  `absoluteYearly`, and a BYSETPOS beside BYMONTHDAY is dropped (TODO.md).
  Reading drops both relative patterns to no recurrence.

- **Graph always notifies.** Attendees in the body of a create or an update
  are mailed, and deleting a meeting on the organizer's calendar sends its
  cancellation (Microsoft's documentation; not measured live). Its calendars
  therefore carry `always_notifies_attendees` with `notifier_name`
  "Microsoft 365", and the editors say so instead of offering a silent save
  or delete (decision 82b).

- **A busy Graph.** The request helpers build `GraphError::Busy` (decision
  270): for 429, which did nothing, and for 503 with `Retry-After` on a
  request that writes nothing (the reads, `getSchedule` through
  `post_query_json`, and the token endpoint). A write answered 503 may have
  been carried out, so it stays unsure. The wait Graph names is only logged.
  An update whose read of the current event is busy sends no PATCH. A busy
  DELETE after a landed `/cancel` counts as done, as a 404 does: the meeting
  is cancelled and off the calendar. Any other failure of that DELETE goes
  through `after_landed`: the cancellation reached every attendee, so it never
  reads as "nothing was changed". A contact search says busy only where
  neither source answered. A new contact whose photo upload fails is still
  returned, with `has_photo` false, and the editor says so.

## Testing

`mockito` with canned Graph JSON, including delta envelopes
(`@odata.deltaLink` / `@odata.nextLink`). Tests cover the
initial-vs-follow delta paths, the `410 → resync` fallback, and the
calendarView instance mapping. Live testing needs an Azure app
registration with the relevant Graph scopes.
