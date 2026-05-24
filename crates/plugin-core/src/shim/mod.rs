//! Rust-side shim wrappers that implement cal-core / sync-core
//! traits by dispatching across the FFI boundary.
//!
//! One shim per plugin type. Each shim holds a (cheap) reference
//! to a [`crate::manager::LoadedPlugin`] and the typed vtable
//! pointer it carries, then implements the corresponding feature
//! trait by:
//!
//!   1. Serialising the typed arguments to JSON.
//!   2. Calling the matching FFI fn through `tokio::task::spawn_blocking`
//!      so a slow plugin can't stall the async runtime.
//!   3. Decoding the response bytes (or the error message) back
//!      into Rust types.
//!   4. Releasing the plugin's bytes via the
//!      [`crate::ffi::PluginBytes::free`] function pointer.
//!
//! Status-code mapping is centralised so every shim returns the
//! exact same `cal_core::Error` / `sync_core::SyncError` variant
//! for the same underlying plugin status. See [`call`].
//!
//! ## Scope in P1
//!
//! Only [`FfiCalendarAdapter`] lands here — it's the canonical
//! pattern. The other three shims (Tasks / Contacts / Sync) are
//! P1b work; they mirror this file's structure one-to-one.

pub mod calendar;
mod call;

pub use calendar::FfiCalendarAdapter;
