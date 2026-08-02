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

## Testing

`mockito` with canned REST v2 *and* Sync `/sync` JSON. Tests cover the
assignee read/write/clamp, collaborator name resolution, and the
membership Sync commands (success + error). Live caveats: confirm the
`assignee_id` string-vs-int on write and the exact Sync command shapes
against a real shared project.
