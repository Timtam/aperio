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
/// A family pointer may be APPENDED without an ABI bump. [`Self::struct_size`]
/// tells the host how far a plugin's own struct goes, so one built before the
/// append reads as null there — not offered, rather than locked out. Anything
/// else about the layout is frozen for a revision: reordering, resizing or
/// removing a field is a bump, because a length cannot describe those.
#[repr(C)]
pub struct AdapterVtable {
    /// Same value as [`crate::ABI_VERSION`], read before the rest of the layout
    /// is trusted. Detects a plugin built against a different revision of this
    /// struct before any pointer in it is followed.
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

// SAFETY: `#[repr(C)]`, opens with `vtable_version` then `struct_size`, and
// holds nothing after them but family pointers — so an all-zero tail reads as
// null, which is what "this plugin does not serve that family" already means.
unsafe impl ForeignVtable for AdapterVtable {
    // Revision 3 shipped the same five family pointers behind the leading
    // `u32`, whose four bytes of padding are now `struct_size`: 48 bytes. A
    // literal, not `size_of::<Self>()` — see `read_vtable`. Appending a SIXTH
    // family does not change how big revision 3's struct was.
    const REVISION_3_SIZE: usize = 48;
}

// The invariant `read_vtable` relies on to stay in bounds. Appending a family
// pointer keeps it true; shrinking the struct, or mistyping the size, does not.
const _: () = assert!(
    <AdapterVtable as ForeignVtable>::REVISION_3_SIZE <= std::mem::size_of::<AdapterVtable>()
);

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
            struct_size: std::mem::size_of::<Self>() as u32,
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

/// Is this a vtable layout the host knows how to read?
///
/// `vtable_version` sits at offset 0 of every vtable in every revision, which
/// makes it the one field readable before the layout is known. Everything else
/// — including the `struct_size` beside it — is only trusted once this has said
/// yes, because a revision the host does not know is a revision whose second
/// field it cannot name either.
///
/// The accepted set is a RANGE, `ABI_VERSION_MIN..=ABI_VERSION`, not the
/// current value. Revisions 1 and 2 are outside it and stay out: they predate
/// [`AdapterVtable`] entirely, and what a v2 plugin puts behind its single
/// vtable pointer depends on a `plugin_type` tag this host no longer reads.
/// That is not a length problem and no length fixes it.
///
/// The C header has always promised that "the host gates this on
/// `vtable_version` / `abi_version`". Until ABI 3 only `abi_version` was
/// checked, which left the promise unkept in the one place it mattered:
/// appending a slot to an existing vtable. A plugin built against the shorter
/// layout still declared the `abi_version` the loader wanted, and the host
/// would then read past the end of its struct and CALL whatever `.rodata`
/// happened to follow.
///
/// # Appending a slot
///
/// Append it. That is the whole rule now, and it was not always: while the host
/// had no per-vtable length, a plugin built against a shorter layout was
/// indistinguishable from one built against the current one, so the only safe
/// answer was to refuse anything that did not match exactly. Adding one method
/// meant every plugin in the world had to be rebuilt before it could load —
/// for a method it does not implement.
///
/// [`AdapterVtable::struct_size`] and its siblings ended that. The host copies
/// the bytes the plugin says it has into a zeroed struct of its own, so a slot
/// the plugin never had reads as `None` and is reported as unsupported. See
/// [`read_vtable`].
pub fn vtable_layout_ok(vtable_version: u32) -> bool {
    (crate::ABI_VERSION_MIN..=crate::ABI_VERSION).contains(&vtable_version)
}

/// The two `u32`s every vtable starts with, in every revision: `vtable_version`
/// at offset 0, `struct_size` at offset 4. A plugin claiming to be smaller than
/// this is not describing a vtable at all.
pub(crate) const VTABLE_HEADER_SIZE: usize = 2 * std::mem::size_of::<u32>();

/// A `#[repr(C)]` vtable the host reads out of a plugin's library.
///
/// The associated constant exists because revision 3 — the oldest this host
/// accepts — carries no length of its own. Its sizes are therefore a historical
/// fact the HOST has to remember, not something it may infer from its own
/// struct: infer it, and the first appended slot makes the host read every
/// revision-3 plugin past its end. Write it down instead and appending stays
/// what this ABI promises it is — free.
///
/// # Safety
///
/// An implementor must be `#[repr(C)]`, must begin with `u32 vtable_version`
/// then `u32 struct_size`, and must hold nothing after them but nullable
/// pointer-sized slots, so that an all-zero tail is a valid value meaning
/// "absent". [`REVISION_3_SIZE`](Self::REVISION_3_SIZE) must be this struct's
/// size as ABI revision 3 shipped it, and no larger than its size today.
pub unsafe trait ForeignVtable: Sized {
    /// `size_of::<Self>()` as ABI revision 3 shipped this struct.
    ///
    /// Frozen: it describes a struct that is already out in the world, so it
    /// never changes when a slot is appended. It stops being needed the day
    /// [`crate::ABI_VERSION_MIN`] rises past
    /// [`crate::ABI_VERSION_STRUCT_SIZE`], and can be deleted with the branch
    /// that reads it.
    const REVISION_3_SIZE: usize;
}

/// Why the host would not read a plugin's vtable.
///
/// Two causes, kept apart because they call for opposite fixes and the wrong
/// message sends an author to the wrong field. Zero is by far the likelier of
/// the two in practice: a C designated initializer that names its methods and
/// never mentions `struct_size` silently leaves it zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtableReadError {
    /// `vtable_version` is outside `ABI_VERSION_MIN..=ABI_VERSION`.
    UnknownRevision(u32),
    /// `struct_size` is below the two-`u32` header every revision shares.
    SizeBelowHeader(u32),
}

