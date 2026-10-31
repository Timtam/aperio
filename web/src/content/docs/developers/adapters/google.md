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
  `iCalUID`, which a series' instances share, is the event's `ical_uid`.
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
  is never touched. A new series' deletions are matched otherwise, by a
  listing of its occurrences (see below).
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
- **An invitation from a file goes in through `events.import`.** Built from
  Google's documentation, not measured (no Google account to test with).
  `events.import` adds "a private copy of an existing event": it is the one
  call that takes an `organizer`, it requires the `iCalUID`, and it has no
  `sendUpdates`, so nobody is told. Google takes it only where the
  calendar's owner is the organizer or an attendee, so only the primary
  calendar (`primary: true` in the calendar list, whose id is the account's
  address) says `invitation_import: kept`; any other calendar says
  `not-kept`, and the import becomes the account's own appointment there.
  A create marked `keeps_invitation` whose organizer is not the account
  (`cal_core::attendee::invitation_organizer`, the account from
  `GET /calendars/primary`) is imported with the file's invitees without the
  organizer, the account added where the file did not name it
  (`invitation_attendees`). Google does not say what an import does with a
  UID the calendar already holds, so `events.list?iCalUID=` asks first, and
  a held one is refused as `uid-taken`. A copy imported without the file's
  UID gets `{uuid}@aperio`. Should Google answer with the account as the
  organizer after all (one report says it may), the appointment stays as
  Google made it, logged: nothing was sent, and it reads as the account's own
  meeting. An unmarked create never asks who the account is.
