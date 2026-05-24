//! `ContactsVtable` — mirrors `cal_core::ContactsFeature`.
//!
//! Same FFI shape + JSON-arguments convention as the calendar
//! vtable. See `crates/cal-core/src/adapter.rs::ContactsFeature`
//! for the source-of-truth trait the JSON payloads mirror.
//!
//! Photos cross the bridge as base64 strings inside their
//! `ContactPhoto` struct — same on-the-wire shape cal-core
//! already uses for sync events, so the JSON encoder and decoder
//! Just Work without bespoke handling.

use super::VtableMethodFn;

/// Vtable for plugins that declare `Capability::Contacts` in
/// their manifest. Multi-capability plugins ship one of these
/// alongside a [`super::calendar::CalendarVtable`] / [`super::tasks::TasksVtable`].
///
/// Layout MUST stay binary-compatible across plugin-core 0.x
/// patch versions.
#[repr(C)]
#[derive(Debug)]
pub struct ContactsVtable {
    pub vtable_version: u32,

    // ── Base Adapter methods ────────────────────────────────────
    pub authenticate: Option<VtableMethodFn>,
    pub capabilities: Option<VtableMethodFn>,

    // ── Contacts trait methods ─────────────────────────────────
    pub list_contact_lists: Option<VtableMethodFn>,
    pub get_contacts: Option<VtableMethodFn>,
    pub search_contacts: Option<VtableMethodFn>,
    pub create_contact: Option<VtableMethodFn>,
    pub update_contact: Option<VtableMethodFn>,
    pub delete_contact: Option<VtableMethodFn>,
    /// `rename_contact_list(list_id, new_name)`. Default-
    /// `Unsupported` on read-only providers (e.g. Google's
    /// "Other contacts") — leave `None` to inherit that at the
    /// shim level.
    pub rename_contact_list: Option<VtableMethodFn>,
    /// `get_contact_photo(contact_id) -> Option<ContactPhoto>`.
    /// Lazy fetch — listings only carry the `has_photo` flag.
    pub get_contact_photo: Option<VtableMethodFn>,
    pub set_contact_photo: Option<VtableMethodFn>,
    pub delete_contact_photo: Option<VtableMethodFn>,
    /// `invalidate_contacts_cache()`. Default no-op on the
    /// trait (the local adapter inherits the no-op); external
    /// plugins override to clear in-memory listing caches.
    pub invalidate_contacts_cache: Option<VtableMethodFn>,
}

impl ContactsVtable {
    pub const fn empty() -> Self {
        Self {
            vtable_version: crate::ABI_VERSION,
            authenticate: None,
            capabilities: None,
            list_contact_lists: None,
            get_contacts: None,
            search_contacts: None,
            create_contact: None,
            update_contact: None,
            delete_contact: None,
            rename_contact_list: None,
            get_contact_photo: None,
            set_contact_photo: None,
            delete_contact_photo: None,
            invalidate_contacts_cache: None,
        }
    }

    pub fn has_minimum_surface(&self) -> bool {
        self.list_contact_lists.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_vtable_has_no_methods() {
        let v = ContactsVtable::empty();
        assert!(v.list_contact_lists.is_none());
        assert!(v.get_contacts.is_none());
        assert!(v.search_contacts.is_none());
        assert!(v.get_contact_photo.is_none());
        assert!(v.invalidate_contacts_cache.is_none());
        assert!(!v.has_minimum_surface());
        assert_eq!(v.vtable_version, crate::ABI_VERSION);
    }
}
