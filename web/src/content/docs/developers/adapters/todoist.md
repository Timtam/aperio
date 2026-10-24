---
title: "Todoist"
---

**Crate:** `adapter-todoist` · **Capabilities:** tasks

Todoist is a hosted task service. The adapter is **tasks-only** and is the
second collaborative-tasks backend (alongside Vikunja), but Todoist is more
limited: a single assignee per task and email-invite sharing gated behind
the Sync API.

## Authentication

A long-lived personal **API token** (Settings → Integrations → Developer),
sent as a `Bearer` token. The base URL is fixed (REST v2:
`https://api.todoist.com/rest/v2`).

## Data model mapping

- **Projects → task lists** (nest via `parent_id`); **sections** are a
  first-class resource (`GET /sections?project_id=…`).
- **Status** is the boolean `is_completed`, driven via the dedicated
  `/close` and `/reopen` endpoints (the update body doesn't accept it).
- **Priority** is *inverted* relative to the UI (1 = none … 4 = highest).
- **Named colours** map to hex.

## Collaboration

- **Assignee (single):** `assignee_id` on the task — only meaningful in
  *shared* projects. Read accepts the string-or-int wire shape and
  normalises to a string id; names are resolved from the project's
  `GET /projects/{id}/collaborators` (only when something is actually
  assigned, to skip a round-trip for personal projects). On write the
  multi-assignee list is clamped to the first entry (warn on > 1); update
  sends `assignee_id: null` to clear.
- **Member pool:** `GET /projects/{id}/collaborators`.
- **Membership/sharing — Sync API.** REST v2 has no share endpoint, so
  these go through the Sync API v9 (`POST …/sync/v9/sync`): read
  `collaborators` + `collaborator_states` (pending = `state == "invited"`),
  add via `share_project {project_id, email}`, remove via
  `delete_collaborator {project_id, email}`. There are **no roles** and
  membership keys on the **email** (what `delete_collaborator` wants).
  `current_user` is left unset (REST v2 has no `/user`), so "assigned to
  me" highlighting is a follow-up.

## Quirks

- The "list multi-assignee, adapter clamps" model: the UI is multi-select;
  Todoist's adapter keeps the first assignee and warns.
- Sync API replies `200` even on per-command failure — the outcome is in
  `sync_status`, which the adapter inspects.

## A busy server

HTTP 429 is a busy provider (`busy_detail`, decision 270): the request did
nothing. A failed Sync command (`sync_status`, a plan limit such as
`MAX_ITEMS_LIMIT_REACHED` among them) is no throttle and never reads so;
whether Todoist throttles inside `sync_status` is not measured. An update
writes the fields, then moves the task to its section through the Sync API
where the section changed, then closes or reopens it where the update's
answer says the server holds another completion state than the one asked for
(before PR 7 it was sent on every save, so a throttled one made a plain title
edit "only partly saved"). Once the fields landed, a failed move or status
call leaves the save in part (`TodoistError::PartlySaved`, 277); so does a
failed close after a task created as completed. A move that fails while a
status change is still due names `cal_core::STATUS_STEP`, because the status
was not sent either: the hosts then do not count a check-off as done. A busy server is no answer about a project's members:
the share list fails instead of coming back empty.

## Testing

`mockito` with canned REST v2 *and* Sync `/sync` JSON. Tests cover the
assignee read/write/clamp, collaborator name resolution, and the
membership Sync commands (success + error). Live caveats: confirm the
`assignee_id` string-vs-int on write and the exact Sync command shapes
against a real shared project.
