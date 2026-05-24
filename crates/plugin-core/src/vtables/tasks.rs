//! `TasksVtable` — mirrors `cal_core::TasksFeature`.
//!
//! Same FFI shape as [`super::calendar::CalendarVtable`]: every
//! slot is an `Option<CalendarMethodFn>`-style fn pointer that
//! takes JSON-encoded args + returns a [`PluginCallResult`]. The
//! method-fn type itself is reused under a more generic alias
//! ([`super::VtableMethodFn`]) so future vtables don't have to
//! re-declare the same signature.
//!
//! See `crates/cal-core/src/adapter.rs::TasksFeature` for the
//! source-of-truth trait the JSON payloads mirror.

use super::VtableMethodFn;

/// Vtable for plugins that declare `Capability::Tasks` in their
/// manifest. Multi-capability plugins ship one of these alongside
/// a [`super::calendar::CalendarVtable`] / [`super::contacts::ContactsVtable`].
///
/// Layout MUST stay binary-compatible across plugin-core 0.x
/// patch versions.
#[repr(C)]
#[derive(Debug)]
pub struct TasksVtable {
    pub vtable_version: u32,

    // ── Base Adapter methods ────────────────────────────────────
    /// `authenticate(Credentials) -> AuthToken`. Same semantics
    /// as the CalendarVtable slot — MAY be `None`.
    pub authenticate: Option<VtableMethodFn>,
    /// `capabilities() -> Vec<Capability>`. MAY be `None`;
    /// manifest is the fallback source.
    pub capabilities: Option<VtableMethodFn>,

    // ── Tasks trait methods ────────────────────────────────────
    pub list_task_lists: Option<VtableMethodFn>,
    pub get_tasks: Option<VtableMethodFn>,
    pub create_task: Option<VtableMethodFn>,
    pub update_task: Option<VtableMethodFn>,
    pub delete_task: Option<VtableMethodFn>,
    /// `rename_task_list(list_id, new_name)`. Default-`Unsupported`
    /// on read-only adapters; leave `None` to inherit that
    /// behaviour at the shim level.
    pub rename_task_list: Option<VtableMethodFn>,
}

impl TasksVtable {
    pub const fn empty() -> Self {
        Self {
            vtable_version: crate::ABI_VERSION,
            authenticate: None,
            capabilities: None,
            list_task_lists: None,
            get_tasks: None,
            create_task: None,
            update_task: None,
            delete_task: None,
            rename_task_list: None,
        }
    }

    /// Same load-time sanity check as CalendarVtable: a plugin
    /// that doesn't fill `list_task_lists` can't service the
    /// `Capability::Tasks` it declared.
    pub fn has_minimum_surface(&self) -> bool {
        self.list_task_lists.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_vtable_has_no_methods() {
        let v = TasksVtable::empty();
        assert!(v.authenticate.is_none());
        assert!(v.list_task_lists.is_none());
        assert!(v.get_tasks.is_none());
        assert!(!v.has_minimum_surface());
        assert_eq!(v.vtable_version, crate::ABI_VERSION);
    }
}
