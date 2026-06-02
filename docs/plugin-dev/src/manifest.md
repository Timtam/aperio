# The `plugin.json` manifest

Every plugin ships a `plugin.json` next to its shared library. The host
reads it to discover the plugin before loading any code.

## Full example

```json
{
  "id": "com.aperio.cal-adapter-todoist",
  "name": "Aperio Todoist",
  "version": "0.1.0",
  "plugin_type": "calendar-adapter",
  "capabilities": ["tasks"],
  "abi_version": 2,
  "min_app_version": "0.1.0",
  "author": "Aperio Contributors",
  "description": "Bundled tasks adapter for Todoist (REST API v2).",
  "signed": false,
  "tasks": {
    "nested_projects": true,
    "sections": true,
    "subtasks": true,
    "move_between_projects": false,
    "create_lists": true,
    "delete_lists": true,
    "manageable": true,
    "member_add_by": "email"
  }
}
```

## Top-level fields

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | string | ✅ | Stable reverse-DNS identifier, e.g. `com.example.my-plugin`. The primary key — two plugins can't share it. |
| `name` | string | ✅ | Human-readable display name. |
| `version` | string | ✅ | Plugin version (semver). |
| `plugin_type` | string | ✅ | `"calendar-adapter"`, `"sync-adapter"`, or `"vc-adapter"`. There is **no** separate `task-adapter` type — a tasks-only provider is a `calendar-adapter` with `capabilities: ["tasks"]`. |
| `capabilities` | string[] | ✅ | Which feature surfaces the plugin fills: any of `"calendars"`, `"tasks"`, `"contacts"`. Drives which vtables the host expects to be non-null. |
| `abi_version` | number | ✅ | The ABI the plugin was built against (current: `2`). |
| `min_app_version` | string | ✅ | Lowest app version that can load this plugin. |
| `author` | string | ✅ | Author/maintainer. |
| `description` | string | ✅ | One-line description shown in plugin settings. |
| `signed` | boolean | ✅ | Whether the plugin is signed (bundled plugins are `false`). |

## Capability detail blocks

Optional per-capability objects refine what the UI offers. They default
permissively when absent, so you only set what differs.

### `tasks`

| Field | Meaning |
|---|---|
| `nested_projects` | Task lists nest into a tree (`TaskList.parent_id`). |
| `sections` | Tasks group into sections within a list. |
| `subtasks` | Tasks can carry subtasks. |
| `move_between_projects` | A task can move to a different list. |
| `create_lists` / `delete_lists` | The adapter can create/delete task lists at the source. |
| `manageable` | The adapter supports managing list membership/sharing (shows the "manage members" UI). |
| `member_add_by` | How members are added when `manageable`: `"search"` (directory search, e.g. Vikunja) or `"email"` (raw-email invite, e.g. Todoist). |

A calendar-capability block exists analogously for things like recurrence
support. Set only the flags that apply; everything else falls back to the
cal-core-native default.

## Where the host looks

The bundled plugins keep their manifest at
`crates/cal-adapter-*-plugin/plugin.json`. A distributed plugin ships the
manifest inside its `.aperio` archive alongside the shared library.
