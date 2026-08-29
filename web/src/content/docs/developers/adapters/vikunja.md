---
title: "Vikunja"
---

**Crate:** `adapter-vikunja` · **Capabilities:** tasks

Vikunja is an open-source, self-hosted task manager. The adapter is
**tasks-only** and is one of the two "collaborative tasks" backends
(assignment + membership), alongside Todoist.

## Authentication

A personal **API token** the user mints in Vikunja, sent as a `Bearer`
token. The server base URL is user-supplied (self-hosted or hosted
instance).

## API versions

Vikunja 2.4.0 introduced **API v2** and froze v1 (deprecated in 3.0,
removed in 4.0). The adapter speaks both: it probes
`GET /api/v2/projects` once per client (concurrent first calls share
one probe) and pins v2 only on **positive evidence** that Vikunja's v2
router answered — a JSON body (the list envelope, or a problem+json
auth error), which the things standing in front of a server (SPA
fallbacks, auth gates, WAFs) never produce. 404/405 means a pre-2.4
server and pins v1; other non-JSON answers conservatively pin v1;
transient answers (5xx, 408/429) and transport errors propagate
*without* pinning, so the next call re-probes. Version-dependent
behaviour funnels through semantic client helpers (`create_json`,
`update_json`, `get_page`), so call sites stay verb-agnostic. The
differences the adapter bridges:

- **Verbs:** v1's unusual `PUT` = create / `POST` = update becomes
  conventional `POST` = create / `PATCH` = update (JSON Merge Patch,
  sent as `Content-Type: application/merge-patch+json` — v2 dispatches
  its patch dialects by media type). PATCH is chosen over v2's
  full-replace `PUT` so fields Aperio doesn't model (reminders,
  favourites) survive an update.
- **Clear semantics:** v1 cleared omitted fields (fresh-struct decode);
  merge patch keeps them. Fields our body omits *with clear intent* —
  the three dates and an emptied description — are sent as explicit
  `null` on v2.
- **Lists** become pagination envelopes (`{ items, total_pages, … }`)
  instead of bare arrays with header-based paging; walkers trust
  `total_pages` on v2 (the short-page heuristic only decides when no
  count was given), `items: null` (Go's nil slice) reads as empty, and
  endpoints that were unpaginated on v1 (views, buckets, members,
  shares) are walked page by page on v2.
- **Strict schemas:** v2 request bodies are validated with
  `additionalProperties: false`. The share bodies drop the legacy
  `user_id`/`right` compatibility keys on v2 (v1 keeps sending old +
  new names so legacy servers bind them).
- **Renamed/moved endpoints:** `/projects/{id}/projectusers` →
  `/projects/{id}/users/search`; user search `?s=` → `?q=`; bucket move
  and assignee-bulk go from `POST` to `PUT` (same path/body); share
  right-change goes from `POST` to `PUT`; bucket rename only has a
  full-replace `PUT` on v2, so the current bucket is read first and its
  `position` and WIP `limit` sent along (a bucket the read-back can't
  find is an error — replacing blind would zero both).
- **Errors** are RFC 9457 problem+json; validation failures report 422
  where v1 used 412 (both map to `Conflict`).

## Data model mapping

- **Projects → task lists.** Projects nest via `parent_project_id`
  (surfaced as `TaskList.parent_id`), so the sidebar renders the tree.
- **Buckets → sections** (per-project kanban view); degrades to "no
  sections" on servers that don't expose the view/bucket endpoints.
- **Tasks:** `start_date`/`due_date` map onto Aperio's
  `scheduled_*`/`deadline_*`. Vikunja uses an RFC-3339 sentinel
  (`0001-01-01T…`) for "no date" — treated as unset on read, emitted on
  write when a slot is empty.
- **Priority** maps by label: Vikunja 1/2/3 (Low/Medium/High) ↔ Aperio
  Low/Medium/High. Unset (0) reads as Low; Urgent (4) and DO NOW (5) collapse
  to High and write back as High (3) — Aperio has only three levels.
- **Status / in-progress.** Vikunja has no boolean "in progress", only
  `done` + `percent_done`. Aperio rides `percent_done`: Completed →
  `done = true`; **InProgress → `done = false`, `percent_done = 0.5`**;
  Open → `done = false`, `percent_done = 0`. On read a not-done task with
  any progress is InProgress, so a task nudged to e.g. 50% in Vikunja's own
  UI round-trips too, and the three-step check-off works
  (`supports_in_progress: true`). Cancelled has no Vikunja equivalent
  (`done = false`, `percent_done = 0`) — the marker stays local.
- **Subtasks are task *relations***, not a task field: the child's
  `parenttask` relation (created on `/tasks/{child}/relations`, removed
  via `DELETE /tasks/{child}/relations/parenttask/{parent}`) maps onto
  `Task.parent_id`; reads take the first non-zero
  `related_tasks.parenttask` entry. Updates reconcile against an
  authoritative `GET /tasks/{id}` — Vikunja populates `related_tasks` only
  on reads, never on the update echo — unlink every stale parent (Vikunja
  allows several), and treat create→409 (already linked) / DELETE→404
  (already gone) as success.

## Collaboration (assignment + membership)

- **Assignees** (multi): read inline on the task; written via the bulk
  endpoint `…/tasks/{id}/assignees/bulk` (v1 `POST`, v2 `PUT`;
  replace-semantics — the full desired set). The assignee `TaskUser.id`
  is the numeric user id.
- **Member pool** for the picker: `GET /projects/{id}/projectusers` on
  v1, `GET /projects/{id}/users/search` on v2.
- **Own identity** ("me"): `GET /user`.
- **Membership/sharing:** `GET /projects/{id}/users` lists direct shares
  *with rights* (read/write/admin → 0/1/2); add/remove/change-right on
  `/projects/{id}/users` (create and right-change follow the version's
  verb scheme — see "API versions"). Sharing is immediate to existing
  users — no invitation flow. Membership keys on the **username**,
  distinct from the numeric-id assignee key.
- **User search:** `GET /users?s=` (v1) / `GET /users?q=` (v2) for the
  add-member dialog.

## Testing

`mockito` with canned Vikunja JSON. The v1 suite pins the client to v1
(its mocks use `/api/v1/...` paths verbatim); a v2 suite covers the
envelope pagination (both the stop and the continue direction, plus
`items: null`), the verb swaps (create, update, bucket move, assignee
bulk, shares), the explicit-null merge-patch body and its
`application/merge-patch+json` content type, the strict share bodies
(exact-matched), and the position+limit-preserving bucket rename; the
detection probe has its own tests (JSON vs HTML answers, 404/405,
5xx-no-pin-then-retry). Tests cover list/project mapping, the date
sentinel, the bucket→section fallback, and the assignee/membership
endpoints. Live testing: any Vikunja instance with an API token.
