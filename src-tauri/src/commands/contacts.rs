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
use cal_core::{Contact, ContactList, ContactPhoto, ContactsFeature, NewContact};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Runtime, State};
use tracing::warn;

use super::cache_swr;
use super::{CommandError, CommandResult};
use crate::cache::{CacheStore, RefreshCoordinator, SyncScope};
use crate::contact_sync::{
    ContactSyncScheduler, ContactsSyncStatus, PREF_INCLUDE_READ_ONLY_ON_SYNC, PREF_LAST_SYNCED_AT,
    PREF_SYNC_INTERVAL_MINUTES,
};
use crate::db::DbHandle;
use crate::registry::{AdapterRegistry, LOCAL_ID};
use crate::user_prefs::UserPrefsRepo;

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
    app: AppHandle,
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    coord: State<'_, Arc<RefreshCoordinator>>,
    db: State<'_, DbHandle>,
) -> CommandResult<Vec<ContactListRow>> {
    let cmd_start = std::time::Instant::now();
    tracing::info!(
        target: "aperio::startup",
        total_ms = crate::startup_elapsed_ms(),
        "list_contact_lists command invoked",
    );
    let registry = Arc::clone(&registry);
    let cache = Arc::clone(&cache);
    let coord = Arc::clone(&coord);
    let local = adapter.list_contact_lists().await?;
    for l in &local {
        registry.note_contact_list_route(&l.id, LOCAL_ID);
    }
    let mut external = external_contact_lists_swr(&app, &registry, &cache, &coord).await;
    let mut out = local;
    out.append(&mut external);
    // External address books get their user-chosen color-label binding
    // from the override layer; local ones carry it on the row.
    let shared = db.shared();
    let repo = crate::overrides::OverridesRepo::new(&shared);
    crate::overrides::apply_color_to_contact_lists(&repo, &mut out);
    let rows: Vec<ContactListRow> = out
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
        .collect();
    tracing::info!(
        target: "aperio::startup",
        elapsed_ms = cmd_start.elapsed().as_millis(),
        total_ms = crate::startup_elapsed_ms(),
        contact_lists = rows.len(),
        "list_contact_lists command returning",
    );
    Ok(rows)
}

/// Cache-first aggregation of every external account's contact lists.
/// Same stale-while-revalidate shape as `external_task_lists_swr`.
async fn external_contact_lists_swr(
    app: &AppHandle,
    registry: &Arc<AdapterRegistry>,
    cache: &Arc<CacheStore>,
    coord: &Arc<RefreshCoordinator>,
) -> Vec<ContactList> {
    let mut out = Vec::new();
    for (account, adapter) in registry.snapshot_contact_adapters() {
        let state = cache
            .get_sync_state(&account, SyncScope::ContactLists, "")
            .ok()
            .flatten();
        // Cache-first, never blocking — see `external_calendars_swr` for
        // the rationale. Serve the snapshot (empty on first run) and spawn a
        // deduplicated background refresh when missing or stale; the
        // resulting `cache-updated` re-runs this listing and re-fetches
        // contacts once it lands.
        let cached = cache.read_contact_lists(&account).unwrap_or_default();
        for l in &cached {
            registry.note_contact_list_route(&l.id, &account);
        }
        if cache_swr::is_stale(&state, cache_swr::SWR_TTL_SECS) {
            let adapter_bg = Arc::clone(&adapter);
            let reg = Arc::clone(registry);
            let acc = account.clone();
            cache_swr::spawn_refresh(
                app.clone(),
                Arc::clone(cache),
                Arc::clone(coord),
                SyncScope::ContactLists,
                account.clone(),
                String::new(),
                move || async move { adapter_bg.list_contact_lists().await },
                move |c, lists: &[ContactList]| {
                    for l in lists {
                        reg.note_contact_list_route(&l.id, &acc);
                    }
                    c.replace_contact_lists(&acc, lists)
                },
            );
        }
        out.extend(cached);
    }
    out
}

