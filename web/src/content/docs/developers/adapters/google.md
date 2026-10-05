---
title: "Google"
---

**Crate:** `adapter-google` · **Capabilities:** calendars, tasks, contacts

Talks to the Google Calendar, Tasks, and People (Contacts) REST APIs.

## Authentication

OAuth2. The host runs the OAuth flow and threads the resulting access/
refresh token to the adapter; token refresh is handled host-side. The
adapter just sends a `Bearer` token.

## Reading data

- **Events:** `events.list` with `singleEvents=false`, so recurring
  **masters** come through with their `recurrence` (RRULE/EXDATE) intact —
  the frontend expands occurrences. Incremental updates use a `syncToken`
  (no time bound); a `410 Gone` means the token expired → full resync.
- **Tasks:** the Tasks API per task list.
- **Contacts:** the People API, including *Other Contacts* via its own
  `syncToken`.

Colours: Google calendars expose a `backgroundColor` hex, taken directly.

## Quirks

- **`singleEvents=false` + `timeMin`/`timeMax`.** The full read is range
  bounded. Whether Google returns a recurring master whose `DTSTART`
  predates `timeMin` (but which recurs into the window) is Google-specific
  behaviour worth verifying against a real account; if a long-running
  series ever fails to show, this is the first suspect (drop/relax
  `timeMin` on the full read).
- **410 → resync.** Treat an invalid `syncToken` as "start over with a full
  list", then resume delta from the new token.
- **One occurrence is found by its slot.** Google keeps a changed or
  cancelled occurrence as an instance with an id of its own, and it rejects
  an instance id a client builds itself (HTTP 400). Updating an override,
  deleting one, and skipping an occurrence all look the instance up with
  `events/{master}/instances?originalStart=…&showDeleted=true`, which
  returns the instance in that slot however far it was moved. The adapter
  then writes to the id Google returned. The slot is an RFC 3339 instant,
  or a date for an all-day series, and the answer is compared as a parsed
  instant, because Google replies in the event's own zone. An empty slot is
  an error: nothing is cancelled or written, and a neighbouring occurrence
  is never touched.
- **The organizer is flagged among the attendees.** Google marks the
  organizer's row with `organizer: true`; the adapter drops it by that flag,
  which also holds when the address is spelled otherwise (gmail.com and
  googlemail.com). `organizer.self` says whether the calendar's own account
  organizes the event, and that answer counts even without an organizer
  address; when it says no, the event is `organized_elsewhere` and only the
  organizer notifies. An event without an organizer is the account's own. A PATCH replaces the whole `attendees` array, so an update whose
  invitees did not change (`keep_attendees`) leaves the array out, and
  Google's list, the organizer's row included, stays as it is. One that
  removed every invitee (`clear_attendees`) sends an empty array; an empty
  list alone leaves the array out.
- **Deletions come in several spellings.** Google itself deletes an
  occurrence as a cancelled instance, never as an `EXDATE` line, but other
  apps write `EXDATE` lines into the master's `recurrence`, and Google
  returns them as written. The adapter reads every spelling seen:
  - a wall clock in a zone, `EXDATE;TZID=Europe/Berlin:20260601T090000`
    (Google's own export form), and without `TZID`, a wall clock in the
    series' `start.timeZone`;
  - a UTC instant, `EXDATE:…Z` or `EXDATE;VALUE=DATE-TIME:…Z`;
  - a date, `EXDATE;VALUE=DATE:20260601`, which on a series of days names
    that day (its local midnight). Google ignores a date on a timed series,
    and so does the adapter.

  Several values on one line, any line order, names in any case and a rule
  with parameters (`RRULE;X-…:FREQ=…`) are read too. A wall clock a clock
  change repeats is its first reading; one it skips takes the offset from
  before the change, where the views place the occurrence. A line the
  adapter does not keep (`RDATE`, `EXRULE`, an unknown zone) is logged as a
  warning once per run.
- **Deletions are written as Google spells them.** The wall clock in the
  series' zone with `TZID`, a date (`VALUE=DATE`) on a series of days (the
  only form Google's documentation allows there), and the UTC instant on a
  series without a zone or for the second pass of an hour the clock shows
  twice. One value per line.
- **A kept repeat stays out of the PATCH.** A PATCH replaces the whole
  `recurrence` array, so an update that kept the repeat, the start and the
  all-day flag (`keep_fields`) leaves the array out, and lines the adapter
  does not read stay on Google. Before, every save, a rename included,
  wrote the array anew and erased deletions other apps had written.

## Testing

`mockito` with canned Calendar/Tasks/People JSON. Tests cover the
full-list vs. incremental (`syncToken`) paths, the `410 → full resync`
fallback, and the recurrence/colour mapping. A live check needs a Google
account and an OAuth client configured for the Calendar/Tasks/People
scopes.
