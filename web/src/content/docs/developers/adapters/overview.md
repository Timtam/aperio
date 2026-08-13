---
title: "Adapters"
---

An **adapter** turns one provider's API into Aperio's `cal-core` vocabulary.
Each lives in `crates/adapter-<provider>` as a normal Rust library and
implements the `cal-core` feature traits it can support; a matching
`crates/adapter-<provider>-plugin` exposes it over the plugin ABI.

## The capability model

A provider implements only the traits it can. Capabilities are declared in
the plugin's `plugin.json` and surfaced to the UI so it can hide
affordances a backend can't fulfil.

| Trait | Provides |
|---|---|
| `Adapter` | base: `authenticate`, `capabilities` |
| `CalendarFeature` | calendars + events |
| `TasksFeature` | task lists + tasks (+ optional sections, assignees, membership) |
| `ContactsFeature` | address books + contacts |

A "tasks-only" provider like Todoist or Vikunja declares only `["tasks"]`;
a full provider like Google or Microsoft Graph declares calendars, tasks
and contacts.

## How the host reads data: full vs. delta vs. range

Adapters differ in how they enumerate changes, which the host must respect
when caching:

- **Delta / sync-token** providers return *changes since a token*
  (Google `syncToken`, Graph `delta`, CalDAV `sync-collection`, EWS
  `SyncFolderItems`). A `410 Gone`/invalid token means "do a full resync".
- **Folder-complete** snapshots return the *entire* set (EWS events after
  the folder-sync rework, CalDAV/iCloud events via PROPFIND-enumeration +
  `sync-collection`, iCal feeds). These set `ChangeSet.complete = true` so
  the host stores an unbounded cache window and serves every later view
  range straight from the snapshot.
- **Range-scoped** reads return only what overlaps a time window
  (Google/Graph event reads, plus the legacy CalDAV ctag fallback for
  servers without `sync-collection`). The host keeps a bounded cache window
  for these and re-fetches when the view moves outside it.

