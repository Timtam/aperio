//! `FfiContactsAdapter` — `cal_core::ContactsFeature` impl that
//! dispatches across the FFI boundary into a loaded plugin's
//! [`crate::vtables::ContactsVtable`].
//!
//! Same shape as [`super::calendar::FfiCalendarAdapter`]; see
//! that file's module doc for the canonical pattern.
//!
//! Photo bytes cross the bridge inside the `ContactPhoto` struct
//! that cal-core already uses — base64-encoded, the same shape
//! the existing local adapter persists to SQLite.

use std::sync::Arc;

use async_trait::async_trait;
use cal_core::adapter::{Adapter, AuthToken, Capability, ContactsFeature, Credentials};
use cal_core::error::{Error, Result};
use cal_core::types::{Contact, ContactList, ContactPhoto, NewContact};
use serde::Serialize;
use tracing::warn;

use crate::ffi::*;
use crate::manager::LoadedPlugin;
use crate::vtables::ContactsVtable;

use super::call::{call_method, decode_payload, encode_args, CallOutcome};

pub struct FfiContactsAdapter {
    _plugin: Arc<LoadedPlugin>,
    vtable: VtableSnapshot,
    capabilities: Vec<Capability>,
}

#[derive(Clone, Copy)]
struct VtableSnapshot {
    authenticate: Option<crate::vtables::VtableMethodFn>,
    list_contact_lists: Option<crate::vtables::VtableMethodFn>,
    get_contacts: Option<crate::vtables::VtableMethodFn>,
    search_contacts: Option<crate::vtables::VtableMethodFn>,
    create_contact: Option<crate::vtables::VtableMethodFn>,
    update_contact: Option<crate::vtables::VtableMethodFn>,
    delete_contact: Option<crate::vtables::VtableMethodFn>,
    rename_contact_list: Option<crate::vtables::VtableMethodFn>,
    get_contact_photo: Option<crate::vtables::VtableMethodFn>,
    set_contact_photo: Option<crate::vtables::VtableMethodFn>,
    delete_contact_photo: Option<crate::vtables::VtableMethodFn>,
    invalidate_contacts_cache: Option<crate::vtables::VtableMethodFn>,
}

impl FfiContactsAdapter {
    pub fn new(plugin: Arc<LoadedPlugin>) -> Option<Self> {
        let raw = plugin.vtable_ptr();
        if raw.is_null() {
            warn!(
                plugin_id = %plugin.manifest.id,
                "contacts plugin has NULL vtable; refusing to wrap",
            );
            return None;
        }
        // SAFETY: the manifest tells us this is a contacts-capable
        // adapter, so the vtable points at ContactsVtable.
        let vtable_ref: &ContactsVtable =
            unsafe { &*(raw as *const ContactsVtable) };
        if !vtable_ref.has_minimum_surface() {
            warn!(
                plugin_id = %plugin.manifest.id,
                "contacts plugin's vtable lacks list_contact_lists; refusing to wrap",
            );
            return None;
        }
        let snapshot = VtableSnapshot {
            authenticate: vtable_ref.authenticate,
            list_contact_lists: vtable_ref.list_contact_lists,
            get_contacts: vtable_ref.get_contacts,
            search_contacts: vtable_ref.search_contacts,
            create_contact: vtable_ref.create_contact,
            update_contact: vtable_ref.update_contact,
            delete_contact: vtable_ref.delete_contact,
            rename_contact_list: vtable_ref.rename_contact_list,
            get_contact_photo: vtable_ref.get_contact_photo,
            set_contact_photo: vtable_ref.set_contact_photo,
            delete_contact_photo: vtable_ref.delete_contact_photo,
            invalidate_contacts_cache: vtable_ref.invalidate_contacts_cache,
        };
        let capabilities = super::manifest_capabilities(&plugin.manifest.capabilities);
        Some(Self {
            _plugin: plugin,
            vtable: snapshot,
            capabilities,
        })
    }
}