#[tauri::command]
pub async fn get_contacts(
    app: AppHandle,
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    coord: State<'_, Arc<RefreshCoordinator>>,
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

    // Stale-while-revalidate (see get_tasks for the shape).
    let state = cache
        .get_sync_state(&account, SyncScope::Contacts, &list_id)
        .ok()
        .flatten();
    if cache_swr::has_snapshot(&state) {
        let cached = cache.read_contacts(&account, &list_id).unwrap_or_default();
        if cache_swr::is_stale(&state, cache_swr::SWR_TTL_SECS) {
            let ext_bg = Arc::clone(&ext);
            let cache_bg = Arc::clone(&cache);
            let acc = account.clone();
            let list = list_id.clone();
            cache_swr::spawn_item_refresh(
                app.clone(),
                Arc::clone(&cache),
                Arc::clone(&coord),
                SyncScope::Contacts,
                account.clone(),
                list_id.clone(),
                move || async move {
                    cache_swr::refresh_contacts(&cache_bg, ext_bg.as_ref(), &acc, &list).await
                },
            );
        }
        return Ok(cached);
    }

    // Cold path: no snapshot yet. Don't block the first paint on the
    // network — serve whatever rows exist now and refresh in the
    // background; `cache-updated` fills the list in when the fetch lands.
    let snapshot = cache.read_contacts(&account, &list_id).unwrap_or_default();
    let ext_bg = Arc::clone(&ext);
    let cache_bg = Arc::clone(&cache);
    let acc = account.clone();
    let list = list_id.clone();
    cache_swr::spawn_item_refresh(
        app.clone(),
        Arc::clone(&cache),
        Arc::clone(&coord),
        SyncScope::Contacts,
        account.clone(),
        list_id.clone(),
        move || async move {
            cache_swr::refresh_contacts(&cache_bg, ext_bg.as_ref(), &acc, &list).await
        },
    );
    Ok(snapshot)
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
    cache: State<'_, Arc<CacheStore>>,
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
            message: format!("contact list '{}' is not routable", request.list_id),
        });
    };
    let created = ext
        .create_contact(&request.list_id, request.contact)
        .await?;
    let _ = cache.invalidate(&account, SyncScope::Contacts, &request.list_id);
    Ok(created)
}

#[tauri::command]
pub async fn update_contact(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
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
            message: format!("contact list '{}' is not routable", contact.list_id),
        });
    };
    let list_id = contact.list_id.clone();
    let updated = ext.update_contact(contact).await?;
    let _ = cache.invalidate(&account, SyncScope::Contacts, &list_id);
    Ok(updated)
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
    cache: State<'_, Arc<CacheStore>>,
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
        if let Some(lid) = &list_id {
            let _ = cache.invalidate(&account, SyncScope::Contacts, lid);
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct CreateContactListRequest {
    pub name: String,
    /// Bind the new address book's color to this color-label id (or
    /// `None` for no color).
    #[serde(default)]
    pub color_label: Option<String>,
}

#[tauri::command]
pub async fn create_contact_list(
    adapter: State<'_, LocalAdapter>,
    request: CreateContactListRequest,
) -> CommandResult<ContactListRow> {
    let color_label = request.color_label.map(cal_core::ColorLabelId);
    let list = adapter.create_contact_list(&request.name, None, color_label)?;
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
    cache: State<'_, Arc<CacheStore>>,
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
        let _ = cache.invalidate(&account, SyncScope::ContactLists, "");
    }
    Ok(())
}

/// Resolve the adapter that owns this contact's list (or the
/// local adapter when `list_id` is missing or doesn't match a
/// known external book). Returns the account id alongside so the
/// caller can decide whether to invoke the local or external
/// path. Mirrors the lookup pattern `delete_contact` uses.
fn resolve_contact_account(registry: &AdapterRegistry, list_id: Option<&str>) -> String {
    list_id
        .and_then(|lid| registry.account_for_contact_list(lid))
        .unwrap_or_else(|| LOCAL_ID.to_string())
}

