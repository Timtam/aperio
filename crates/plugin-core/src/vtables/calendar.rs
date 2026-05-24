//! `CalendarVtable` — mirrors `cal_core::CalendarFeature`.
//!
//! Every method takes JSON-encoded args (input pointer + length)
//! and returns a [`super::super::PluginCallResult`]. The shim
//! wrapper [`super::super::shim::FfiCalendarAdapter`] is the
//! canonical consumer: it serialises the typed arguments to JSON,
//! calls the FFI fn through `tokio::task::spawn_blocking`, frees
//! the returned payload, and deserialises the response — or maps
//! the non-zero status into a `cal_core::Error` variant.
//!
//! JSON shape per method
//! ─────────────────────
//! Args are encoded as a single JSON object whose keys mirror the
//! Rust parameter names. Empty-args methods take a JSON `null` or
//! an empty buffer (the plugin SHOULD accept either).
//!
//! Responses match what the trait method returns:
//!   - `list_calendars` → `Vec<Calendar>` as a JSON array.
//!   - `get_events` → `Vec<Event>`.
//!   - `create_event` / `update_event` → `Event`.
//!   - `delete_event` / `add_event_exdate` / `rename_calendar` →
//!     `null` (empty payload also accepted).
//!   - `get_free_busy` → `Vec<FreeBusy>`.
//!   - `calendar_color` is SYNCHRONOUS — it just looks up a value
//!     the plugin already has in memory — and so it stays out of
//!     the JSON path: response is the JSON-encoded
//!     `Option<ContainerColor>`.
//!   - `authenticate` → `AuthToken`; args carry `Credentials`.
//!   - `capabilities` → `Vec<Capability>` (JSON array of the
//!     lowercase capability names).
//!
//! See `crates/cal-core/src/adapter.rs` for the trait surface
//! these mirror.

use std::os::raw::c_int;

use super::VtableMethodFn;

/// Vtable for `plugin_type = "calendar-adapter"` plugins
/// (DESIGN.md §20.3).
///
/// Capabilities matter here too: a plugin that only declares
/// `["calendar"]` in its manifest fills `list_calendars` and the
/// event/free-busy/rename methods but leaves the contact + task
/// slots `None`. (Those plugins ship a `TasksVtable` and/or
/// `ContactsVtable` instead — multi-capability adapters publish
/// multiple vtables in parallel via the SDK macros.)
///
/// Layout MUST stay binary-compatible across plugin-core 0.x patch
/// versions; new methods are appended at the end + their absence
/// is signalled by the host detecting an older `abi_version`.
#[repr(C)]
#[derive(Debug)]
pub struct CalendarVtable {
    /// Conventional bump indicator local to this vtable. Equals
    /// [`crate::ABI_VERSION`] for plugins built against the current
    /// header; we keep it here too so a partial header revision
    /// (new method appended to one vtable only) can be detected
    /// without touching the global ABI version.
    pub vtable_version: u32,

    // ── Base Adapter methods ────────────────────────────────────
    /// `authenticate(Credentials) -> AuthToken`. May be `None`
    /// for adapters that don't need a separate auth dance (the
    /// local adapter, for example).
    pub authenticate: Option<VtableMethodFn>,

    /// `capabilities() -> Vec<Capability>`. Sync — answered by
    /// the plugin from static state. MAY be `None` and the host
    /// will fall back to the manifest's `capabilities` field.
    pub capabilities: Option<VtableMethodFn>,

    // ── Calendar trait methods ─────────────────────────────────
    pub list_calendars: Option<VtableMethodFn>,
    pub get_events: Option<VtableMethodFn>,
    pub create_event: Option<VtableMethodFn>,
    pub update_event: Option<VtableMethodFn>,
    pub delete_event: Option<VtableMethodFn>,
    pub get_free_busy: Option<VtableMethodFn>,
    /// `calendar_color(calendar_id) -> Option<ContainerColor>`.
    /// Sync method (see [`VtableMethodFn`]).
    pub calendar_color: Option<VtableMethodFn>,
    /// `add_event_exdate(event_id, occurrence)`. Default-
    /// `Unsupported` on most adapters — leave `None` and the
    /// shim returns `Error::Unsupported` automatically.
    pub add_event_exdate: Option<VtableMethodFn>,
    /// `rename_calendar(calendar_id, new_name)`. Same default-
    /// `Unsupported` story.
    pub rename_calendar: Option<VtableMethodFn>,
}

impl CalendarVtable {
    /// Build an "all-None" vtable. Useful in tests + as a starting
    /// point for the SDK macro's code generator — the macro fills
    /// in only the slots the plugin actually implements, leaving
    /// the rest at their `None` defaults so the shim wrapper sees
    /// `Error::Unsupported` for the others.
    pub const fn empty() -> Self {
        Self {
            vtable_version: crate::ABI_VERSION,
            authenticate: None,
            capabilities: None,
            list_calendars: None,
            get_events: None,
            create_event: None,
            update_event: None,
            delete_event: None,
            get_free_busy: None,
            calendar_color: None,
            add_event_exdate: None,
            rename_calendar: None,
        }
    }

    /// Lightweight sanity check used by the manager at load time:
    /// a calendar-adapter that doesn't fill `list_calendars` is
    /// almost certainly a build mistake (the trait method has no
    /// default impl). Failing fast here gives a clear error
    /// instead of an `Error::Unsupported` deep in some later sync
    /// round.
    pub fn has_minimum_surface(&self) -> bool {
        self.list_calendars.is_some()
    }
}

/// Vtable expected at the pointer fields of `CalendarVtable`. C
/// consumers cast `AperioPlugin.vtable` straight to
/// `const struct CalendarVtable*`; Rust consumers go through
/// `unsafe { &*(ptr as *const CalendarVtable) }`. Either way the
/// layout is what matters — these `pub type` aliases just
/// document the contract.
pub type CalendarVtablePtr = *const CalendarVtable;

/// Status code reserved for "no vtable method registered" — set
/// by the host's shim wrapper when it sees a `None` slot. Lives
/// here (next to the vtable) rather than in [`super::super::ffi`]
/// because it's a host-injected status, never returned by a
/// plugin itself.
pub const VTABLE_SLOT_UNSET: c_int = crate::ffi::PLUGIN_CALL_ERR_UNSUPPORTED;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_vtable_has_no_methods() {
        let v = CalendarVtable::empty();
        assert!(v.authenticate.is_none());
        assert!(v.capabilities.is_none());
        assert!(v.list_calendars.is_none());
        assert!(v.get_events.is_none());
        assert!(v.create_event.is_none());
        assert!(v.update_event.is_none());
        assert!(v.delete_event.is_none());
        assert!(v.get_free_busy.is_none());
        assert!(v.calendar_color.is_none());
        assert!(v.add_event_exdate.is_none());
        assert!(v.rename_calendar.is_none());
        assert_eq!(v.vtable_version, crate::ABI_VERSION);
    }

    #[test]
    fn minimum_surface_requires_list_calendars() {
        let v = CalendarVtable::empty();
        assert!(!v.has_minimum_surface());
    }
}
