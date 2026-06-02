# Vikunja

**Crate:** `cal-adapter-vikunja` · **Capabilities:** tasks

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
- **Priority** 0–5 collapses to Aperio's Low/Medium/High.

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