impl std::fmt::Display for VtableReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownRevision(version) => write!(
                f,
                "vtable_version is {version}; this host reads {} to {}",
                crate::ABI_VERSION_MIN,
                crate::ABI_VERSION
            ),
            Self::SizeBelowHeader(claimed) => write!(
                f,
                "vtable struct_size is {claimed}, below the {VTABLE_HEADER_SIZE}-byte header every vtable has; set it to sizeof the struct"
            ),
        }
    }
}

/// Copy a plugin's vtable into one of this host's, reading only the bytes the
/// plugin says it wrote.
///
/// # Why a copy
///
/// The obvious `&*(raw as *const T)` is undefined behaviour the moment the
/// plugin's struct is shorter than the host's — which is precisely the case
/// this exists to handle. A reference asserts the whole `T` is there. So the
/// host never makes one: it zeroes a `T` of its own, copies the prefix the
/// plugin actually provided, and reads that.
///
/// Zero is the right filler because it is what "absent" already looks like in
/// every field: `Option<VtableMethodFn>` is a null-niche option, so all-zero
/// bytes are `None`, and a family pointer on [`AdapterVtable`] is null. A slot
/// the plugin predates therefore arrives as "not implemented", which is a state
/// every caller already handles.
///
/// # How the length is decided
///
/// - At [`crate::ABI_VERSION_STRUCT_SIZE`] and above, from the plugin's own
///   `struct_size`, clamped to the host's own size. The clamp bounds the WRITE:
///   `struct_size` is a number a foreign library chose, and a plugin that
///   claims more than the host's struct holds — a lie, a mismatched build, a
///   half-written static — must not be able to run this off the end of the
///   host's own local. It is not a forward-compatibility path: a plugin from a
///   LATER revision never reaches here, because [`vtable_layout_ok`] and the
///   manifest gate both refuse anything above [`crate::ABI_VERSION`]. That
///   refusal is deliberate — see below.
/// - Below it, from [`ForeignVtable::REVISION_3_SIZE`] — a written-down
///   historical size, NOT `size_of::<T>()`. Revision 3 states no length, so the
///   host has to supply one, and the only correct one is how big the struct was
///   when that revision shipped. Using the host's current size would be right
///   exactly until the first appended slot, and then it would read every
///   revision-3 plugin past its end and call whatever followed — reintroducing
///   the hazard on the very operation this revision exists to make free.
///   Revision 3's four bytes at offset 4 are padding, and padding is
///   indeterminate: nothing here reads them.
///
/// # Only one direction, and only one is possible
///
/// This buys a NEW host reading an OLD plugin. The reverse — an old host
/// reading a plugin from a later revision — stays refused, and no length can
/// change that: a version number alone cannot tell "revision 5 appended a
/// method" (which a prefix read would survive) from "revision 5 changed what an
/// existing slot's argument means" (which it would not). A host that has never
/// heard of revision 5 cannot know which it is looking at. `min_app_version` in
/// the manifest is the lever for a plugin that needs a newer Aperio: it makes
/// the refusal say "update Aperio" instead of "wrong ABI".
///
/// # Safety
///
/// `raw` must be non-null, aligned for `T`, and point at a vtable a plugin
/// produced, of at least `T::REVISION_3_SIZE` bytes. Every revision this host
/// accepts is at least that big; how much MORE is there is what this answers.
pub unsafe fn read_vtable<T: ForeignVtable>(raw: *const T) -> Result<T, VtableReadError> {
    let head = raw as *const u32;
    let version = unsafe { head.read() };
    if !vtable_layout_ok(version) {
        return Err(VtableReadError::UnknownRevision(version));
    }

    let host_size = std::mem::size_of::<T>();
    let readable = if version >= crate::ABI_VERSION_STRUCT_SIZE {
        let claimed = unsafe { head.add(1).read() };
        // A plugin claiming less than the shared header is describing something
        // that is not a vtable. Refusing beats reading two fields out of it and
        // hoping.
        if (claimed as usize) < VTABLE_HEADER_SIZE {
            return Err(VtableReadError::SizeBelowHeader(claimed));
        }
        (claimed as usize).min(host_size)
    } else {
        // Not `host_size`. See "How the length is decided" above — this is the
        // line an appended slot would turn into an over-read.
        T::REVISION_3_SIZE.min(host_size)
    };

    let mut local = std::mem::MaybeUninit::<T>::zeroed();
    // SAFETY: `readable` is at most `size_of::<T>()`, so the write stays inside
    // `local`; and it is at most what the plugin has, so the read stays inside
    // the plugin's struct. The two regions are distinct allocations, so they
    // cannot overlap.
    unsafe {
        std::ptr::copy_nonoverlapping(raw as *const u8, local.as_mut_ptr() as *mut u8, readable);
        Ok(local.assume_init())
    }
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
    // SAFETY: the ABI contract makes every plugin's vtable an `AdapterVtable`,
    // and `read_vtable` reads only as far as the plugin says it wrote.
    //
    // A reference would be wrong here for the same reason it is wrong in the
    // shims, and less obviously: today revisions 3 and 4 are the same size, so
    // `&*ptr` happens to be sound. The first revision that appends a family
    // pointer makes it undefined behaviour for every plugin built before it —
    // which is the exact moment nobody would be looking at this function.
    let table = match unsafe { read_vtable(vtable as *const AdapterVtable) } {
        Ok(table) => table,
        // The error says which of the two refusals this was. Reporting only the
        // revision would name the one field that is right, and send an author
        // who forgot `struct_size` looking at a version this host accepts.
        Err(why) => {
            return Err(crate::PluginError::Manifest(format!(
                "{}'s vtable cannot be read: {why}",
                manifest.id
            )))
        }
    };
    let table = &table;
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
            kind_names: Default::default(),
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
    fn the_layout_gate_accepts_the_revisions_this_host_can_read() {
        // A range, not a point. Revision 3 is readable because the host knows
        // how big its structs were — `ForeignVtable::REVISION_3_SIZE` — not
        // because 3 and 4 happen to be the same size today.
        assert!(vtable_layout_ok(crate::ABI_VERSION));
        assert!(vtable_layout_ok(crate::ABI_VERSION_MIN));
        // Above the range is a layout that did not exist when this host was
        // built: the host cannot say where its slots are, or whether an
        // existing one changed meaning.
        assert!(!vtable_layout_ok(crate::ABI_VERSION + 1));

        // The floor is asserted as LITERALS, not as `ABI_VERSION_MIN - 1`.
        // Written relatively, both ends hold for any floor — including 1 — so
        // lowering the constant would let revision-1 and revision-2 vtables in
        // while these assertions stayed green. Those two predate `AdapterVtable`
        // entirely: what a v2 plugin puts behind its vtable pointer depends on a
        // `plugin_type` tag this host no longer reads. That is not a length
        // problem and no length fixes it.
        assert!(!vtable_layout_ok(0));
        assert!(!vtable_layout_ok(1));
        assert!(!vtable_layout_ok(2));
    }

    /// A buffer shaped like a vtable a plugin wrote, 8-aligned, with a header
    /// this host may or may not know and `slots` fn-pointer-sized words after
    /// it — each byte non-zero, so anything read past what the header claims
    /// shows up as a bogus non-null pointer rather than as a tidy `None`.
    fn foreign_vtable(version: u32, struct_size: u32, slots: usize) -> Vec<u64> {
        let mut words = vec![0xAAAA_AAAA_AAAA_AAAAu64; 1 + slots];
        words[0] = (version as u64) | ((struct_size as u64) << 32);
        words
    }

    /// The sizes revision 3 shipped, as literals. Nothing here derives them
    /// from `size_of`, because deriving them is the bug: the moment a slot is
    /// appended, a derived number describes the NEW struct while a real
    /// revision-3 plugin is still the old size, and the host reads past its end.
    #[test]
    fn the_revision_3_sizes_are_the_sizes_revision_3_shipped() {
        use crate::vtables::ForeignVtable;
        // 8 bytes of header (`vtable_version` plus the padding that is now
        // `struct_size`) and one 8-byte slot each. Slot counts as of ABI 3.
        assert_eq!(CalendarVtable::REVISION_3_SIZE, 8 + 14 * 8);
        assert_eq!(TasksVtable::REVISION_3_SIZE, 8 + 22 * 8);
        assert_eq!(ContactsVtable::REVISION_3_SIZE, 8 + 14 * 8);
        assert_eq!(SyncVtable::REVISION_3_SIZE, 8 + 10 * 8);
        assert_eq!(VcVtable::REVISION_3_SIZE, 8 + 6 * 8);
        assert_eq!(AdapterVtable::REVISION_3_SIZE, 8 + 5 * 8);
    }

    #[test]
    fn a_plugin_from_the_previous_revision_is_read_at_the_size_that_revision_had() {
        use crate::vtables::ForeignVtable;
        // Revision 3 wrote no length: those four bytes were padding, and this
        // stands in for padding that happens to hold 16 — a plausible leftover,
        // and small enough that trusting it would visibly truncate the read.
        // The host must ignore it and use the size it recorded for that
        // revision.
        let slots = (VcVtable::REVISION_3_SIZE - 8) / 8;
        let raw = foreign_vtable(3, 16, slots);
        let read = unsafe { read_vtable(raw.as_ptr() as *const VcVtable) }
            .expect("revision 3 is readable");
        assert_eq!(read.vtable_version, 3);
        // The first slot alone proves nothing: 16 bytes would reach it too.
        // This one is what separates "used REVISION_3_SIZE" from "believed the
        // padding" — delete the `ABI_VERSION_STRUCT_SIZE` branch and it goes
        // `None`, because a 16-byte read stops after `test_connection`.
        assert!(
            read.test_connection.is_some(),
            "the first slot is within even a truncated read",
        );
        assert!(
            read.create_meeting.is_some(),
            "a revision-3 vtable must be read at its own full length, not at \
             whatever its padding happened to hold",
        );
        assert!(
            read.list_meetings.is_some(),
            "including its last slot — the one an appended slot would push past",
        );
    }

    #[test]
    fn a_shorter_vtable_reads_as_absent_beyond_what_it_wrote() {
        // The case the whole arrangement is for: a plugin built before a slot
        // was appended. It says how far it goes, and everything past that is
        // the host's own zeroed memory — `None`, reported as unsupported.
        const HEADER: u32 = 8;
        let one_slot = HEADER + 8;
        let raw = foreign_vtable(crate::ABI_VERSION, one_slot, 8);
        let read = unsafe { read_vtable(raw.as_ptr() as *const VcVtable) }
            .expect("a current-revision plugin is readable");
        assert_eq!(read.struct_size, one_slot);
        assert!(
            read.test_connection.is_some(),
            "the one slot it did write should survive",
        );
        assert!(
            read.create_meeting.is_none(),
            "a slot past the plugin's own length must read as absent, not as \
             whatever followed its struct",
        );
    }

    #[test]
    fn a_longer_vtable_is_read_only_as_far_as_this_host_understands() {
        // The other direction: an older host meeting a newer plugin. It takes
        // the prefix it has a description for and leaves the rest alone —
        // writing all of it would overrun the host's own struct.
        let host_size = std::mem::size_of::<VcVtable>() as u32;
        let raw = foreign_vtable(crate::ABI_VERSION, host_size + 4096, 64);
        let read = unsafe { read_vtable(raw.as_ptr() as *const VcVtable) }
            .expect("a longer plugin is still readable");
        assert!(read.test_connection.is_some());
        assert_eq!(
            read.struct_size,
            host_size + 4096,
            "the plugin's own claim is carried through untouched; the clamp is \
             on how much was COPIED, not on what it said",
        );
    }

    #[test]
    fn a_vtable_claiming_less_than_the_shared_header_is_refused_by_size_not_by_revision() {
        // Two u32s is the one thing every revision has. Something claiming to
        // be smaller is not a vtable, and reading its header anyway would be
        // taking its word for the very thing in doubt.
        //
        // Zero is the case that will actually happen: a C designated
        // initializer that names its methods and never mentions `struct_size`.
        // The refusal has to say SIZE, because saying "revision 4" would name
        // the one field the author got right.
        for claimed in [0, 4, 7] {
            let raw = foreign_vtable(crate::ABI_VERSION, claimed, 8);
            let err = unsafe { read_vtable(raw.as_ptr() as *const VcVtable) }
                .expect_err("a sub-header size is refused");
            assert_eq!(err, VtableReadError::SizeBelowHeader(claimed));
            assert!(
                err.to_string().contains("struct_size"),
                "the message must name the field that is wrong, got: {err}",
            );
        }
    }

    #[test]
    fn a_revision_this_host_has_no_description_of_is_refused_by_revision() {
        // Literals at the bottom, so lowering `ABI_VERSION_MIN` cannot leave
        // this green; `ABI_VERSION + 1` at the top, because there is no literal
        // for a revision that does not exist yet.
        for version in [0, 1, 2, crate::ABI_VERSION + 1] {
            let raw = foreign_vtable(version, std::mem::size_of::<VcVtable>() as u32, 8);
            let err = match unsafe { read_vtable(raw.as_ptr() as *const VcVtable) } {
                Err(err) => err,
                Ok(_) => panic!("revision {version} should be refused"),
            };
            assert_eq!(err, VtableReadError::UnknownRevision(version));
        }
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
        // The leading `8` is the header: `vtable_version` + `struct_size`, two
        // u32s. It was 8 before ABI 4 too — one u32 plus four bytes of padding
        // — which is exactly why adding the size field grew nothing.
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
        // list_meetings. Appending them is what forced the bump to 3: back then
        // the host had no per-vtable length, so strict equality on the manifest
        // was the only thing keeping a plugin built against the shorter layout
        // from being read past its end. `struct_size` replaced that in ABI 4,
        // and the next append will not cost a revision.
        assert_eq!(size_of::<VcVtable>(), 8 + 6 * 8);
        // u32 + one pointer per feature family: calendar, tasks, contacts,
        // sync, videoconference.
        assert_eq!(size_of::<AdapterVtable>(), 8 + 5 * 8);
    }
}
