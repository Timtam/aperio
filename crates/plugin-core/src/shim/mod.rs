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
/// The outer vtable of a loaded plugin, once it is safe to read.
///
/// Every plugin points its single vtable slot at an
/// [`crate::vtables::AdapterVtable`], so there is nothing to decide here: one
/// null check, one layout check, one cast. Which families the plugin actually
/// serves is then a question about the pointers inside, and every shim asks it
/// the same way.
///
/// `None` when the pointer is null or the layout revision is not this host's.
/// Both mean the same thing to a caller — this plugin cannot be called — and
/// neither is worth a distinct error, because the loader has already rejected
/// the version mismatch loudly and this is the belt to that's braces.
pub(super) fn adapter_vtable(
    plugin: &crate::LoadedPlugin,
) -> Option<crate::vtables::AdapterVtable> {
    let raw = plugin.vtable_ptr();
    if raw.is_null() {
        return None;
    }
    // SAFETY: `vtable_version` sits at offset 0 of every vtable in every
    // revision, so it is the one field readable before the layout is known.
    // Everything past it is read only once the revision matches this host's.
    let version = unsafe { *(raw as *const u32) };
    if !crate::vtables::vtable_layout_ok(version) {
        return None;
    }
    // SAFETY: the revision matches, so the struct behind the pointer has this
    // host's layout, and the ABI contract makes it an `AdapterVtable`.
    Some(unsafe { &*(raw as *const crate::vtables::AdapterVtable) }.clone_shallow())
}

pub(super) fn manifest_capabilities(
    raw: &[crate::Capability],
) -> Vec<cal_core::adapter::Capability> {
    raw.iter()
        .filter_map(|c| match c {
            crate::Capability::Calendar => Some(cal_core::adapter::Capability::Calendar),
            crate::Capability::Tasks => Some(cal_core::adapter::Capability::Tasks),
            crate::Capability::Contacts => Some(cal_core::adapter::Capability::Contacts),
            // Not data families: `cal_core::Capability` describes what a
            // CALENDAR adapter offers, and a sync backend or a meeting service
            // is neither. They reach the host through their own vtables.
            crate::Capability::Sync
            | crate::Capability::Videoconference
            | crate::Capability::Unknown(_) => None,
        })
        .collect()
}
