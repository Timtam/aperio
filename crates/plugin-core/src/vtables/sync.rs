//! `SyncVtable` — mirrors `sync_core::SyncAdapter`.
//!
//! Same FFI shape + JSON-arguments convention as the calendar
//! vtable. See `crates/sync-core/src/adapter.rs` for the
//! source-of-truth trait the JSON payloads mirror.
//!
//! Log files + snapshots cross the bridge as their already-
//! existing serde-encoded JSON shapes — sync-core's
//! `LogFile`, `Snapshot`, `MetaJson` types all already derive
//! `Serialize` / `Deserialize` because they get written to disk
//! by the file-based local sync adapter. Reusing the same
//! encoding here keeps the wire format one-to-one with the
//! on-disk format.

use super::VtableMethodFn;

/// Vtable for `plugin_type = "sync-adapter"` plugins
/// (DESIGN.md §19.6 + §20.3).
///
/// Layout MUST stay binary-compatible across plugin-core 0.x
/// patch versions.
#[repr(C)]
#[derive(Debug)]
pub struct SyncVtable {
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

    // ── SyncAdapter methods ────────────────────────────────────
    /// `test_connection()` — adapter-specific probe. The
    /// orchestrator calls this from `configure_sync_adapter`
    /// so misconfigurations surface at the Settings dialog
    /// rather than the first sync round.
    pub test_connection: Option<VtableMethodFn>,
    /// `fetch_meta()` — read `meta.json`. The optional Some/None
    /// distinguishes "no dataset yet" from a fetch failure;
    /// the JSON response is `null` for the None case.
    pub fetch_meta: Option<VtableMethodFn>,
    /// `push_meta(MetaJson)` — write `meta.json`. Atomic from
    /// the adapter's perspective.
    pub push_meta: Option<VtableMethodFn>,
    /// `fetch_new_logs(DeviceCursor)` — paginated read of every
    /// log file the cursor doesn't already cover.
    pub fetch_new_logs: Option<VtableMethodFn>,
    /// `push_log(LogFile)`.
    pub push_log: Option<VtableMethodFn>,
    /// `fetch_snapshot()`. Like fetch_meta the response is
    /// `null` for the no-snapshot case.
    pub fetch_snapshot: Option<VtableMethodFn>,
    /// `push_snapshot(Snapshot)`.
    pub push_snapshot: Option<VtableMethodFn>,
    /// `delete_log(LogFileName)`. Used by the compactor.
    pub delete_log: Option<VtableMethodFn>,
    /// `push_sound_asset(hash, bytes)` — DESIGN.md §19.10
    /// custom-sound sync.
    pub push_sound_asset: Option<VtableMethodFn>,
    /// `fetch_sound_asset(hash) -> Option<bytes>`.
    pub fetch_sound_asset: Option<VtableMethodFn>,
}

// SAFETY: `#[repr(C)]`, opens with `vtable_version` then `struct_size`, and
// holds nothing after them but `Option<VtableMethodFn>` slots — so an all-zero
// tail reads as `None`, which is what "the plugin does not implement this"
// already means everywhere else. The revision-3 size is the one below.
unsafe impl crate::vtables::ForeignVtable for SyncVtable {
    // Revision 3 shipped 10 slots behind the leading `u32`, and the four
    // bytes that are now `struct_size` were padding it already had: 88
    // bytes. A literal, not `size_of::<Self>()` — see `read_vtable`. It stays
    // 88 when a slot is appended, because appending does not change a
    // struct that already shipped.
    const REVISION_3_SIZE: usize = 88;
}

// The invariant `read_vtable` relies on to stay in bounds. Appending a slot
// keeps it true; shrinking the struct, or mistyping the size above, does not.
const _: () = assert!(
    <SyncVtable as crate::vtables::ForeignVtable>::REVISION_3_SIZE
        <= std::mem::size_of::<SyncVtable>()
);

impl SyncVtable {
    pub const fn empty() -> Self {
        Self {
            vtable_version: crate::ABI_VERSION,
            struct_size: std::mem::size_of::<Self>() as u32,
            test_connection: None,
            fetch_meta: None,
            push_meta: None,
            fetch_new_logs: None,
            push_log: None,
            fetch_snapshot: None,
            push_snapshot: None,
            delete_log: None,
            push_sound_asset: None,
            fetch_sound_asset: None,
        }
    }

    /// A sync adapter that can't list nor push logs is useless —
    /// fast-fail at load time rather than at the first sync
    /// round.
    pub fn has_minimum_surface(&self) -> bool {
        self.fetch_new_logs.is_some()
            && self.push_log.is_some()
            && self.fetch_meta.is_some()
            && self.push_meta.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_vtable_has_no_methods() {
        let v = SyncVtable::empty();
        assert!(v.test_connection.is_none());
        assert!(v.fetch_meta.is_none());
        assert!(v.push_log.is_none());
        assert!(!v.has_minimum_surface());
        assert_eq!(v.vtable_version, crate::ABI_VERSION);
    }
}
