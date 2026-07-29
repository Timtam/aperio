---
title: "The C ABI"
---

The host loads a plugin as a shared library and calls into it through a
stable, versioned C ABI defined in the `plugin-core` crate, mirrored by the
hand-maintained C headers `aperio_plugin.h` (lifecycle + exported symbols) and
`aperio_plugin_vtables.h` (the capability vtables). You normally interact with
it through the [Rust SDK](/plugins/rust-sdk/); this chapter documents the
contract itself.

## Lifecycle

A plugin exports a small set of `extern "C"` symbols the host calls:

- **Discovery** — the host reads `plugin.json` for the id, type,
  capabilities and ABI version.
- **`plugin_open_instance(config_json) -> OpenInstanceResult`** — create an
  instance from a JSON config string (e.g. `{ "token": "…" }`). The host
  threads per-account configuration/secrets in here. Returns an opaque
  handle (a pointer) on success, or an error.
- **`plugin_close_instance(handle)`** — destroy the instance and free its
  resources.
- **`aperio_plugin_set_log(log_fn)`** *(optional)* — the host calls this once,
  right after creation, to hand the plugin a host-side log sink so the
  plugin's diagnostics reach the host log. See [Logging](#logging).

The SDK's `declare_lifecycle!` and `declare_cdylib_exports!` macros generate
these for you (including the metadata and log exports).

## Vtables: one table per capability

The host talks to an instance through `#[repr(C)]` **vtables** — structs of
nullable function pointers (`Option<VtableMethodFn>`). Every plugin points its
single vtable slot at the same outer struct, `AdapterVtable`, with one pointer
per feature family:

```text
AdapterVtable {
    vtable_version: u32,          // = ABI_VERSION
    calendar:        *const CalendarVtable,   // null if unsupported
    tasks:           *const TasksVtable,      // null if unsupported
    contacts:        *const ContactsVtable,   // null if unsupported
    sync:            *const SyncVtable,       // null if unsupported
    videoconference: *const VcVtable,         // null if unsupported
}
```

Fill in the families you serve; leave the rest null. The manifest's
`capabilities` array must name exactly the non-null ones — the host checks at
load time and refuses a plugin that promises a surface it does not ship.

A capability you don't support is a **null pointer**. Within a vtable, a
method you don't implement is a **`None` slot** — the host treats it as
"unsupported" and falls back to a sensible default (e.g. an empty list, or
an `Unsupported` error for an action).

## Method signature

Every method slot has the same shape:

```text
unsafe extern "C" fn(handle: *mut c_void,
                     args:   *const u8,
                     len:    usize) -> PluginCallResult
```

- **`handle`** is the instance pointer from `plugin_open_instance`.
- **`args`** / **`len`** are a serde-JSON byte buffer of the method's
  arguments (multiple args are wrapped in a small struct).
- The return `PluginCallResult` carries either a serde-JSON-encoded success
  value or a structured error.

So **all domain data crosses as serde JSON**. The SDK decodes args and
encodes results for you.

## Memory ownership

- The host owns the `args` buffer it passes in; the plugin must not retain
  it past the call.
- Result buffers the plugin returns are handed back through the
  `PluginCallResult` contract and freed via the agreed mechanism (the SDK
  handles this) — the plugin allocates, the host signals when done.
- The instance handle is owned by the plugin and freed only in
  `plugin_close_instance`.

## Logging

A plugin shared library links its own logging stack, so by default its
diagnostics never reach the host's log (`data/logs/aperio.log`). One optional
export bridges that gap:

```text
void aperio_plugin_set_log(void (*log)(uint8_t level,
                                       const char *target,
                                       const char *message));
```

The host calls `aperio_plugin_set_log` once, right after
`aperio_plugin_create`, handing the plugin a log sink. To forward a log line
the plugin calls `log` with:

- **`level`** — a severity byte: `1` ERROR, `2` WARN, `3` INFO, `4` DEBUG,
  `5` TRACE (the `APERIO_PLUGIN_LOG_LEVEL_*` constants; they mirror
  `tracing::Level`).
- **`target`** — the event's target/module (e.g. `my_adapter::sync`).
- **`message`** — the rendered message.

`target` and `message` are NUL-terminated UTF-8 valid only for the duration of
the call, and the callback must not unwind across the boundary. The host
re-emits each forwarded event into its own log at the same level, under the
target `aperio::plugin`, keeping your original target in a `source` field — so
look for `aperio::plugin` lines (with `source=my_adapter::…`) in `aperio.log`.
The host's own log-level filter then decides what is actually written.

The export is **optional** — like an absent vtable slot. A plugin built before
this ABI addition simply doesn't forward, and the host treats a missing symbol
as "no logging", never a load error. (On the static-linked mobile build the
plugin already shares the host's logging, so the host never calls it.)

**Rust plugins get this for free.** `declare_cdylib_exports!` emits
`aperio_plugin_set_log` and installs a forwarding subscriber, so your ordinary
`tracing` / `log` macros (`warn!`, `info!`, …) reach the host log with no extra
code. **C / other-language plugins** implement the export themselves and call
the supplied function pointer for each line they want forwarded.

## Versioning & compatibility

`vtable_version` is `ABI_VERSION`, and the host reads it before it trusts the
rest of a vtable's layout. The rules:

- **Never reorder or remove** a vtable slot. New methods are **appended**
  to the end, so existing offsets stay stable.
- **Appending a slot to an existing vtable requires an ABI bump.** The host
  has no per-vtable length: it reads your vtable as a struct of the size IT
  was compiled with, so a plugin built against the shorter layout would be
  read past its end. Strict equality on `abi_version` is what prevents that.
  (An earlier version of this page said such a plugin would simply see the
  new slot as `None`. That was never true for an appended slot — only for a
  slot a plugin chooses to leave `NULL` within the same revision.)
- Adding a whole **new** vtable for a **new** plugin type needs no bump:
  nothing reads it unless that type exists.
- Because data is serde JSON, adding a `#[serde(default)]` field to a
  domain type is **ABI-transparent** — it doesn't touch the vtable at all.
  Only a brand-new *method* needs a vtable slot.
- A new optional **named export** needs no bump either. It is looked up by
  symbol at load time and its absence simply means the host asks you to do
  less.

`plugin-core` has a compile-time assertion that the vtable struct sizes
match the C header, so the Rust and C views can't drift.

See [ABI versions and how to migrate](/plugins/abi-versions/) for what each
revision contains and what moving between them costs.

## Other surfaces

Sync backends and videoconference providers use the same lifecycle, the same
JSON-over-FFI mechanics and the same outer vtable — they fill `sync` or
`videoconference` instead of `calendar`. The patterns below generalise, and
one plugin may fill several.