/// Pull the avatar bytes for a contact. Returns `None` when the
/// contact has no photo (a frontend that calls this opportunistically
/// because `has_photo` was true would have surfaced the no-photo
/// placeholder; getting `Ok(None)` keeps it from showing a broken
/// image instead).
#[tauri::command]
pub async fn get_contact_photo(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    id: String,
    list_id: Option<String>,
) -> CommandResult<Option<ContactPhoto>> {
    let account = resolve_contact_account(&registry, list_id.as_deref());
    if account == LOCAL_ID {
        return Ok(adapter.get_contact_photo(&id).await?);
    }
    let Some(ext) = registry.contact_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("account '{account}' is not routable"),
        });
    };
    Ok(ext.get_contact_photo(&id).await?)
}

/// Replace (or set) the contact's avatar. `photo.data` arrives
/// from the frontend already base64-decoded via the serde shape
/// on `ContactPhoto`.
#[tauri::command]
pub async fn set_contact_photo(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    id: String,
    list_id: Option<String>,
    photo: ContactPhoto,
) -> CommandResult<()> {
    let account = resolve_contact_account(&registry, list_id.as_deref());
    if account == LOCAL_ID {
        adapter.set_contact_photo(&id, photo).await?;
        return Ok(());
    }
    let Some(ext) = registry.contact_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("account '{account}' is not routable"),
        });
    };
    ext.set_contact_photo(&id, photo).await?;
    Ok(())
}

/// Run a contact sync pass on demand — used by the "Refresh"
/// button in the contacts view and the equivalent action in
/// Settings → Kontakte. Returns `true` when the pass actually
/// ran, `false` when another pass was already in flight and this
/// invocation was deduped.
///
/// `include_read_only`: explicit per-call override of the
/// read-only-directory toggle. `Some(true)` always pulls the
/// expensive sentinel lists (EWS GAL, Google Other Contacts /
/// Workspace Directory, Graph Suggested People); `Some(false)`
/// always skips them; `None` reads the user's persisted
/// `contacts.includeReadOnlyOnSync` pref so the manual button
/// matches whatever the periodic scheduler would do. Most
/// callers should pass `None` to keep the behaviour consistent.
#[tauri::command]
pub async fn sync_contacts_now<R: Runtime>(
    scheduler: State<'_, Arc<ContactSyncScheduler>>,
    app: AppHandle<R>,
    include_read_only: Option<bool>,
) -> CommandResult<bool> {
    let effective = include_read_only.unwrap_or_else(|| scheduler.read_include_read_only_on_sync());
    Ok(scheduler.run_sync(&app, effective).await)
}

/// Snapshot the contact sync status — used by the contacts view
/// footer to render "Last synced at …" + the configured interval.
#[tauri::command]
pub async fn get_contacts_sync_status(
    scheduler: State<'_, Arc<ContactSyncScheduler>>,
) -> CommandResult<ContactsSyncStatus> {
    Ok(scheduler.status())
}

