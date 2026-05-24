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
//! ## Anatomy of a plugin
//!
//! A typical Rust plugin (single-feature calendar adapter):
//!
//! ```ignore
//! use async_trait::async_trait;
//! use cal_core::{Adapter, CalendarFeature, /* … */};
//! use plugin_sdk::{declare_lifecycle, response, PluginSingleton};
//!
//! struct MyAdapter { /* state */ }
//!
//! #[async_trait]
//! impl Adapter for MyAdapter { /* … */ }
//!
//! #[async_trait]
//! impl CalendarFeature for MyAdapter { /* … */ }
//!
//! static INSTANCE: PluginSingleton<MyAdapter> = PluginSingleton::new();
//!
//! // FFI fn for each trait method:
//! unsafe extern "C" fn list_calendars(
//!     _args: *const u8, _len: usize,
//! ) -> plugin_core::PluginCallResult {
//!     let Some((plugin, rt)) = INSTANCE.parts() else {
//!         return response::error_response(
//!             plugin_core::PLUGIN_CALL_ERR_INTERNAL,
//!             "plugin not initialised",
//!         );
//!     };
//!     match rt.block_on(plugin.list_calendars()) {
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
//! unsafe extern "C" fn plugin_init(_: *const std::os::raw::c_char) -> std::os::raw::c_int {
//!     match INSTANCE.init(MyAdapter::default()) {
//!         Ok(()) => plugin_core::PLUGIN_OK,
//!         Err(_) => plugin_core::PLUGIN_ERR_INIT,
//!     }
//! }
//!
//! declare_lifecycle! {
//!     id: "com.example.mycal",
//!     name: "My Calendar",
//!     version: "1.0.0",
//!     plugin_type: "calendar-adapter",
//!     vtable: CALENDAR_VTABLE,
//!     init: plugin_init,
//!     destroy: none,
//! }
//! ```
//!
//! ## Module map
//!
//! - [`runtime`] — current-thread tokio runtime the plugin uses
//!   to `block_on` its async trait methods inside the
//!   synchronous FFI fn bodies.
//! - [`singleton`] — `OnceLock`-backed holder for the
//!   process-singleton plugin instance + its runtime.
//! - [`response`] — build [`plugin_core::PluginCallResult`]s
//!   (ok / error / empty payloads) the host can decode.
//! - [`error_map`] — translate `cal_core::Error` /
//!   `sync_core::SyncError` variants into the matching
//!   `PLUGIN_CALL_ERR_*` status codes.
//! - [`args`] — decode the host's JSON-encoded args pointer pair
//!   into a typed value.
//! - [`macros`] — `declare_lifecycle!`, the only macro for
//!   now. The bigger `aperio_plugin_export!` that would auto-
//!   build the vtable from trait impls is deliberately deferred
//!   (see [`macros`] module doc for the reasoning).

pub mod args;
pub mod error_map;
pub mod macros;
pub mod response;
pub mod runtime;
pub mod singleton;

// Plugin authors import everything they need from the SDK so
// they don't have to add plugin-core to their own Cargo.toml.
pub use plugin_core;

pub use args::decode_args;
pub use error_map::{cal_error_to_response, sync_error_to_response};
pub use response::{
    bytes_to_response, error_response, free_boxed_slice, ok_empty_response,
    ok_response,
};
pub use runtime::PluginRuntime;
pub use singleton::{InitError, PluginSingleton};
