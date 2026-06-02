# Example: hello-world

The smallest plugin that loads and is recognised by the host: a
calendar-adapter that supports the `calendars` capability and returns an
empty calendar list. Copy this, then grow it.

## Layout

```text
hello-world/
├── plugin.json
├── Cargo.toml
└── src/
    └── lib.rs
```

## `plugin.json`

```json
{
  "id": "com.example.hello-world",
  "name": "Hello World",
  "version": "0.1.0",
  "plugin_type": "calendar-adapter",
  "capabilities": ["calendars"],
  "abi_version": 2,
  "min_app_version": "0.1.0",
  "author": "You",
  "description": "Minimal example calendar adapter.",
  "signed": false
}
```

## `Cargo.toml`

```toml
[package]
name = "hello-world"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
cal-core = { path = "../../crates/cal-core" }
plugin-sdk = { path = "../../crates/plugin-sdk" }
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

## `src/lib.rs`

```rust
use std::os::raw::{c_char, c_void};

use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Calendar, CalendarFeature, Capability,
    Credentials, DateRange, Event, NewEvent, Result,
};
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::{CalendarAdapterVtable, CalendarVtable};
use plugin_sdk::{ok_response, open_instance_with, PluginInstance};

plugin_sdk::cal_dispatch_helpers!(HelloAdapter);

// ── The adapter ───────────────────────────────────────────────
pub struct HelloAdapter;

#[async_trait]
impl Adapter for HelloAdapter {
    async fn authenticate(&self, _c: Credentials) -> Result<AuthToken> {
        Ok(AuthToken::default())
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Calendar]
    }
}

#[async_trait]
impl CalendarFeature for HelloAdapter {
    async fn list_calendars(&self) -> Result<Vec<Calendar>> {
        Ok(vec![]) // ← a real adapter returns the provider's calendars
    }
    async fn get_events(&self, _cal: &str, _r: DateRange) -> Result<Vec<Event>> {
        Ok(vec![])
    }
    // create/update/delete event etc. fall back to cal-core defaults
    // (Unsupported) until you implement them.
    async fn create_event(&self, _cal: &str, _e: NewEvent) -> Result<Event> {
        Err(cal_core::Error::Unsupported("read-only example".into()))
    }
}

// ── Lifecycle ─────────────────────────────────────────────────
/// # Safety: FFI export.
pub unsafe extern "C" fn plugin_open_instance(
    _config: *const c_char,
) -> OpenInstanceResult {
    open_instance_with(_config, |_json| Ok(HelloAdapter))
}

/// # Safety: `handle` from `plugin_open_instance`.
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<HelloAdapter>::drop_handle(handle);
}

// ── ABI glue ──────────────────────────────────────────────────
unsafe extern "C" fn ffi_capabilities(
    h: *mut c_void, _a: *const u8, _l: usize,
) -> PluginCallResult {
    let inst = match instance(h) { Ok(i) => i, Err(r) => return r };
    let caps: Vec<Capability> =
        cal_core::Adapter::capabilities(inst.plugin()).to_vec();
    ok_response(&caps)
}

unsafe extern "C" fn ffi_list_calendars(
    h: *mut c_void, _a: *const u8, _l: usize,
) -> PluginCallResult {
    dispatch(h, |p| async move { p.list_calendars().await })
}

pub static CALENDAR_VTABLE: CalendarVtable = CalendarVtable {
    capabilities: Some(ffi_capabilities),
    list_calendars: Some(ffi_list_calendars),
    ..CalendarVtable::empty()
};

pub static ADAPTER_VTABLE: CalendarAdapterVtable = CalendarAdapterVtable {
    vtable_version: plugin_sdk::plugin_core::ABI_VERSION,
    calendar: &CALENDAR_VTABLE,
    tasks: std::ptr::null(),
    contacts: std::ptr::null(),
};

plugin_sdk::declare_lifecycle! {
    id: "com.example.hello-world",
    name: "Hello World",
    version: "0.1.0",
    plugin_type: "calendar-adapter",
    vtable: ADAPTER_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}
```

> This is illustrative — exact helper names (`instance`, `ok_response`,
> `dispatch`) come from `plugin-sdk`; check the bundled `cal-adapter-*-plugin`
> crates for the current spelling, which is the authoritative reference.

## Build & try

```sh
cargo build --release
```

Package the resulting shared library with `plugin.json` into a `.aperio`
archive, install it in the app's plugin settings, and you'll see an account
that lists no calendars — your foothold for a real adapter.
