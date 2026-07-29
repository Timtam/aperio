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

    // ── ABI 3 ──────────────────────────────────────────────────
    /// `resolve_meeting(join_url) -> Option<Meeting>` — the
    /// meeting a join link belongs to.
    ///
    /// The link is the only identifier that reaches the calendar:
    /// it travels in the event, where every client can read it,
    /// while the provider's own meeting id does not. Without this
    /// the host can only manage meetings it created ITSELF and
    /// still remembers locally — not one made in the provider's
    /// own UI, not one made by another device, not one an
    /// invitation brought in.
    ///
    /// NULL when the provider has no lookup by link. Not every
    /// one does, which is why this is a slot and not a
    /// requirement.
    pub resolve_meeting: Option<VtableMethodFn>,

    /// `list_meetings(DateRange) -> Vec<Meeting>` — the account's
    /// scheduled meetings in a window.
    ///
    /// Lets the host surface meetings that have no calendar entry
    /// at all — the ones created straight in the provider's web
    /// UI, which otherwise exist only there and are invisible in
    /// a calendar app.
    ///
    /// NULL when the provider cannot enumerate; the host then
    /// simply offers no such view.
    pub list_meetings: Option<VtableMethodFn>,
}

impl VcVtable {
    pub const fn empty() -> Self {
        Self {
            vtable_version: crate::ABI_VERSION,
            test_connection: None,
            create_meeting: None,
            get_meeting: None,
            delete_meeting: None,
            resolve_meeting: None,
            list_meetings: None,
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
