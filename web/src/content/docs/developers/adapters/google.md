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

## Testing

`mockito` with canned Calendar/Tasks/People JSON. Tests cover the
full-list vs. incremental (`syncToken`) paths, the `410 → full resync`
fallback, and the recurrence/colour mapping. A live check needs a Google
account and an OAuth client configured for the Calendar/Tasks/People
scopes.