fn status_to_cal_error(outcome: CallOutcome) -> Error {
    let msg = outcome.message();
    match outcome.status {
        PLUGIN_CALL_ERR_UNSUPPORTED => Error::Unsupported(msg),
        PLUGIN_CALL_ERR_INVALID => Error::InvalidInput(msg),
        PLUGIN_CALL_ERR_AUTH => Error::Authentication(msg),
        PLUGIN_CALL_ERR_NETWORK => Error::Network(msg),
        PLUGIN_CALL_ERR_NOT_FOUND => Error::NotFound(msg),
        PLUGIN_CALL_ERR_PROTOCOL => Error::Protocol(msg),
        PLUGIN_CALL_ERR_CONFLICT => Error::Conflict(msg),
        PLUGIN_CALL_ERR_FORBIDDEN => Error::Forbidden(msg),
        PLUGIN_CALL_ERR_IO => Error::Internal(format!("plugin IO: {msg}")),
        PLUGIN_CALL_ERR_INTERNAL => Error::Internal(msg),
        other => Error::Internal(format!("plugin status {other}: {msg}")),
    }
}

async fn call_then_decode<T, A>(
    method: Option<crate::vtables::VtableMethodFn>,
    args: &A,
) -> Result<T>
where
    T: serde::de::DeserializeOwned,
    A: Serialize,
{
    let bytes = encode_args(args).map_err(|e| Error::Internal(format!(
        "encode args: {e}"
    )))?;
    let outcome = call_method(method, bytes).await;
    if outcome.is_ok() {
        decode_payload(&outcome.bytes).map_err(|e| Error::Protocol(format!(
            "decode plugin response: {e}"
        )))
    } else {
        Err(status_to_cal_error(outcome))
    }
}

async fn call_for_unit<A: Serialize>(
    method: Option<crate::vtables::VtableMethodFn>,
    args: &A,
) -> Result<()> {
    let bytes = encode_args(args).map_err(|e| Error::Internal(format!(
        "encode args: {e}"
    )))?;
    let outcome = call_method(method, bytes).await;
    if outcome.is_ok() {
        Ok(())
    } else {
        Err(status_to_cal_error(outcome))
    }
}

#[derive(Serialize)]
struct CreateContactArgs<'a> {
    list_id: &'a str,
    contact: NewContact,
}

#[derive(Serialize)]
struct RenameContactListArgs<'a> {
    list_id: &'a str,
    new_name: &'a str,
}

#[derive(Serialize)]
struct SetContactPhotoArgs<'a> {
    contact_id: &'a str,
    photo: ContactPhoto,
}

#[async_trait]
impl Adapter for FfiContactsAdapter {
    async fn authenticate(&self, credentials: Credentials) -> Result<AuthToken> {
        call_then_decode(self.vtable.authenticate, &credentials).await
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl ContactsFeature for FfiContactsAdapter {
    async fn list_contact_lists(&self) -> Result<Vec<ContactList>> {
        call_then_decode(self.vtable.list_contact_lists, &()).await
    }

    async fn get_contacts(&self, list_id: &str) -> Result<Vec<Contact>> {
        call_then_decode(self.vtable.get_contacts, &list_id).await
    }

    async fn search_contacts(&self, query: &str) -> Result<Vec<Contact>> {
        call_then_decode(self.vtable.search_contacts, &query).await
    }

    async fn create_contact(
        &self,
        list_id: &str,
        contact: NewContact,
    ) -> Result<Contact> {
        let args = CreateContactArgs { list_id, contact };
        call_then_decode(self.vtable.create_contact, &args).await
    }

    async fn update_contact(&self, contact: Contact) -> Result<Contact> {
        call_then_decode(self.vtable.update_contact, &contact).await
    }

    async fn delete_contact(&self, contact_id: &str) -> Result<()> {
        call_for_unit(self.vtable.delete_contact, &contact_id).await
    }

    async fn rename_contact_list(
        &self,
        list_id: &str,
        new_name: &str,
    ) -> Result<()> {
        let args = RenameContactListArgs { list_id, new_name };
        call_for_unit(self.vtable.rename_contact_list, &args).await
    }

    async fn get_contact_photo(
        &self,
        contact_id: &str,
    ) -> Result<Option<ContactPhoto>> {
        call_then_decode(self.vtable.get_contact_photo, &contact_id).await
    }

    async fn set_contact_photo(
        &self,
        contact_id: &str,
        photo: ContactPhoto,
    ) -> Result<()> {
        let args = SetContactPhotoArgs { contact_id, photo };
        call_for_unit(self.vtable.set_contact_photo, &args).await
    }

    async fn delete_contact_photo(&self, contact_id: &str) -> Result<()> {
        call_for_unit(self.vtable.delete_contact_photo, &contact_id).await
    }

    async fn invalidate_contacts_cache(&self) -> Result<()> {
        call_for_unit(self.vtable.invalidate_contacts_cache, &()).await
    }
}
