//! Per-feature vtable structs (DESIGN.md §20.3).
//!
//! Each vtable is a `#[repr(C)]` struct of `Option<VtableMethodFn>`
//! pointers — one per method on the corresponding cal-core /
//! sync-core trait. The pointer is wrapped in [`Option`] for the
//! same reason the lifecycle `init` / `destroy` are: a plugin
//! that doesn't implement a method can leave the slot `None` and
//! the host's shim wrapper returns `cal_core::Error::Unsupported`
//! (or the sync equivalent) verbatim — exactly the same UX as the
//! existing default-Unsupported trait methods.
//!
//! Every method takes JSON-encoded arguments (a const-pointer + len
//! pair) and returns a [`super::PluginCallResult`]. See
//! [`super::ffi`] for the full ownership + threading rules.
//!
//! ## Vtable layout by plugin_type
//!
//! The single `AperioPlugin.vtable: *mut c_void` slot is cast
//! based on `plugin_type`:
//!
//! - `"calendar-adapter"` → [`CalendarAdapterVtable`] (the multi-
//!   capability wrapper that bundles up to three sub-vtables).
//! - `"sync-adapter"` → [`SyncVtable`] directly (sync plugins
//!   are single-capability by definition).
//! - Other plugin types: reserved for future phases (vc-adapter,
//!   notification).
//!
//! Pure-calendar / pure-tasks / pure-contacts plugins still go
//! through [`CalendarAdapterVtable`] — they just leave the
//! sub-vtable slots they don't implement at `null`. That keeps
//! the host's casting logic uniform: every calendar-adapter
//! plugin has the same outer shape.

use crate::ffi::PluginCallResult;

/// Method-pointer type used by every vtable slot. Takes JSON
/// args (pointer + length; may be `(NULL, 0)` for void-arg
/// methods) and returns a [`PluginCallResult`].
///
/// The shim wrappers wrap each call in `tokio::task::spawn_blocking`
/// so a slow plugin can't stall the async runtime. Sync-shape
/// trait methods (e.g. `CalendarFeature::calendar_color`) call
/// these directly — the plugin's implementation is expected to
/// answer from in-memory state without IO.
pub type VtableMethodFn = unsafe extern "C" fn(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult;

pub mod calendar;
pub mod contacts;
pub mod sync;
pub mod tasks;

pub use calendar::CalendarVtable;
pub use contacts::ContactsVtable;
pub use sync::SyncVtable;
pub use tasks::TasksVtable;

/// Multi-capability outer vtable for `plugin_type = "calendar-adapter"`.
///
/// Aperio's calendar-adapter trait split (DESIGN.md §10.2) puts
/// calendar / tasks / contacts on three separate traits, and the
/// big real-world adapters (CalDAV+CardDAV, Google, Microsoft
/// Graph, EWS) all implement at least two of them on the same
/// adapter instance. The plugin ABI has a single
/// `AperioPlugin.vtable: *mut c_void`, so we wrap the three
/// sub-vtable pointers inside this one struct.
///
/// `null` for any of the three sub-vtable pointers means
/// "capability not provided" — the host's
/// [`super::shim::FfiCalendarAdapter::new`] / `FfiTasksAdapter::new`
/// / `FfiContactsAdapter::new` returns `None` for those, and the
/// registry skips them. The plugin's manifest `capabilities`
/// array MUST match the non-null pointers here; the host
/// cross-checks at load time so a mismatch surfaces as a clear
/// plugin-author error.
///
/// Layout MUST stay binary-compatible across plugin-core 0.x
/// patch versions — adding a new sub-vtable slot is an ABI bump.
#[repr(C)]
pub struct CalendarAdapterVtable {
    /// Conventional bump indicator — same value as
    /// [`crate::ABI_VERSION`]. Detects a misaligned partial
    /// header revision before any cast goes wrong.
    pub vtable_version: u32,
    /// Calendar surface. Null when the plugin doesn't declare
    /// `Capability::Calendar`.
    pub calendar: *const CalendarVtable,
    /// Tasks surface. Null when the plugin doesn't declare
    /// `Capability::Tasks`.
    pub tasks: *const TasksVtable,
    /// Contacts surface. Null when the plugin doesn't declare
    /// `Capability::Contacts`.
    pub contacts: *const ContactsVtable,
}

// SAFETY: all three sub-vtable pointers point at `static`
// instances in the plugin's library data segment. They live for
// the lifetime of the loaded library and contain only fn-pointer
// fields (themselves into the library's code segment). Concurrent
// reads across threads are safe; we never write through these
// pointers.
unsafe impl Send for CalendarAdapterVtable {}
unsafe impl Sync for CalendarAdapterVtable {}

impl CalendarAdapterVtable {
    /// All-null wrapper. Plugin authors construct one of these
    /// as a `static` + fill in only the sub-vtables they actually
    /// implement, leaving the rest at `null`.
    pub const fn empty() -> Self {
        Self {
            vtable_version: crate::ABI_VERSION,
            calendar: std::ptr::null(),
            tasks: std::ptr::null(),
            contacts: std::ptr::null(),
        }
    }

    /// True iff at least one sub-vtable is provided. A plugin
    /// where all three are null is degenerate — the host refuses
    /// to wrap it.
    pub fn has_any_surface(&self) -> bool {
        !self.calendar.is_null()
            || !self.tasks.is_null()
            || !self.contacts.is_null()
    }
}

impl std::fmt::Debug for CalendarAdapterVtable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CalendarAdapterVtable")
            .field("vtable_version", &self.vtable_version)
            .field("calendar_present", &!self.calendar.is_null())
            .field("tasks_present", &!self.tasks.is_null())
            .field("contacts_present", &!self.contacts.is_null())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_outer_vtable_has_no_surface() {
        let v = CalendarAdapterVtable::empty();
        assert!(!v.has_any_surface());
        assert!(v.calendar.is_null());
        assert!(v.tasks.is_null());
        assert!(v.contacts.is_null());
        assert_eq!(v.vtable_version, crate::ABI_VERSION);
    }

    #[test]
    fn populated_outer_vtable_reports_surface() {
        static CAL: CalendarVtable = CalendarVtable::empty();
        let v = CalendarAdapterVtable {
            vtable_version: crate::ABI_VERSION,
            calendar: &CAL,
            tasks: std::ptr::null(),
            contacts: std::ptr::null(),
        };
        assert!(v.has_any_surface());
    }
}
