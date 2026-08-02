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
| `adopts_adapter_kinds` | string[] | — | Kinds written by an adapter this plugin has absorbed, so the rows keep resolving here. Resolution only — never offered as its own entry. See below. |
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

## `adopts_adapter_kinds` — taking over another adapter's rows

Two adapters sometimes become one: a provider you served with two plugins turns
out to be one account with two capabilities. The problem is the rows that are
already out there. A kind is written into every account row and travels in every
sync payload, so renaming it means rewriting rows on one device and propagating
the rewrite — across devices that are offline, devices on an older version, and
devices where the plugin is not installed at all.

Declare instead that you now serve the old kind:

```json
"adapter_kind": "acme",
"adopts_adapter_kinds": ["acme-drive"]
```

Nothing that was written down changes. `adapter_kind` is what accounts created
from now on carry; the adopted kinds are what existing ones already carry, and
they keep resolving to you.

What you take on with them:

- **Your `open` must accept the config those rows were written with.** The host
  does not translate between the two shapes and could not — it does not know
  what your fields mean. Accept both, or migrate inside your own plugin where
  you know what a value means.
- **Adopted kinds are for resolution, not for offering.** They still appear
  wherever the app DESCRIBES accounts — an account that already carries one has
  to stay visible, groupable and repairable — but never where it CREATES one.
  The Add-account picker and the sync-target form both skip them, so a merged
  adapter is offered once, under one name. They need no display name in the
  app's locale files.
- **Delete the plugin you adopted from.** While both are installed the one that
  declares the kind as its own keeps it, so the adopting half serves nothing —
  and which one a user's accounts bind to depends on what they have installed.

The manifest is rejected at load time if an adopted kind is blank, is listed
twice, repeats your own `adapter_kind`, or appears without one.

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
| `kind` | One of the seven below. Drives the control the user gets — including the on-screen keyboard on mobile, which is why `url` is worth distinguishing from `text`. |
| `label` | Your own string. Used verbatim when the app has no translation, which is the normal case for a third-party plugin. |
| `label_key` | Optional translation key the app resolves in the user's language; it takes precedence over `label`. Bundled adapters set this so their strings live in the app's locale files. |
| `hint` / `hint_key` | Optional explanatory line under the field. Same arrangement. |
| `required` | Whether the form refuses to submit without it. |
| `default` | Starting value — a boolean for `bool`, a string otherwise. A `number` default is a string too (`"22"`), and it must parse. |
| `secret_slot` | Present exactly when the field is a secret. One of `access_token`, `refresh_token`, `password`, `api_token`, `oauth_client_secret`, `key_passphrase`. |
| `options` | The choices, for `kind: "choice"`. Each is `{ "value", "label" }` with an optional `label_key`. Rejected on any other kind. |
| `min` / `max` | Accepted range, for `kind: "number"`. Rejected on any other kind. |
| `device_local` | Whether this value stays on the machine that entered it. See below. |

### Kinds

| Kind | Control | Notes |
|---|---|---|
| `text` | Single-line text | The default. |
| `url` | Single-line text | Mobile gets a URL keyboard. |
| `secret` | Masked | Requires — and is required by — `secret_slot`. |
| `bool` | Checkbox | Its value is a JSON boolean. |
| `number` | Numeric entry | Reaches you as a JSON **number**, not as text. |
| `choice` | Closed list | Requires `options`. |
| `directory` / `file` | Path entry | A path on the user's machine. Mark these `device_local`. |

Two of those are worth a paragraph.

**`number` exists because both frontends produce strings.** If your init config
declares `port: u16`, serde will not accept `"22"` — and the failure is the
whole struct, so your adapter never opens and the user sees a deserialisation
error naming a Rust type instead of the field they typed in. Declaring `number`
makes the host convert before you see it, and reject a non-number where the
message can name the field. Declare `min` and `max` as well: only you know that
a port is a `u16`, and without the bound `70000` still reaches you.

**`choice` exists because several adapters do not reject a value outside their
set.** A free-text box lets a typo pick a different transport, and the
connection then behaves differently than the user asked with nothing said. The
set belongs in your manifest, where the host can enforce it.

### Values that stay on one device

An account row travels between a user's devices. Most of what it carries
travels well — a server address, a user name, a client id, the name of a folder
in someone's cloud storage. Some values do not: an SSH private key lives at
`/home/anna/.ssh/id_ed25519` on one machine and somewhere else entirely on the
next, and a folder on a local disk means nothing anywhere else.

Mark those `device_local: true`. They are kept out of the synced part of the
account and stored per device instead; the host merges them back into your init
config before your instance is opened, so you read them under the same key
either way. On a device that has not configured the account, they are simply
absent — give those fields a serde default, or handle their absence.

Only you can answer this. The host cannot tell a filesystem path from a URL by
looking, and guessing wrong is costly in both directions: too eager and the
user retypes settings on every device they own, too shy and one machine's paths
overwrite another's.

Secrets are already device-local by a different route — they live in the
platform keychain, never in the account row — so there is no need to mark them.

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
| `client_secret_field` | Which field holds the client secret, for providers that require one. Omit for a public PKCE client. Must be a secret in the `oauth_client_secret` slot. That field's `required` flag also decides what opening an account does when the keychain holds no client secret: a required one refuses, an optional one opens without it. A keychain that will not *answer* is refused either way. |
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
