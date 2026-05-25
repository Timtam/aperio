//! Rust-side shim wrappers that implement cal-core / sync-core
//! traits by dispatching across the FFI boundary.
//!
//! One shim per plugin type. Each shim holds an Arc to a
//! [`crate::manager::LoadedPlugin`] and a snapshot of the typed
//! vtable pointers it carries, then implements the corresponding
//! feature trait by:
//!
//!   1. Serialising the typed arguments to JSON.
//!   2. Calling the matching FFI fn through
//!      `tokio::task::spawn_blocking` so a slow plugin can't
//!      stall the async runtime.
//!   3. Decoding the response bytes (or the error message) back
//!      into Rust types.
//!   4. Releasing the plugin's bytes via the
//!      [`crate::ffi::PluginBytes::free`] function pointer.
//!
//! Status-code mapping is per-shim because the target error type
//! differs: calendar / tasks / contacts shims surface
//! `cal_core::Error`, the sync shim surfaces `sync_core::SyncError`.

pub mod calendar;
mod call;
pub mod contacts;
pub mod sync;
pub mod tasks;
pub mod vc;

pub use calendar::FfiCalendarAdapter;
pub use contacts::FfiContactsAdapter;
pub use sync::FfiSyncAdapter;
pub use tasks::FfiTasksAdapter;
pub use vc::FfiVcAdapter;

/// Project plugin-core's capability tags onto cal-core's
/// [`cal_core::Capability`] enum. Shared by the three calendar-
/// family shims (Calendar / Tasks / Contacts); the sync shim
/// has no capabilities to project.
///
/// Unknown plugin-core capabilities (forward-compat tags from a
/// future Aperio) are dropped — the host doesn't know how to
/// dispatch to them, and the trait's `capabilities()` slot is a
/// list, not an error channel.
pub(super) fn manifest_capabilities(
    raw: &[crate::Capability],
) -> Vec<cal_core::adapter::Capability> {
    raw.iter()
        .filter_map(|c| match c {
            crate::Capability::Calendar => Some(cal_core::adapter::Capability::Calendar),
            crate::Capability::Tasks => Some(cal_core::adapter::Capability::Tasks),
            crate::Capability::Contacts => Some(cal_core::adapter::Capability::Contacts),
            crate::Capability::Unknown(_) => None,
        })
        .collect()
}
