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
  googlemail.com). `organizer.self` says the calendar's own account organizes
  the event; otherwise it is `organized_elsewhere` and only the organizer
  notifies. A PATCH replaces the whole `attendees` array, so an update whose
  invitees did not change (`keep_attendees`) leaves the array out, and
  Google's list, the organizer's row included, stays as it is.

## Testing

`mockito` with canned Calendar/Tasks/People JSON. Tests cover the
full-list vs. incremental (`syncToken`) paths, the `410 → full resync`
fallback, and the recurrence/colour mapping. A live check needs a Google
account and an OAuth client configured for the Calendar/Tasks/People
scopes.
