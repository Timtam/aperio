---
title: "Microsoft Graph"
---

**Crate:** `adapter-microsoft-graph` · **Capabilities:** calendars, tasks, contacts

The modern Microsoft API for Outlook/Microsoft 365 — calendars + events,
Microsoft To Do (`todoTask`), and Outlook contacts.

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
  each month's instances are fetched (and cached) per range.
- **Delta:** `/calendarView/delta`, `/todoTask` delta, and contacts delta,
  each with the same `410/invalid token → full resync` rule.

## Quirks

- **Colours** arrive as a named enum (`auto`, `lightBlue`, …) and are mapped
  to hex.
- **Recurrence model.** Because `/calendarView` expands server-side, the
  "recurring master missing from a future view" class of bug doesn't apply
  here — but each range needs its own fetch (no master reuse across
  months).
- **MS To Do has no assignment.** Task assignment is a *Planner* concept,
  which is a separate, heavier surface and out of scope.
- **MS To Do has no subtasks either.** Its `checklistItems` are plain
  strings without a usable write API and don't map onto task→task
  parents, so the plugin manifest declares `subtasks: false` and the
  editors never offer subtasks on a Graph list.

## Testing

`mockito` with canned Graph JSON, including delta envelopes
(`@odata.deltaLink` / `@odata.nextLink`). Tests cover the
initial-vs-follow delta paths, the `410 → resync` fallback, and the
calendarView instance mapping. Live testing needs an Azure app
registration with the relevant Graph scopes.
