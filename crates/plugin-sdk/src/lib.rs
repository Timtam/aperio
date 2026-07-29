//! `plugin-sdk` — ergonomic Rust API for plugin authors
//! (DESIGN.md §20.11).
//!
//! Aperio's plugin ABI is C-flavoured (a struct of fn pointers
//! that take JSON-encoded args + return JSON-encoded responses).
//! Rust plugin authors don't have to wrestle with that surface
//! directly: this crate provides the wrappers + macros that turn
//! a normal `impl CalendarFeature for MyAdapter { … }` block
//! into a loadable plugin with minimal boilerplate.
//!
//! ## Anatomy of a plugin (ABI v2)
//!
//! A typical Rust plugin (single-feature calendar adapter):
//!
//! ```ignore
//! use async_trait::async_trait;
//! use cal_core::{Adapter, CalendarFeature, /* … */};
//! use plugin_sdk::{declare_lifecycle, response, PluginInstance};
//!
//! struct MyAdapter { /* state */ }
//!
//! #[async_trait]
//! impl Adapter for MyAdapter { /* … */ }
//!
//! #[async_trait]
//! impl CalendarFeature for MyAdapter { /* … */ }
//!
//! /// Per-account open hook. The host calls this once per
//! /// account it wants to wire up with this adapter.
//! unsafe extern "C" fn plugin_open_instance(
//!     config_json: *const std::os::raw::c_char,
//! ) -> plugin_core::OpenInstanceResult {
//!     plugin_sdk::open_instance_with(config_json, |json| {
//!         let cfg: MyConfig = serde_json::from_str(json)
//!             .map_err(|e| e.to_string())?;
//!         Ok(MyAdapter::new(cfg))
//!     })
//! }
//!
//! /// Per-account close hook. Just reclaims the box the SDK
//! /// produced in open_instance.
//! unsafe extern "C" fn plugin_close_instance(
//!     handle: *mut std::os::raw::c_void,
//! ) {
//!     PluginInstance::<MyAdapter>::drop_handle(handle);
//! }
//!
//! // FFI fn for each trait method:
//! unsafe extern "C" fn list_calendars(
//!     instance: *mut std::os::raw::c_void,
//!     _args: *const u8,
//!     _len: usize,
//! ) -> plugin_core::PluginCallResult {
//!     let Some(inst) =
//!         (unsafe { PluginInstance::<MyAdapter>::from_handle(instance) })
//!     else {
//!         return response::error_response(
//!             plugin_core::PLUGIN_CALL_ERR_INTERNAL_FFI,
//!             "null instance",
//!         );
//!     };
//!     match inst.runtime().block_on(inst.plugin().list_calendars()) {
//!         Ok(cals) => response::ok_response(&cals),
//!         Err(err) => plugin_sdk::cal_error_to_response(err),
//!     }
//! }
//!
//! static CALENDAR_VTABLE: plugin_core::CalendarVtable = plugin_core::CalendarVtable {
//!     list_calendars: Some(list_calendars),
//!     ..plugin_core::CalendarVtable::empty()
//! };
//!
//! declare_lifecycle! {
//!     id: "com.example.mycal",
//!     name: "My Calendar",
//!     version: "1.0.0",
//!     plugin_type: "calendar-adapter",
//!     vtable: CALENDAR_VTABLE,
//!     open_instance: plugin_open_instance,
//!     close_instance: plugin_close_instance,
//! }
//! ```
//!
//! ## Module map
//!
//! - [`runtime`] — current-thread tokio runtime the plugin uses
//!   to `block_on` its async trait methods inside the
//!   synchronous FFI fn bodies.
//! - [`instance`] — `PluginInstance<T>` boxed handle the host
//!   stores and threads back into every vtable method.
//! - [`open_instance`] — helper that turns a typed open closure
//!   into the [`plugin_core::OpenInstanceResult`] the C ABI
//!   expects, including UTF-8 error messages on failure.
//! - [`response`] — build [`plugin_core::PluginCallResult`]s
//!   (ok / error / empty payloads) the host can decode.
//! - [`error_map`] — translate `cal_core::Error` /
//!   `sync_core::SyncError` variants into the matching
//!   `PLUGIN_CALL_ERR_*` status codes.
//! - [`args`] — decode the host's JSON-encoded args pointer pair
//!   into a typed value.
//! - [`interactive_auth`] / [`discover`] / [`probe_host_key`] —
//!   optional FFI entry points for OAuth flows, service-discovery
//!   cascades, and TOFU host-key fingerprint probes.
//! - [`macros`] — `declare_lifecycle!`, `declare_interactive_auth!`,
//!   `declare_discover!`, `declare_probe_host_key!`.

pub mod args;
pub mod discover;
pub mod dispatch;
pub mod error_map;
pub mod host_channel;
pub mod instance;
pub mod interactive_auth;
pub mod log_forward;
pub mod macros;
pub mod open_instance;
pub mod probe_host_key;
pub mod response;
pub mod runtime;
pub mod strings;

// Plugin authors import everything they need from the SDK so
// they don't have to add plugin-core to their own Cargo.toml.
pub use plugin_core;

pub use args::decode_args;
pub use discover::discover_with;
pub use dispatch::{
    cal_dispatch, cal_dispatch_unit, instance, sync_dispatch, sync_dispatch_unit, vc_dispatch,
    vc_dispatch_unit,
};
pub use error_map::{cal_error_to_response, sync_error_to_response, vc_error_to_response};
pub use instance::{InitError, PluginInstance};
pub use interactive_auth::interactive_auth_with;
pub use open_instance::{error_result, open_instance_with};
pub use probe_host_key::probe_host_key_with;
pub use response::{
    bytes_to_response, error_response, free_boxed_slice, ok_empty_response, ok_response,
};
pub use runtime::PluginRuntime;