> **The host's token is authoritative for a delta.** `get_*_delta(…,
> since_token)` must compute its changes relative to `since_token` — the
> cursor the host's cache is actually at — **not** any cursor the adapter
> persists for itself. Other host paths read the same provider on their own
> schedule (the reminder scanner calls `get_events` directly), so a stateful
> adapter that drains from its *own* advancing cursor would skip changes that
> path already consumed but the host never cached, stranding an edited event
> at its old time until a full resync. CalDAV is the model — `sync-collection`
> drains straight from the passed token; EWS learned this the hard way and
> now seeds its `SyncFolderItems` drain from `since_token` too.

> **Reads never block the first paint.** `get_events`/`get_tasks`/
> `get_contacts` serve whatever snapshot exists *right now* and run the
> refresh in the background (stale-while-revalidate). When it lands the host
> emits `cache-updated` and the view re-reads. A slow cold sync — e.g. a
> first iCloud full fetch — therefore fills in progressively instead of
> freezing startup for 20 s+.

> **Recurring events.** Most adapters return the recurring **master** with
> its `RRULE`; the frontend expands occurrences for the visible range via
> `rrule.js`. Adapters must therefore pass a master through even when its
> *first* occurrence falls outside the requested window (it may still
> recur into it). Microsoft Graph is the exception — it uses
> `/calendarView`, which expands occurrences server-side.

> **Attendee scheduling is server-side, never client SMTP.** When the user
> opts to notify, the adapter asks the *provider* to email attendees:
> EWS flips `SendMeetingInvitations*` to `SendToAllAndSaveCopy`, Google
> appends `?sendUpdates=all`, CalDAV/iCloud writes `ORGANIZER`+`ATTENDEE`
> for RFC 6638 auto-scheduling (detected at discovery via
> `schedule-outbox-URL`), and Graph sends automatically once attendees are
> in the body. Each calendar carries a `supports_scheduling` flag — static
> for EWS/Google/Graph, runtime-detected for CalDAV — that gates the UI
> toggle. The transient `send_invitations` (on `NewEvent`/`Event`) and
> `send_cancellations` (on `delete_event`) ride the call, never the stored
> data. Note: on iCloud and Graph, *storing* attendees and *emailing* them
> are inseparable — they're written only when notifying.

> **Free/busy lookup** runs through `get_free_busy(emails, range)` and the
> host `query_free_busy` command (the dialog's "Check availability"
> button). Each provider answers in its own dialect: EWS `GetUserAvailability`
> SOAP (`RequestedView=Detailed`, one `MailboxData` per address, results in
> request order), CalDAV/iCloud an RFC 6638 iTIP `VFREEBUSY` POSTed to the
> principal's `schedule-outbox-URL` (busy periods parsed out of the
> `schedule-response`), Google `POST /freeBusy`, Graph `POST
> /me/calendar/getSchedule`. All degrade gracefully: a mailbox the server
> can't resolve (or a provider that can't answer) yields an empty slot list
> — "availability unknown" — rather than failing the call. Local/iCal
> calendars return empty.

> **RSVP** rides three pieces: read-side population of `Event.organizer` +
> `attendee_responses` (per provider — CalDAV `ATTENDEE;PARTSTAT`, EWS
> `ResponseType`, Google/Graph `responseStatus`), `current_user_email()`
> for the "am I a non-organizer attendee?" gate (CalDAV
> calendar-user-address, Graph `/me`, Google primary-calendar id, EWS
> login), and `respond_to_event(event_id, status, send_response)`: EWS
> `AcceptItem`/`DeclineItem`/`TentativelyAcceptItem`, Graph
> `/accept|/decline|/tentativelyAccept`, Google self-`responseStatus`
> patch + `sendUpdates`, CalDAV `PARTSTAT` PUT (RFC 6638 servers auto-emit
> the iTIP `REPLY`; `Schedule-Reply: F` suppresses it). `NeedsAction`
> isn't respondable. The shim maps a null `current_user_email` slot to
> `Ok(None)` so read-only adapters hide RSVP rather than erroring.

## Contact channels carry a label

`Contact.emails`, `Contact.phone_numbers` and `Contact.urls` are lists of
`ContactValue { value, label }` — the label being free text, because two of
the five contact providers store whatever word the user typed. Each provider
records the same idea in its own way, and the adapter translates at its edge:

| Provider | Where the label lives | Free labels? |
|---|---|---|
| CardDAV | `TYPE` parameter; Apple's grouped property + `X-ABLabel` for custom ones | yes |
| Google People | `type` on each `emailAddresses`/`phoneNumbers`/`urls` entry | yes |
| Exchange (EWS) | the entry `Key` (`MobilePhone`, `HomePhone`, …) | no — four voice slots + fax, three email slots |
| Microsoft Graph | the collection the value sits in (`mobilePhone`, `homePhones`, `businessPhones`) | no — and `mobilePhone` holds exactly one |
| Local store | stored verbatim as JSON | yes |

Three rules follow from that asymmetry:

- **The value always travels, the word may not.** On a fixed-slot provider,
  a label with no slot of its own (or one already taken) falls back to the
  next free slot rather than dropping the value. What can't be written at
  all is logged, never discarded silently. The fallback deliberately skips
  Exchange's fax keys: a voice number filed under `HomeFax` would be dialled
  as a fax by every other client.
- **A masked write replaces, so read before you write.** Google's
  `updatePersonFields` clears any listed field the body omits. Aperio models
  one dated entry (the anniversary) but Google lets a contact carry several,
  so `update_person` re-reads `events` and passes the others back through —
  without that, renaming a contact deleted every custom date on it.
- **A bare string is still a legal channel.** Everything stored before
  labels existed is a plain `"max@example.com"` on the wire and in the
  cache, so `ContactValue` deserialises both shapes and the frontends
  normalise via `toContactValues` from `@aperio/shared`.

Alongside the channels, a contact carries `anniversary`, `job_title` and
`department`. Every provider has all three except **Microsoft Graph, which
has no anniversary property on `contact` in v1.0** — only `birthday`. It
stays null on Outlook accounts rather than being faked onto another field.

## The adapters

| Adapter | Crate | Capabilities | Protocol |
|---|---|---|---|
| [Local store](/developers/adapters/local/) | `adapter-local` | calendars, tasks, contacts | host SQLite (source of truth) |
| [CalDAV / iCloud](/developers/adapters/caldav/) | `adapter-caldav` | calendars, tasks, contacts | CalDAV / CardDAV |
| [Google](/developers/adapters/google/) | `adapter-google` | calendars, tasks, contacts | Google REST APIs (OAuth2) |
| [Microsoft Graph](/developers/adapters/microsoft/) | `adapter-microsoft-graph` | calendars, tasks, contacts | Microsoft Graph (OAuth2) |
| [Exchange (EWS)](/developers/adapters/ews/) | `adapter-ews` | calendars, tasks, contacts | Exchange Web Services (SOAP) |
| [Vikunja](/developers/adapters/vikunja/) | `adapter-vikunja` | tasks | Vikunja REST API |
| [Todoist](/developers/adapters/todoist/) | `adapter-todoist` | tasks | Todoist REST v2 (+ Sync API) |

There is also an iCal/ICS subscription adapter (`adapter-ical`,
read-only calendar feeds).

Each page below covers the protocol, the authentication flow, provider
quirks worth knowing, and how to test the adapter.
