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
  `parenttask` relation (`PUT /tasks/{child}/relations`, removed via
  `DELETE /tasks/{child}/relations/parenttask/{parent}`) maps onto
  `Task.parent_id`; reads take the first non-zero
  `related_tasks.parenttask` entry. Updates reconcile against an
  authoritative `GET /tasks/{id}` — Vikunja populates `related_tasks` only
  on reads, never on the update echo — unlink every stale parent (Vikunja
  allows several), and treat PUT→409 (already linked) / DELETE→404
  (already gone) as success.

## Collaboration (assignment + membership)

- **Assignees** (multi): read inline on the task; written via the bulk
  endpoint `POST …/tasks/{id}/assignees/bulk` (replace-semantics — the full
  desired set). The assignee `TaskUser.id` is the numeric user id.
- **Member pool** for the picker: `GET /projects/{id}/projectusers`.
- **Own identity** ("me"): `GET /user`.
- **Membership/sharing:** `GET /projects/{id}/users` lists direct shares
  *with rights* (read/write/admin → 0/1/2); add/remove/change-right via
  `PUT`/`DELETE`/`POST` on `/projects/{id}/users` (Vikunja quirk: **PUT =
  create, POST = update**). Sharing is immediate to existing users — no
  invitation flow. Membership keys on the **username**, distinct from the
  numeric-id assignee key.
- **User search:** `GET /users?s=` for the add-member dialog.

## Testing

`mockito` with canned Vikunja JSON and the project/page pagination headers.
Tests cover list/project mapping, the date sentinel, the bucket→section
fallback, and the assignee/membership endpoints. Live testing: any Vikunja
instance with an API token.
