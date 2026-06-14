---
title: "The C ABI"
---

The host loads a plugin as a shared library and calls into it through a
stable, versioned C ABI defined in the `plugin-core` crate (with a mirror
C header, `aperio_plugin_vtables.h`). You normally interact with it through
the [Rust SDK](/plugins/rust-sdk/); this chapter documents the contract itself.

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

The SDK's `declare_lifecycle!` macro generates these plus the metadata
exports.

## Vtables: one table per capability

The host talks to an instance through `#[repr(C)]` **vtables** — structs of
nullable function pointers (`Option<VtableMethodFn>`). The top-level
`CalendarAdapterVtable` carries:

```text
CalendarAdapterVtable {
    vtable_version: u32,          // = ABI_VERSION
    calendar: *const CalendarVtable,   // null if unsupported
    tasks:    *const TasksVtable,      // null if unsupported
    contacts: *const ContactsVtable,   // null if unsupported
}
```

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

## Versioning & compatibility

`vtable_version` is `ABI_VERSION`. The rules that keep old plugins working:

- **Never reorder or remove** a vtable slot. New methods are **appended**
  to the end of a vtable, so existing offsets are stable.
- A method added as a new slot is `None` in plugins that predate it; the
  host detects the null and uses the default behaviour — no recompile of
  old plugins required.
- Because data is serde JSON, adding a `#[serde(default)]` field to a
  domain type is **ABI-transparent** — it doesn't touch the vtable at all.
  Only a brand-new *method* needs a vtable slot.

`plugin-core` has a compile-time assertion that the vtable struct sizes
match the C header, so the Rust and C views can't drift.

## Other plugin types

`sync-adapter` and `vc-adapter` plugins use the same lifecycle and
JSON-over-FFI mechanics with their own vtables. The patterns below
generalise.