- **Deletions come in several spellings.** Google itself deletes an
  occurrence as a cancelled instance, never as an `EXDATE` line, but other
  apps write `EXDATE` lines into the master's `recurrence`, and Google
  returns them as written. The adapter reads every spelling seen:
  - a wall clock in a zone, `EXDATE;TZID=Europe/Berlin:20260601T090000`
    (Google's own export form), and without `TZID`, a wall clock in the
    series' `start.timeZone`;
  - a UTC instant, `EXDATE:…Z` or `EXDATE;VALUE=DATE-TIME:…Z`. On a series
    of days, UTC midnight is its date: the form Aperio wrote a date in
    before, and no local midnight of another date falls on it;
  - a date, `EXDATE;VALUE=DATE:20260601`, which on a series of days names
    that day (its local midnight). Google ignores a date on a timed series,
    and so does the adapter.

  Several values on one line, any line order, names in any case and a rule
  with parameters (`RRULE;X-…:FREQ=…`) are read too. A wall clock a clock
  change repeats is its first reading; one it skips takes the offset from
  before the change, where the views place the occurrence. A zone tzdata
  does not know, named by `TZID` or by the series' `start.timeZone`, costs
  only a wall clock on a timed series. A line the
  adapter does not keep (`RDATE`, `EXRULE`, an earlier of two rules, an
  unknown zone's wall clock) is logged as a warning once per run.
- **Deletions are written as Google spells them, on an update.** The wall
  clock in the series' zone with `TZID`, a date (`VALUE=DATE`) on a series
  of days (the only form Google's documentation allows there), and the UTC
  instant on a series without a zone or for the second pass of an hour the
  clock shows twice. One value per line. A create writes none: a new
  series' deletions are cancelled once it stands (see below).
- **A kept start, end and repeat stay out of the PATCH, together.** A PATCH
  replaces the whole `recurrence` array, so an update that kept the start,
  the end, the all-day flag and the repeat (`keep_fields`) leaves all four
  out, and lines the adapter does not read stay on Google. They go
  together: Google expands the lines from the start, and a kept field may
  be another device's newer change, which this device's start must not be
  paired with. Before, every save, a rename included, wrote the array anew
  and erased deletions other apps had written.
- **Zone data.** Wall clocks are read and written with chrono-tz's tzdata
  (2025b, the newest chrono-tz release). For a zone whose rules changed
  since (Casablanca and El Aaiún from 2026-09-20; Vancouver, Edmonton and
  Inuvik from 2026-11-01; Chișinău's transition times), a wall clock can
  sit an hour from Google's occurrence. That holds for a timed deletion
  written there that was not read as a wall clock in the series' own zone
  — a UTC one another app wrote, say, or a wall clock in another zone: both
  are written as a wall clock in the series' zone. A wall clock read in the
  series' own zone and written again keeps its digits, since both
  directions use the same data — except one that data skips: it goes back
  as the reading after the change (02:30 as 03:30), the same instant in
  Aperio's data. Where Google's data has no gap there, or another one
  (Casablanca and El Aaiún after each Ramadan from 2027; Vancouver,
  Edmonton and Inuvik each March from 2027; Chișinău's spring hour),
  Google reads it an hour later, and the deletion meets no occurrence.

## A new series keeps its deleted occurrences

Google takes `EXDATE` lines with a new series, but whether it heeds them
was never measured (decision 137), and Google itself deletes an occurrence
as a cancelled instance. So a series created with deletions — a split's new
part, a move, a copy, a duplicate, the editor's calendar change — goes
through `series_create` (decisions 202-213, 263-269):

- Deletions before the device's day are not written again
  (`cal_core::deletions_from_today`); with none left it is one create of
  the series, without a deletion line.
- The series is created without a deletion line, under an id Aperio gives
  it (32 lowercase hex digits, within Google's base32hex), and the create
  follows the notify switch (`sendUpdates`): the invitees get their
  invitation with the series (204, 267). A create Google turns down whole
  is that error. One asked to slow down (429, or 403 `rateLimitExceeded` or
  `userRateLimitExceeded`) made nothing and is sent again, a few times, and
  then says the provider is busy; asked to wait longer than 4 s, it says so
  at once. One refused with 400 is sent once more
  without Aperio's id. A create whose
  answer was lost — none came, a 5xx, one no reader takes — is read back by
  its id and never sent again, as Google does not promise to catch the
  duplicate; not found, it may stand (`created-may-stand:
  create-answer-lost`).
- The series Google stored must carry its one rule and no deletion line of
  its own.
- From the confirmed create there are 15 s (`cal_core::SERIES_WITH_DELETIONS`),
  however long the create took.
  The occurrences in the deletions' window are listed
  (`events/{id}/instances?showDeleted=true`, every page), and each deletion
  is matched to the occurrence starting there, else to one at most an hour
  off and never past half the way to its neighbours — the hour an outdated
  zone table can put between Aperio's slot and Google's (see Zone data). An
  all-day deletion is matched by the day it names on the device. A deletion
  without a match is listed for once more; still without one, Google's rule
  makes no occurrence there, and it is skipped and logged (207, 266, 269).
- Each match is cancelled with `PATCH {"status":"cancelled"}` on the id
  Google listed, `sendUpdates` as the switch says (204). Its answer is the
  proof (208): this occurrence, cancelled, of this series, at this slot,
  where the answer names them. A lost answer is decided by reading the
  occurrence, and one not cancelled is cancelled once more.
- The occurrences are listed again: each one that stood before and was not
  cancelled here still stands, and each one cancelled here is cancelled or
  gone. Someone else's cancel takes everything back.
- A rate limit is waited out by `Retry-After`, or 1 s, 2 s, 4 s, while it
  fits (`cal_core::may_send` counts the slowest request so far); a quota
  used up for the day is not. Every request is cut at the deadline. A step
  that no longer fits right after Google asked to wait, or once the waits
  took half the time to settle or more, fails as `busy`, not `time`
  (`cal_core::out_of_time_reason`, 276), and a series taken back for it
  says the provider was busy.
- Any failure deletes the series again inside the 15 s kept for that,
  telling the invitees exactly when the create told them (212, 264):
  `deletions-not-kept` when it is gone, `created-may-stand` when it may
  stand, the detail starting with `invited` where the invitees may have
  heard of it. With notify on they may also have got a cancellation for
  each occurrence cancelled before.
- On `adapter_google::write`, never with an id: one INFO line per skipped
  slot; on success one INFO line with the counts, the time and every slot;
  on failure one WARN with the counts, the time, the step that failed and
  why (`http-<status>`, `google-<reason>`, `network`, `unreadable`,
  `control`, `after-control`, `foreign-deletion`, `busy`, `time`). A lost
  create answer has that WARN too.
- The manifest declares it (`recurrence.deletions_in_steps`), and the
  surfaces show a "please wait" dialog while such a series is created.
- Aperio reads Google's cancelled occurrences only as far as its cache
  window reaches, about a year ahead (150): a deletion further ahead is
  cancelled at Google too, but once Aperio reads the series afresh it no
  longer sees it, and a later copy of the series shows that occurrence
  again.

## A busy Google, and saves of several requests

- **A throttle says the provider is busy.** Google turns a request away for
  throttling with 429, or with 403 and the reason `rateLimitExceeded` or
  `userRateLimitExceeded`. The adapter reads that from the status and the
  whole answer — the error it keeps is cut at 300 characters — wherever an
  answer is read, the token endpoint included, and says the provider is busy
  (`cal_core::Error::provider_busy`, decision 270): a write was turned away
  and nothing changed, a read failed. The contact typeahead says so only
  when none of its three sources answered and one was busy; any answer gives
  what it found.
- **A used-up limit is no sign-in failure.** A 403 naming `quotaExceeded`,
  `dailyLimitExceeded` or `calendarUsageLimitsExceeded` means a limit is used
  up for a while (275): a write says the server refused it, with that
  reason, and a read is an ordinary failure. Neither asks to sign in again,
  which any other 403 still does.
- **A walk stops for the whole account.** A delete, a cancelled occurrence
  and an answer to an invitation walk the calendars (a task delete, the
  task lists), as the trait gives no container. A busy answer or a used-up
  limit holds for the whole account, so the walk stops there instead of
  asking the others, whose "not found" would hide why. Where an earlier
  calendar's delete lost its answer, the event may be gone: the outcome
  stays that unsure one, never "nothing changed". A listing that fails
  before anything is written is mapped as that write's failure.
- **A save of several requests reads before it writes.**
  - A task edit sends a PATCH for its fields, and a `/move` only when the
    PATCH answer shows the task under another parent than the one asked
    for: an edit that keeps the parent is one request. The task returned is
    the move's answer.
  - A contact group is created and then given its members
    (`members:modify`). A group edit reads the group first, and renames it
    only when the name changed, so a members-only edit is one request. Both
    find every member by address before anything is written, with the three
    searches of the typeahead: a source that answered it has nothing for
    this account (an HTTP 4xx other than a refused sign-in; an account
    without a directory) gives no hits, but one that gave no such answer —
    busy, a used-up limit, unreached, a server error — fails the save with
    nothing written. Before, such a member was left out of a new group, or
    taken out of an edited one.
  - A contact edit reads the contact's other dates first, as the PATCH
    replaces the whole list. A busy answer there changes nothing; any other
    failure still saves the edit, without them.
  - A new contact with a photo is created first, then gets its photo. A
    photo turned away, by a busy provider too, leaves the contact without
    one, and the create succeeds: the editors cannot yet stay on a created
    contact, and an error would offer the create again.

  When a later request fails after an earlier one landed, the change stands
  in part, and the error never says nothing was changed (277,
  `cal_core::Error::after_landed`): a busy provider is said as "partly
  saved" (`partly-saved: move` or `members`), anything else as a network
  error that may have landed. After any contact create, edit, delete or
  photo write, whether it worked, stood in part or failed, the adapter drops
  its own contacts listing, so the hosts' re-read asks Google again.
- **Other Contacts.** A listing Google throttles, or turns away for a
  used-up limit, is an error. Only a 403 that names neither (the scope not
  granted) still reads as an empty list, so neither ever empties the Other
  Contacts Aperio keeps.

## Testing

`mockito` with canned Calendar/Tasks/People JSON; `ApiState` carries the
three API bases, so a test points all of them at its mock server. Tests
cover the full-list vs. incremental (`syncToken`) paths, the `410 → full
resync` fallback, the recurrence/colour mapping, every throttle and limit
answer, each walk stopped for the account, and each save of several
requests failing at each step. A new series written in steps
runs against a scripted Google series instead: its occurrences, the
requests in order, and faults injected by request kind and turn (a rate
limit, a lost or unreadable answer, a cancel that did nothing, someone
else's cancel, a listing that lags or comes back empty). One test runs every
fault at each request of a plain run, a second every fault at the requests
only another fault sends (the read-back after a lost create, the read of a
cancel whose answer was lost, the take-back and its read), at their first
turn and at the second one a failed first turn sends, and both check that
each outcome says what stands.

None of that was run against Google: there is no live test of it. The model
pins Aperio's reading of Google's documentation where Google promises
little — a cancel's answer may name only its id and status, a listing right
after a write may lag once, Google may spell the rule otherwise, a 400 may
refuse Aperio's id — so a different answer from Google fails a named test.
A live check needs a Google account and an OAuth client configured for the
Calendar/Tasks/People scopes.
