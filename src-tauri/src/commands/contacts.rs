//! Contact list and contact commands (DESIGN.md §10).
//!
//! Mirrors the shape of `tasks.rs` / `calendars.rs`:
//!
//!   - The local adapter (`LocalAdapter`) is always available
//!     directly via Tauri State. The default
//!     `local-default-contacts` list is seeded by migration 0007,
//!     so the implicit local-only flow has a destination from
//!     day one.
//!   - External adapters with `ContactsFeature` (still none in
//!     Phase 10a — CardDAV, Google People, MS Graph Contacts
//!     come in 10b+) sit behind the `AdapterRegistry`. Routes
//!     are filled lazily during `list_contact_lists` and
//!     re-resolved per command from the `list_id`.
//!   - `search_contacts` fans out across local + every external
//!     adapter and concatenates the hits — matches how the
//!     attendees picker (§10.4) will consume the surface in
//!     10a-3.

use cal_adapter_local::LocalAdapter;
use cal_core::{Contact, ContactList, ContactsFeature, NewContact};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

use super::{CommandError, CommandResult};
use crate::registry::{AdapterRegistry, LOCAL_ID};

/// Wire-format `ContactList` enriched with the owning account id —
/// same shape rationale as `CalendarRow` and `TaskListRow`. Lets
/// the sidebar group containers by source without a second
/// round-trip to the registry.
#[derive(Debug, Serialize)]
pub struct ContactListRow {
    #[serde(flatten)]
    pub inner: ContactList,
    pub account_id: String,
}

#[tauri::command]
pub async fn list_contact_lists(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
) -> CommandResult<Vec<ContactListRow>> {
    let local = adapter.list_contact_lists().await?;
    for l in &local {
        registry.note_contact_list_route(&l.id, LOCAL_ID);
    }
    let mut external = registry.list_external_contact_lists().await;
    let mut out = local;
    out.append(&mut external);
    Ok(out
        .into_iter()
        .map(|list| {
            let account_id = registry
                .account_for_contact_list(&list.id)
                .unwrap_or_else(|| LOCAL_ID.to_string());
            ContactListRow {
                inner: list,
                account_id,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn get_contacts(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    list_id: String,
) -> CommandResult<Vec<Contact>> {
    let account = registry
        .account_for_contact_list(&list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(adapter.get_contacts(&list_id).await?);
    }
    let Some(ext) = registry.contact_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("contact list '{list_id}' is not routable"),
        });
    };
    Ok(ext.get_contacts(&list_id).await?)
}

/// Cross-account contacts search. Local hits land first, external
/// hits follow. The local adapter caps its own result at 50 rows;
/// each external adapter does whatever it does (the trait
/// contract leaves the cap up to the implementer). The picker UI
/// expects a "reasonable handful" — if a sync ever ships thousands
/// of contacts per account the command can grow paging later.
#[tauri::command]
pub async fn search_contacts(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    query: String,
) -> CommandResult<Vec<Contact>> {
    let local = adapter.search_contacts(&query).await?;
    let mut external = registry.search_external_contacts(&query).await;
    let mut out = local;
    out.append(&mut external);
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct CreateContactRequest {
    pub list_id: String,
    #[serde(flatten)]
    pub contact: NewContact,
}

#[tauri::command]
pub async fn create_contact(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    request: CreateContactRequest,
) -> CommandResult<Contact> {
    let account = registry
        .account_for_contact_list(&request.list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(adapter
            .create_contact(&request.list_id, request.contact)
            .await?);
    }
    let Some(ext) = registry.contact_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!(
                "contact list '{}' is not routable",
                request.list_id
            ),
        });
    };
    Ok(ext.create_contact(&request.list_id, request.contact).await?)
}

#[tauri::command]
pub async fn update_contact(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    contact: Contact,
) -> CommandResult<Contact> {
    let account = registry
        .account_for_contact_list(&contact.list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(adapter.update_contact(contact).await?);
    }
    let Some(ext) = registry.contact_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!(
                "contact list '{}' is not routable",
                contact.list_id
            ),
        });
    };
    Ok(ext.update_contact(contact).await?)
}

/// Delete a contact by id.
///
/// `list_id` is an optional routing hint: the `ContactsFeature`
/// trait surface for `delete_contact` only carries the contact id,
/// but the registry needs the owning account to route the write.
/// The frontend always knows the list (it just rendered the row),
/// so passing it through saves us a walk-every-list fallback like
/// the one `delete_event` uses.
#[tauri::command]
pub async fn delete_contact(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    id: String,
    list_id: Option<String>,
) -> CommandResult<()> {
    let account = list_id
        .as_deref()
        .and_then(|lid| registry.account_for_contact_list(lid))
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        adapter.delete_contact(&id).await?;
    } else {
        let Some(ext) = registry.contact_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("account '{account}' is not routable"),
            });
        };
        ext.delete_contact(&id).await?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct CreateContactListRequest {
    pub name: String,
    pub color_hex: Option<String>,
}

#[tauri::command]
pub async fn create_contact_list(
    adapter: State<'_, LocalAdapter>,
    request: CreateContactListRequest,
) -> CommandResult<ContactListRow> {
    let color = request
        .color_hex
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|hex| cal_core::ContainerColor::custom(hex.to_string()));
    let list = adapter.create_contact_list(&request.name, color)?;
    Ok(ContactListRow {
        inner: list,
        account_id: LOCAL_ID.to_string(),
    })
}

#[tauri::command]
pub async fn delete_contact_list(
    adapter: State<'_, LocalAdapter>,
    id: String,
) -> CommandResult<()> {
    adapter.delete_contact_list(&id)?;
    Ok(())
}

#[tauri::command]
pub async fn rename_contact_list(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    id: String,
    new_name: String,
) -> CommandResult<()> {
    let account = registry
        .account_for_contact_list(&id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        adapter.rename_contact_list(&id, &new_name).await?;
    } else {
        let Some(ext) = registry.contact_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("contact list '{id}' is not routable"),
            });
        };
        ext.rename_contact_list(&id, &new_name).await?;
    }
    Ok(())
}
