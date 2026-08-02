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
//! ## One outer vtable
//!
//! The single `AperioPlugin.vtable: *mut c_void` slot always points at an
//! [`AdapterVtable`] — one struct, one per-family pointer each, `null` for
//! every family the plugin does not serve. There is nothing to cast on:
//! whatever the plugin is for, the host reads the same shape.
//!
//! It used to depend on `plugin_type`: a calendar adapter pointed at a
//! three-pointer wrapper, a sync adapter at a bare `SyncVtable`, a
//! videoconference adapter at a bare `VcVtable`. That made the tag load-bearing
//! for memory safety — read the pointer as the wrong struct and the host calls
//! whatever `.rodata` follows — and it made "this provider does calendars AND
//! sync" unrepresentable, because a plugin only gets one vtable slot. Both
//! problems have the same fix, and it is this one.

use crate::ffi::PluginCallResult;

/// Method-pointer type used by every vtable slot. Takes the
/// opaque per-instance handle the host got from
/// [`crate::abi::AperioPlugin::open_instance`], followed by JSON
/// args (pointer + length; may be `(NULL, 0)` for void-arg
/// methods), and returns a [`PluginCallResult`].
///
/// `instance` is the handle the descriptor's `open_instance`
/// returned for this account. May be NULL for instance-less
/// plugins (e.g. process-global notification channels) whose
/// descriptor left `open_instance` itself at None.
///
/// The shim wrappers wrap each call in `tokio::task::spawn_blocking`
/// so a slow plugin can't stall the async runtime. Sync-shape
/// trait methods (e.g. `CalendarFeature::calendar_color`) call
/// these directly — the plugin's implementation is expected to
/// answer from in-memory state without IO.
pub type VtableMethodFn = unsafe extern "C" fn(
    instance: *mut std::os::raw::c_void,
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult;

pub mod calendar;
pub mod contacts;
pub mod sync;
pub mod tasks;
pub mod vc;

pub use calendar::CalendarVtable;
pub use contacts::ContactsVtable;
pub use sync::SyncVtable;
pub use tasks::TasksVtable;
pub use vc::VcVtable;

/// The outer vtable every plugin points at — one provider, however many
/// surfaces.
///
/// One pointer per feature family, `null` for each family this plugin does not
/// serve. A calendar-only adapter fills `calendar` and leaves the other four
/// null; a sync backend fills `sync`; a provider that does both fills both, out
/// of ONE library, which is the whole point — a Google account is a calendar,
/// an address book, a task list, a file store to sync into and a meeting
/// service, and splitting that across four plugins means four OAuth
/// registrations and four sign-ins for one credential.
///
/// The manifest's `capabilities` array MUST match the non-null pointers here.
/// The host cross-checks at load time, so a mismatch surfaces as a plugin-author
/// error rather than a surface that silently answers `Unsupported`.
///
/// Several surfaces does NOT mean one instance serving them all: the host still
/// calls `open_instance` per role, because a calendar account and a sync target
/// are configured differently and neither is the other's business.
///
/// Layout is frozen for an ABI revision. Appending a family pointer is an ABI
/// bump — the host has no per-vtable length, so a plugin built against a shorter
/// layout has to be kept out entirely rather than read past its end.
#[repr(C)]
pub struct AdapterVtable {
    /// Same value as [`crate::ABI_VERSION`], read before the rest of the layout
    /// is trusted. Detects a plugin built against a different revision of this
    /// struct before any pointer in it is followed.
    pub vtable_version: u32,
    /// Calendar surface. Null unless `capabilities` names `calendar`.
    pub calendar: *const CalendarVtable,
    /// Tasks surface. Null unless `capabilities` names `tasks`.
    pub tasks: *const TasksVtable,
    /// Contacts surface. Null unless `capabilities` names `contacts`.
    pub contacts: *const ContactsVtable,
    /// Sync backend. Null unless `capabilities` names `sync`.
    pub sync: *const SyncVtable,
    /// Videoconference surface. Null unless `capabilities` names
    /// `videoconference`.
    pub videoconference: *const VcVtable,
}

// SAFETY: every family pointer points at a `static` in the plugin's library data
// segment. They live for the lifetime of the loaded library and contain only
// fn-pointer fields (themselves into the library's code segment). Concurrent
// reads across threads are safe; we never write through these pointers.
unsafe impl Send for AdapterVtable {}
unsafe impl Sync for AdapterVtable {}

impl AdapterVtable {
    /// All-null. Plugin authors write one of these as a `static` and fill in
    /// only the families they serve:
    ///
    /// ```ignore
    /// pub static ADAPTER_VTABLE: AdapterVtable = AdapterVtable {
    ///     calendar: &CALENDAR_VTABLE,
    ///     ..AdapterVtable::empty()
    /// };
    /// ```
    pub const fn empty() -> Self {
        Self {
            vtable_version: crate::ABI_VERSION,
            calendar: std::ptr::null(),
            tasks: std::ptr::null(),
            contacts: std::ptr::null(),
            sync: std::ptr::null(),
            videoconference: std::ptr::null(),
        }
    }

    /// True iff it serves at least one family. All-null is degenerate and the
    /// host refuses to wrap it.
    pub fn has_any_surface(&self) -> bool {
        !self.calendar.is_null()
            || !self.tasks.is_null()
            || !self.contacts.is_null()
            || !self.sync.is_null()
            || !self.videoconference.is_null()
    }

    /// Copy the pointers out, so a caller can hold them without holding a
    /// borrow of the plugin's library data segment.
    ///
    /// Not `Clone`: copying raw pointers is exactly the thing worth spelling
    /// out at the call site, and they stay valid only as long as the library is
    /// loaded — which the caller guarantees by keeping its `LoadedInstance`
    /// alive.
    pub fn clone_shallow(&self) -> Self {
        Self {
            vtable_version: self.vtable_version,
            calendar: self.calendar,
            tasks: self.tasks,
            contacts: self.contacts,
            sync: self.sync,
            videoconference: self.videoconference,
        }
    }
}

impl std::fmt::Debug for AdapterVtable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterVtable")
            .field("vtable_version", &self.vtable_version)
            .field("calendar_present", &!self.calendar.is_null())
            .field("tasks_present", &!self.tasks.is_null())
            .field("contacts_present", &!self.contacts.is_null())
            .field("sync_present", &!self.sync.is_null())
            .field("videoconference_present", &!self.videoconference.is_null())
            .finish()
    }
}

