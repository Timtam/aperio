//! `VcVtable` — mirrors `vc_core::VcAdapter`.
//!
//! Same FFI shape + JSON-arguments convention as the calendar
//! / sync vtables. See `crates/vc-core/src/lib.rs` for the
//! source-of-truth trait the JSON payloads mirror.
//!
//! Video-conference plugins are single-capability by
//! definition — Zoom is just Zoom, Teams is just Teams — so
//! they wear the vtable directly (no `VcAdapterVtable` outer
//! wrapper like the multi-capability calendar shape uses).

use super::VtableMethodFn;

/// Vtable for `plugin_type = "videoconference-adapter"`
/// plugins (DESIGN.md §11 + §20.3).
///
/// Layout MUST stay binary-compatible across plugin-core 0.x
/// patch versions.
#[repr(C)]
#[derive(Debug)]
pub struct VcVtable {
    pub vtable_version: u32,

    // ── VcAdapter methods ──────────────────────────────────────
    /// `test_connection()` — adapter-specific probe. Drives
    /// the AccountsDialog's "Test connection" button.
    pub test_connection: Option<VtableMethodFn>,

    /// `create_meeting(NewMeeting) -> Meeting` — generate a
    /// fresh meeting on the provider side and return the
    /// populated [`vc_core::Meeting`].
    pub create_meeting: Option<VtableMethodFn>,

    /// `get_meeting(MeetingId) -> Option<Meeting>` — re-fetch a
    /// previously-created meeting. `null` response means the
    /// provider no longer has it (soft delete + clear the
    /// host's cached id).
    pub get_meeting: Option<VtableMethodFn>,

    /// `delete_meeting(MeetingId) -> ()` — drop the meeting on
    /// the provider side.
    pub delete_meeting: Option<VtableMethodFn>,
}

impl VcVtable {
    pub const fn empty() -> Self {
        Self {
            vtable_version: crate::ABI_VERSION,
            test_connection: None,
            create_meeting: None,
            get_meeting: None,
            delete_meeting: None,
        }
    }

    /// A vc adapter that can't even create a meeting is
    /// useless — fast-fail at load time rather than at the
    /// first "Generate meeting link" click. `test_connection`
    /// is intentionally not required: some providers don't
    /// have a cheap probe endpoint, in which case the
    /// AccountsDialog's "Test" button just isn't surfaced.
    pub fn has_minimum_surface(&self) -> bool {
        self.create_meeting.is_some() && self.delete_meeting.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_vtable_has_no_methods() {
        let v = VcVtable::empty();
        assert!(v.create_meeting.is_none());
        assert!(v.delete_meeting.is_none());
        assert!(!v.has_minimum_surface());
        assert_eq!(v.vtable_version, crate::ABI_VERSION);
    }
}
