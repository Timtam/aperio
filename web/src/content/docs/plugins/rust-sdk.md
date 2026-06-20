---
title: "The Rust SDK"
---

`plugin-sdk` lets you write a plugin as ordinary safe Rust: you implement
the `cal-core` traits, and a few macros generate the `extern "C"` glue, the
JSON marshalling, and the lifecycle exports. You should rarely touch a raw
pointer.

This mirrors how the bundled adapters (e.g. `cal-adapter-todoist-plugin`)
are written — read one of those crates alongside this page.

## The shape of a plugin crate

```rust
use std::os::raw::{c_char, c_void};

use cal_core::{Adapter, TasksFeature /*, CalendarFeature, … */};
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::{CalendarAdapterVtable, TasksVtable};
use plugin_sdk::{decode_args, open_instance_with, PluginInstance};

use my_adapter::MyAdapter; // your trait impl from the library crate

// Generates `dispatch` / `dispatch_unit` helpers bound to your type.
plugin_sdk::cal_dispatch_helpers!(MyAdapter);
```

### 1. Lifecycle: open / close

```rust
#[derive(serde::Deserialize)]
struct InitConfig { token: String }

/// # Safety: FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(
    config_json: *const c_char,
) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig = serde_json::from_str(json)
            .map_err(|e| format!("bad config: {e}"))?;
        Ok(MyAdapter::new(cfg.token))
    })
}

/// # Safety: `handle` must be from `plugin_open_instance`.
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<MyAdapter>::drop_handle(handle);
}
```

### 2. One `ffi_*` wrapper per method

Each wrapper decodes its args, then calls your async trait method through
`dispatch` (for a value result) or `dispatch_unit` (for `()`):

```rust
unsafe extern "C" fn ffi_get_tasks(
    h: *mut c_void, a: *const u8, l: usize,
) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,   // marshalling error → straight back to host
    };
    dispatch(h, move |p| async move { p.get_tasks(&list_id).await })
}

// Multiple args → a small #[derive(Deserialize)] struct:
#[derive(serde::Deserialize)]
struct CreateTaskArgs { list_id: String, task: cal_core::NewTask }

unsafe extern "C" fn ffi_create_task(
    h: *mut c_void, a: *const u8, l: usize,
) -> PluginCallResult {
    let args: CreateTaskArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.create_task(&args.list_id, args.task).await
    })
}
```

`decode_args` does the JSON decode; `dispatch`/`dispatch_unit` run your
async method on the host's runtime and encode the result (or error).

### 3. Assemble the vtables

Fill the slots you implement; `..Vtable::empty()` leaves the rest `None`.
**Append** new slots at the end — never reorder.

```rust
pub static TASKS_VTABLE: TasksVtable = TasksVtable {
    authenticate: Some(ffi_authenticate),
    capabilities: Some(ffi_capabilities),
    list_task_lists: Some(ffi_list_task_lists),
    get_tasks: Some(ffi_get_tasks),
    create_task: Some(ffi_create_task),
    // … the methods you support …
    ..TasksVtable::empty()
};

pub static ADAPTER_VTABLE: CalendarAdapterVtable = CalendarAdapterVtable {
    vtable_version: plugin_sdk::plugin_core::ABI_VERSION,
    calendar: std::ptr::null(),     // not a calendar provider
    tasks: &TASKS_VTABLE,
    contacts: std::ptr::null(),
};
```

### 4. Declare it

```rust
plugin_sdk::declare_lifecycle! {
    id: "com.example.my-plugin",
    name: "My Plugin",
    version: "0.1.0",
    plugin_type: "calendar-adapter",
    vtable: ADAPTER_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}
```

## What you get for free

- **Safety:** you write `async fn` trait methods returning
  `cal_core::Result<T>`; the SDK turns `Err(Error::Unsupported(…))` etc.
  into the wire error contract.
- **Defaults:** trait methods you don't implement use `cal-core`'s defaults
  (e.g. `search_users` → empty, `add_task_list_member` → `Unsupported`), so
  you only wire the slots you actually support.
- **Forward compatibility:** when the host adds a new trait method/slot, an
  unrecompiled plugin simply has `None` there and the host falls back.
- **Logging:** your `tracing` / `log` output is forwarded to the host log
  automatically — the SDK exports `aperio_plugin_set_log` and installs a
  forwarding subscriber, so `warn!`/`info!`/… land in `aperio.log` with no
  extra code. See [Logging](/plugins/abi-reference/#logging).
