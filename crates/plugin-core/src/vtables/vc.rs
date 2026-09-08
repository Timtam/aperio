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
    /// Size of this struct as the plugin built it, in bytes.
    ///
    /// The field that makes appending a slot survivable. A host reads it,
    /// copies that many bytes into a zeroed struct of its own, and finds
    /// `None` in every slot the plugin did not have — instead of reading
    /// past the end of the plugin's struct and calling whatever followed.
    ///
    /// Only meaningful when [`Self::vtable_version`] is at least
    /// [`crate::ABI_VERSION_STRUCT_SIZE`]. Before that revision these four
    /// bytes were padding, and padding is indeterminate: a garbage value
    /// there that happened to exceed the host's size would reintroduce the
    /// exact hazard this closes.
    ///
    /// It occupies padding that was already there on every 64-bit target,
    /// so no slot moved and no vtable grew when it was added.
    pub struct_size: u32,

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

    /// `delete_meeting(MeetingRemoval) -> ()` — drop the meeting
    /// on the provider side.
    ///
    /// The argument is an object, not a bare id, as of ABI 3:
    /// `{id, notify_attendees}`. Taking a meeting down is also a
    /// question about the people who were invited to it, and on a
    /// calendar that cannot cancel server-side the provider's mail
    /// is the only word they get. `notify_attendees` carries
    /// `#[serde(default)]`, so a later field costs nothing.
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

// SAFETY: `#[repr(C)]`, opens with `vtable_version` then `struct_size`, and
// holds nothing after them but `Option<VtableMethodFn>` slots — so an all-zero
// tail reads as `None`, which is what "the plugin does not implement this"
// already means everywhere else. The revision-3 size is the one below.
unsafe impl crate::vtables::ForeignVtable for VcVtable {
    // Revision 3 shipped 6 slots behind the leading `u32`, and the four
    // bytes that are now `struct_size` were padding it already had: 56
    // bytes. A literal, not `size_of::<Self>()` — see `read_vtable`. It stays
    // 56 when a slot is appended, because appending does not change a
    // struct that already shipped.
    const REVISION_3_SIZE: usize = 56;
}

// The invariant `read_vtable` relies on to stay in bounds. Appending a slot
// keeps it true; shrinking the struct, or mistyping the size above, does not.
const _: () = assert!(
    <VcVtable as crate::vtables::ForeignVtable>::REVISION_3_SIZE <= std::mem::size_of::<VcVtable>()
);

impl VcVtable {
    pub const fn empty() -> Self {
        Self {
            vtable_version: crate::ABI_VERSION,
            struct_size: std::mem::size_of::<Self>() as u32,
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

    #[test]
    fn a_removal_names_the_meeting_and_says_whether_to_tell_anyone() {
        // The wire shape a plugin decodes on the delete slot. `id` is the key
        // the adapter addresses the provider with; getting it wrong deletes
        // nothing or, worse, something else.
        let json = serde_json::to_value(vc_core::MeetingRemoval::new("m1", true)).unwrap();
        assert_eq!(json["id"], "m1");
        assert_eq!(json["notify_attendees"], true);
    }

    #[test]
    fn an_older_removal_without_the_flag_still_decodes_as_silence() {
        // `notify_attendees` carries `#[serde(default)]` so a caller that
        // predates it — or a later field nobody sends yet — costs nothing. The
        // absent answer is silence, which cannot contradict anything.
        let removal: vc_core::MeetingRemoval = serde_json::from_str(r#"{"id":"m1"}"#).unwrap();
        assert_eq!(removal.id, "m1");
        assert!(!removal.notify_attendees);
    }
}