/// Layout revision of a plugin-supplied vtable, as the FIRST field of every
/// one of them.
///
/// The C header has always promised that "the host gates this on
/// `vtable_version` / `abi_version`". Until now only `abi_version` was checked,
/// which left the promise unkept in the one place it matters: appending a slot
/// to an existing vtable. A plugin built against the shorter layout still
/// declares `abi_version = 2` and passes the loader, and the host would then
/// read past the end of the plugin's struct and CALL whatever `.rodata`
/// happened to follow it.
///
/// So the shims read it now. `vtable_version` sits at offset 0 in every vtable
/// in every revision — it is the one field that is safe to read before the
/// layout is known — and anything other than the current value means the host
/// cannot know how many slots are really there.
///
/// # The rule for the next slot append
///
/// Appending a slot to any vtable — including a family pointer on
/// [`AdapterVtable`] — requires bumping [`crate::ABI_VERSION`]. Strict equality
/// on the manifest then keeps an older plugin out entirely, which is the only
/// safe answer while the host has no per-vtable length.
///
/// A future revision may put a `u32 struct_size` in the four bytes of padding
/// that follow this field on 64-bit targets, at which point a host could read a
/// longer plugin's shorter prefix safely. That field must only ever be trusted
/// when `vtable_version` carries a NEW value: padding bytes in a plugin built
/// before the field existed are indeterminate, and a garbage value there that
/// happened to exceed the host's size would reintroduce exactly the hazard this
/// gate closes.
pub fn vtable_layout_ok(vtable_version: u32) -> bool {
    vtable_version == crate::ABI_VERSION
}

