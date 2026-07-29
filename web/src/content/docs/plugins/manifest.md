---
title: "The `plugin.json` manifest"
---

Every plugin ships a `plugin.json` next to its shared library. The host
reads it to discover the plugin before loading any code.

## Full example

```json
{
  "id": "com.aperio.cal-adapter-todoist",
  "name": "Aperio Todoist",
  "version": "0.1.0",
  "plugin_type": "adapter",
  "capabilities": ["tasks"],
  "abi_version": 3,
  "min_app_version": "0.1.0",
  "author": "Aperio Contributors",
  "description": "Bundled tasks adapter for Todoist (REST API v2).",
  "signed": false,
  "tasks": {
    "nested_projects": true,
    "sections": true,
    "manageable_sections": true,
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
| `plugin_type` | string | ✅ | `"adapter"` for every provider surface, or `"notification"`. There is no per-surface type — what a plugin does is its `capabilities`. |
| `capabilities` | string[] | ✅ | Which feature surfaces the plugin fills: any combination of `"calendar"`, `"tasks"`, `"contacts"`, `"sync"`, `"videoconference"`. Must match the non-null pointers in the vtable exactly; the host checks at load time. An adapter that declares none is rejected. |
| `abi_version` | number | ✅ | The ABI the plugin was built against (current: `3`). Must equal the host's exactly — see [ABI versions and how to migrate](/plugins/abi-versions/). |
| `min_app_version` | string | ✅ | Lowest app version that can load this plugin. |
| `author` | string | ✅ | Author/maintainer. |
| `description` | string | ✅ | One-line description shown in plugin settings. |
| `signed` | boolean | ✅ | Whether the plugin is signed (bundled plugins are `false`). |
| `adapter_kind` | string | — | The value accounts of this adapter carry in the `adapter_kind` column, e.g. `"caldav"`, `"webex"`. Set it if your plugin has accounts; the host builds its kind→plugin map from these. See below. |
| `account` | object | — | What the plugin needs in order to have an account: the fields to ask for, which are secrets, and whether it signs in via OAuth. See below. |

## Capability detail blocks

Optional per-capability objects refine what the UI offers. They default
permissively when absent, so you only set what differs.

### `tasks`

| Field | Meaning |
|---|---|
| `nested_projects` | Task lists nest into a tree (`TaskList.parent_id`). |
| `sections` | Tasks group into sections within a list. |
| `manageable_sections` | The adapter can create/rename/delete sections at the source (`create_section`/`update_section`/`delete_section`). Section *color* is a separate, always-local override, so it doesn't need this. |
| `subtasks` | Tasks can carry subtasks. |
| `move_between_projects` | A task can move to a different list. |
| `create_lists` / `delete_lists` | The adapter can create/delete task lists at the source. |
| `manageable` | The adapter supports managing list membership/sharing (shows the "manage members" UI). |
| `member_add_by` | How members are added when `manageable`: `"search"` (directory search, e.g. Vikunja) or `"email"` (raw-email invite, e.g. Todoist). |

A calendar-capability block exists analogously for things like recurrence
support. Set only the flags that apply; everything else falls back to the
cal-core-native default.

## `adapter_kind` — the routing key

An account row records which adapter it belongs to as a short string, and that
string is what routes every read and write back to your plugin. Declare the one
your accounts use:

```json
"adapter_kind": "webex"
```

The host holds no list of these. It asks the loaded plugins, so an adapter it
has never seen routes exactly like a bundled one.

Two constraints worth knowing:

- **It is not the plugin id.** The kind is written into every account row and
  travels in every sync payload, so it has to stay byte-stable for the life of
  the data; the plugin id may change when a plugin is renamed or re-homed. You
  are free to use your id as your kind — nothing stops you — but they are two
  separate promises, and only one of them is forever.
- **Two values are reserved:** `local` (the built-in store) and
  `device_calendar` (the device's own calendar over a native bridge). Neither is
  a plugin.

An account whose kind no plugin serves is not an error. It lists like any other
and shows as "plugin missing" — which is exactly what a user sees on a device
that has not installed your plugin yet.

## `account` — the connect form

Declare what connecting an account needs, and the host does the rest: it renders
the form on both platforms, collects the values, runs the sign-in, keeps secrets
out of the account row, and hands your plugin back the init config it asked for.

```json
"account": {
  "fields": [
    { "key": "server_url", "kind": "url", "label": "Server URL", "required": true },
    { "key": "username", "kind": "text", "label": "User name", "required": true },
    { "key": "password", "kind": "secret", "secret_slot": "password",
      "label": "Password", "required": true }
  ]
}
```

### Fields

| Field | Meaning |
|---|---|
| `key` | Identifier, and the key this value appears under in your plugin's init config. A non-secret field is persisted in `config_json` under the same key. |
| `kind` | `"text"`, `"url"`, `"secret"` or `"bool"`. Drives the input type — including the on-screen keyboard on mobile, which is why `url` is worth distinguishing. |
| `label` | Your own string. Used verbatim when the app has no translation, which is the normal case for a third-party plugin. |
| `label_key` | Optional translation key the app resolves in the user's language; it takes precedence over `label`. Bundled adapters set this so their strings live in the app's locale files. |
| `hint` / `hint_key` | Optional explanatory line under the field. Same arrangement. |
| `required` | Whether the form refuses to submit without it. |
| `default` | Starting value — a boolean for `bool`, a string otherwise. |
| `secret_slot` | Present exactly when the field is a secret. One of `access_token`, `refresh_token`, `password`, `api_token`, `oauth_client_secret`. |

**A field is a secret only together with its slot, and a secret never reaches
`config_json`.** That column is documented as non-secret and is appended to the
sync event log unencrypted whenever end-to-end encryption is off, so a secret
there would travel to the user's own sync target in the clear. Both directions
are checked when your manifest loads: a `secret` without a slot and a slot on a
non-secret field both fail the load, with a message saying why.

The cross-device encryption key is **not** in the slot list, and not because it
is filtered — there is no name for it. A manifest cannot ask for it.

### OAuth

Add an `oauth` block when connecting means signing in. The host drives the flow
— system browser on the desktop, a native auth session on mobile, and the
two-phase exchange in between — and knows nothing about your provider.
Endpoints, scopes and the token exchange are yours, behind your
`aperio_plugin_interactive_auth` export.

```json
"oauth": {
  "builtin_provider": "webex",
  "client_id_field": "client_id",
  "client_secret_field": "client_secret",
  "refresh_token_field": "refresh_token"
}
```

| Field | Meaning |
|---|---|
| `client_id_field` | Which declared field holds the client id. Must not be a secret — a client id travels in every authorization URL anyway, and the host needs it in the row to record which registration an account belongs to. |
| `client_secret_field` | Which field holds the client secret, for providers that require one. Omit for a public PKCE client. Must be a secret in the `oauth_client_secret` slot. |
| `refresh_token_field` | Init-config key the refresh token is merged under at open time. Omit if you do not want one kept. |
| `access_token_field` | The same for the access token. Usually omitted: an adapter that re-mints on first use would rather not be handed one the host has to keep fresh. |
| `builtin_provider` | Names a credential set the app build may carry for your provider. Optional. |
| `app_redirect_uri` | The redirect the host hands you where it cannot run a loopback listener — mobile. Defaults to `aperio://oauth-callback`. |

When `builtin_provider` is set **and** the build carries that credential, the
two client fields become optional: leaving both blank signs in with the app's
own registration, and the account records only *which* one it was (a marker plus
a short fingerprint), never the credential. Half a pair — an id typed with the
secret left blank — is refused rather than quietly completed.

### `host_channel`

```json
"host_channel": true
```

Set it if your instances need a capability token to report a rotated credential
back for the host to persist. Off unless asked for: the token is authority, and
authority nobody requested is authority nobody audited.

## Where the host looks

The bundled plugins keep their manifest at
`crates/cal-adapter-*-plugin/plugin.json`, `crates/vc-adapter-*-plugin/` and
`crates/sync-adapter-*-plugin/` — the crate names still say what each one is
for, even though the manifests no longer do. A distributed plugin ships the
manifest inside its `.aperio` archive alongside the shared library.