/// Drop every external adapter's in-memory contact cache and
/// reset `contacts.lastSyncedAt` to "never". Backs the
/// "Cache leeren" button in Settings → Kontakte (DESIGN.md §10.6).
///
/// Local contact rows are user data, NOT a cache — this command
/// leaves the SQLite `contacts` / `contact_lists` tables alone.
/// What it wipes is the per-adapter HashMap snapshots; the next
/// sync pass (auto or manual) repopulates them from the wire.
///
/// Returns the number of accounts the invalidate succeeded
/// against; failed adapters log warnings but don't sink the
/// command — partial-success is the right outcome when one
/// account's server is unreachable but others are fine.
#[tauri::command]
pub async fn clear_contacts_cache(
    registry: State<'_, Arc<AdapterRegistry>>,
    db: State<'_, DbHandle>,
) -> CommandResult<usize> {
    let mut succeeded = 0usize;
    for (account_id, adapter) in registry.snapshot_contact_adapters() {
        match adapter.invalidate_contacts_cache().await {
            Ok(()) => {
                succeeded += 1;
            }
            Err(err) => {
                warn!(
                    account_id = %account_id,
                    ?err,
                    "invalidate_contacts_cache failed for adapter",
                );
            }
        }
    }
    // Reset the persisted "last synced" timestamp so the panel
    // footer flips back to "no sync run yet" until the next pass
    // completes. The in-memory state on `ContactSyncScheduler`
    // isn't reset here on purpose — the next `contacts-synced`
    // event will overwrite it. Keeping the in-memory value avoids
    // a brief window where the footer flickers before the prefs
    // round-trip lands.
    let shared = db.shared();
    let repo = UserPrefsRepo::new(&shared);
    if let Err(err) = repo.delete(PREF_LAST_SYNCED_AT) {
        warn!(?err, "failed to delete contacts.lastSyncedAt");
    }
    Ok(succeeded)
}

/// Configure the periodic-sync interval. `minutes` is clamped to
/// at least 1 and at most 24 * 60 = 1440 so a typo doesn't pin
/// the scheduler into a hot loop or wedge it for a calendar day.
/// Writes the value to `user_prefs.contacts.syncIntervalMinutes`
/// — the scheduler re-reads on every tick, so the new interval
/// applies on the next periodic pass.
#[tauri::command]
pub async fn set_contacts_sync_interval(
    db: State<'_, DbHandle>,
    minutes: u32,
) -> CommandResult<u32> {
    // Clamp aggressively: 1-minute floor avoids the hot-loop
    // edge case (scheduler also clamps in-memory, but pinning it
    // at the persistence boundary too means a typo never makes
    // it into the DB), 24-hour ceiling keeps the value visibly
    // sensible — the UI's `interval` dropdown offers presets up
    // to 240 anyway, so the ceiling is just a defensive net.
    let clamped = minutes.clamp(1, 24 * 60);
    let shared = db.shared();
    let repo = UserPrefsRepo::new(&shared);
    repo.set(PREF_SYNC_INTERVAL_MINUTES, &clamped.to_string())
        .map_err(|err| CommandError {
            code: "internal",
            message: err.to_string(),
        })?;
    Ok(clamped)
}

/// Persist the "also pull read-only directories" toggle from
/// Settings → Kontakte. Writes the literal string `"true"` /
/// `"false"` to `user_prefs.contacts.includeReadOnlyOnSync`; the
/// scheduler re-reads on every tick so the new value applies on
/// the next pass without a restart.
///
/// Typed thin wrapper around `set_user_pref` — used in preference
/// to the generic command so the wire shape (a real boolean
/// rather than a string) catches typos at the TypeScript boundary.
#[tauri::command]
pub async fn set_contacts_include_read_only_on_sync(
    db: State<'_, DbHandle>,
    enabled: bool,
) -> CommandResult<()> {
    let shared = db.shared();
    let repo = UserPrefsRepo::new(&shared);
    repo.set(
        PREF_INCLUDE_READ_ONLY_ON_SYNC,
        if enabled { "true" } else { "false" },
    )
    .map_err(|err| CommandError {
        code: "internal",
        message: err.to_string(),
    })?;
    Ok(())
}

/// Clear the avatar without touching any other field. Idempotent
/// — calling this on a contact that already has no photo
/// succeeds silently on the local adapter and (for external
/// adapters) just finds no matching attachments to delete.
#[tauri::command]
pub async fn delete_contact_photo(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    id: String,
    list_id: Option<String>,
) -> CommandResult<()> {
    let account = resolve_contact_account(&registry, list_id.as_deref());
    if account == LOCAL_ID {
        adapter.delete_contact_photo(&id).await?;
        return Ok(());
    }
    let Some(ext) = registry.contact_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("account '{account}' is not routable"),
        });
    };
    ext.delete_contact_photo(&id).await?;
    Ok(())
}