/// Every capability the manifest declares must have a non-null pointer behind
/// it in the vtable the plugin actually ships.
///
/// Run at load time so a plugin that promises more than it implements is
/// refused with its own name on the message. Without it the mismatch surfaces
/// much later and much worse: the account registers, the surface silently
/// isn't there, and the user sees an account with no task lists and nothing
/// anywhere saying why. That failure got easier to hit the moment one plugin
/// could declare five families instead of one.
///
/// The reverse — a pointer with no capability declared — is left alone. It
/// means the plugin implements something it does not offer, which costs the
/// user nothing and may be a surface being staged before its manifest entry.
///
/// Unknown (forward-compat) capabilities are skipped: this host has no slot to
/// look for, and a plugin built for a later Aperio is allowed to name one.
pub fn check_declared_surfaces(
    manifest: &crate::PluginManifest,
    vtable: *const std::os::raw::c_void,
) -> crate::PluginResult<()> {
    let declared: Vec<&crate::Capability> = manifest
        .capabilities
        .iter()
        .filter(|c| c.is_known())
        .collect();
    if declared.is_empty() {
        return Ok(());
    }
    if vtable.is_null() {
        return Err(crate::PluginError::Manifest(format!(
            "{} declares capabilities {:?} but ships no vtable",
            manifest.id,
            declared.iter().map(|c| c.as_str()).collect::<Vec<_>>()
        )));
    }
    // SAFETY: `vtable_version` is at offset 0 of every vtable in every
    // revision — the one field readable before the layout is known.
    let version = unsafe { *(vtable as *const u32) };
    if !vtable_layout_ok(version) {
        return Err(crate::PluginError::Manifest(format!(
            "{} ships a vtable of layout revision {version}, but this host reads {}",
            manifest.id,
            crate::ABI_VERSION
        )));
    }
    // SAFETY: the revision matches, so the struct has this host's layout, and
    // the ABI contract makes every plugin's vtable an `AdapterVtable`.
    let table = unsafe { &*(vtable as *const AdapterVtable) };
    for cap in declared {
        let present = match cap {
            crate::Capability::Calendar => !table.calendar.is_null(),
            crate::Capability::Tasks => !table.tasks.is_null(),
            crate::Capability::Contacts => !table.contacts.is_null(),
            crate::Capability::Sync => !table.sync.is_null(),
            crate::Capability::Videoconference => !table.videoconference.is_null(),
            crate::Capability::Unknown(_) => true,
        };
        if !present {
            return Err(crate::PluginError::Manifest(format!(
                "{} declares capability `{}` but its vtable slot is null",
                manifest.id,
                cap.as_str()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_outer_vtable_has_no_surface() {
        let v = AdapterVtable::empty();
        assert!(!v.has_any_surface());
        assert!(v.calendar.is_null());
        assert!(v.tasks.is_null());
        assert!(v.contacts.is_null());
        assert!(v.sync.is_null());
        assert!(v.videoconference.is_null());
        assert_eq!(v.vtable_version, crate::ABI_VERSION);
    }

    #[test]
    fn populated_outer_vtable_reports_surface() {
        static CAL: CalendarVtable = CalendarVtable::empty();
        let v = AdapterVtable {
            calendar: &CAL,
            ..AdapterVtable::empty()
        };
        assert!(v.has_any_surface());
    }

    /// One library behind two families — the shape that was unrepresentable
    /// while each plugin type had its own outer struct.
    #[test]
    fn one_vtable_can_carry_a_data_family_and_a_sync_backend() {
        static CAL: CalendarVtable = CalendarVtable::empty();
        static SYNC: SyncVtable = SyncVtable::empty();
        let v = AdapterVtable {
            calendar: &CAL,
            sync: &SYNC,
            ..AdapterVtable::empty()
        };
        assert!(v.has_any_surface());
        assert!(!v.calendar.is_null());
        assert!(!v.sync.is_null());
        assert!(v.tasks.is_null());
    }

    fn manifest_with(caps: Vec<crate::Capability>) -> crate::PluginManifest {
        crate::PluginManifest {
            id: "com.example.two-families".to_string(),
            name: "Two Families".to_string(),
            version: "0.1.0".to_string(),
            plugin_type: crate::PluginType::Adapter,
            capabilities: caps,
            abi_version: crate::ABI_VERSION,
            min_app_version: "0.1.0".to_string(),
            author: None,
            description: None,
            signed: false,
            recurrence: Default::default(),
            tasks: Default::default(),
            account: None,
            adapter_kind: None,
            adopts_adapter_kinds: Vec::new(),
            strings: Default::default(),
        }
    }

    #[test]
    fn a_promise_without_a_pointer_is_refused_by_name() {
        static CAL: CalendarVtable = CalendarVtable::empty();
        let table = AdapterVtable {
            calendar: &CAL,
            ..AdapterVtable::empty()
        };
        let ptr = &table as *const AdapterVtable as *const std::os::raw::c_void;

        // What it ships is what it declared.
        check_declared_surfaces(&manifest_with(vec![crate::Capability::Calendar]), ptr)
            .expect("calendar is there");

        // …and one it did not: the message has to carry both the plugin and
        // the missing family, because the user-visible symptom is an absence.
        let err = check_declared_surfaces(
            &manifest_with(vec![crate::Capability::Calendar, crate::Capability::Tasks]),
            ptr,
        )
        .expect_err("tasks is null");
        let msg = err.to_string();
        assert!(msg.contains("com.example.two-families"), "{msg}");
        assert!(msg.contains("tasks"), "{msg}");
    }

    #[test]
    fn a_pointer_without_a_promise_is_left_alone() {
        // The reverse mismatch costs the user nothing — the surface is simply
        // never asked for — so it is not an error.
        static CAL: CalendarVtable = CalendarVtable::empty();
        static SYNC: SyncVtable = SyncVtable::empty();
        let table = AdapterVtable {
            calendar: &CAL,
            sync: &SYNC,
            ..AdapterVtable::empty()
        };
        check_declared_surfaces(
            &manifest_with(vec![crate::Capability::Calendar]),
            &table as *const AdapterVtable as *const std::os::raw::c_void,
        )
        .expect("undeclared sync slot is not an error");
    }

    #[test]
    fn a_capability_from_a_future_aperio_is_skipped_not_refused() {
        static CAL: CalendarVtable = CalendarVtable::empty();
        let table = AdapterVtable {
            calendar: &CAL,
            ..AdapterVtable::empty()
        };
        check_declared_surfaces(
            &manifest_with(vec![
                crate::Capability::Calendar,
                crate::Capability::Unknown("holograms".into()),
            ]),
            &table as *const AdapterVtable as *const std::os::raw::c_void,
        )
        .expect("this host has no slot to look for, so it cannot judge");
    }

    /// ABI sync tripwire (64-bit).
    ///
    /// Each `#[repr(C)]` vtable is `u32 vtable_version` + N
    /// pointer-sized method slots, so its size pins the slot count:
    /// `8 + N*8`. The C mirror `include/aperio_plugin_vtables.h` MUST
    /// list the same slots in the same order. If one of these
    /// assertions fails you added or removed an FFI slot — update the
    /// header to match (and bump the expected size here). This keeps
    /// the hand-maintained header from silently drifting from the Rust
    /// source of truth.
    #[test]
    fn the_layout_gate_accepts_only_the_current_revision() {
        // Every bundled plugin stamps ABI_VERSION, so this accepts them all.
        // What it catches is a hand-written plugin built against a stale
        // header: it would still declare the right `abi_version` and pass the
        // loader, and the host would then read past the end of its vtable.
        assert!(vtable_layout_ok(crate::ABI_VERSION));
        assert!(!vtable_layout_ok(crate::ABI_VERSION - 1));
        assert!(!vtable_layout_ok(crate::ABI_VERSION + 1));
        assert!(!vtable_layout_ok(0));
    }

    #[test]
    fn every_vtable_constructor_stamps_the_current_revision() {
        // The gate is only as good as the constructors: one that forgot to
        // stamp the field would lock its own plugin out.
        assert!(vtable_layout_ok(CalendarVtable::empty().vtable_version));
        assert!(vtable_layout_ok(TasksVtable::empty().vtable_version));
        assert!(vtable_layout_ok(ContactsVtable::empty().vtable_version));
        assert!(vtable_layout_ok(SyncVtable::empty().vtable_version));
        assert!(vtable_layout_ok(VcVtable::empty().vtable_version));
        assert!(vtable_layout_ok(AdapterVtable::empty().vtable_version));
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn vtable_sizes_match_c_header() {
        use std::mem::size_of;
        // 14 method slots: the original 12 + RSVP (current_user_email,
        // respond_to_event).
        assert_eq!(size_of::<CalendarVtable>(), 8 + 14 * 8);
        // 22 slots: 12 task methods + assignee support (2:
        // list_task_list_members, current_user) + membership management
        // (5: list_task_list_shares, search_users, add/remove member,
        // set_member_right) — DESIGN §9.7 — + section CRUD (3:
        // create/update/delete_section).
        assert_eq!(size_of::<TasksVtable>(), 8 + 22 * 8);
        // 14 method slots.
        assert_eq!(size_of::<ContactsVtable>(), 8 + 14 * 8);
        // 10 method slots.
        assert_eq!(size_of::<SyncVtable>(), 8 + 10 * 8);
        // 6 method slots: the original 4 plus ABI 3's resolve_meeting +
        // list_meetings. Appending to an existing vtable is what forced the ABI
        // bump — the host has no per-vtable length, so strict equality on the
        // manifest is the only thing that keeps a plugin built against the
        // shorter layout from being read past its end.
        assert_eq!(size_of::<VcVtable>(), 8 + 6 * 8);
        // u32 + one pointer per feature family: calendar, tasks, contacts,
        // sync, videoconference.
        assert_eq!(size_of::<AdapterVtable>(), 8 + 5 * 8);
    }
}
