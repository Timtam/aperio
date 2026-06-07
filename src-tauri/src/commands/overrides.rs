//! Container name override commands.
//!
//! Three verbs:
//!
//!   - `rename_container(id, kind, name)` — the **canonical** entry
//!     point. Tries to push the new name to the source first (via
//!     the adapter's `rename_calendar` / `rename_task_list` trait
//!     method). On success the source becomes authoritative and any
//!     existing local override is cleared. On `Unsupported` (iCal
//!     feeds, future read-only sources) the rename falls back to a
//!     local override. Other adapter errors bubble up.
//!
//!   - `set_container_name_override(id, kind, name)` — power-user
//!     escape hatch. Sets a local override regardless of whether the
//!     adapter could have done the rename. Useful when the user
//!     wants a display name that diverges from the server name.
//!
//!   - `clear_container_name_override(id, kind)` — drop the
//!     override. The next read uses the source name.

use cal_adapter_local::LocalAdapter;
use cal_core::{CalendarFeature, ColorLabelId, TasksFeature};
use std::sync::Arc;
use sync_core::{EventPayload, SyncEvent};
use tauri::State;

use super::{CommandError, CommandResult};
use crate::cache::CacheStore;
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;
use crate::overrides::{ContainerKind, OverridesError, OverridesRepo};
use crate::registry::{AdapterRegistry, LOCAL_ID};

#[tauri::command]
pub async fn set_container_name_override(
    db: State<'_, DbHandle>,
    container_id: String,
    kind: ContainerKind,
    name: String,
) -> CommandResult<()> {
    let shared = db.shared();
    let repo = OverridesRepo::new(&shared);
    repo.set(&container_id, kind, &name)?;
    Ok(())
}

#[tauri::command]
pub async fn clear_container_name_override(
    db: State<'_, DbHandle>,
    container_id: String,
    kind: ContainerKind,
) -> CommandResult<()> {
    let shared = db.shared();
    let repo = OverridesRepo::new(&shared);
    repo.clear(&container_id, kind)?;
    Ok(())
}

/// Bind (or, with `color_label_id = None`, unbind) a container's color to
/// a global color-label (DESIGN §6.5 / §8.2).
///
/// Routing mirrors the rename story: a LOCAL calendar / task list carries
/// the binding on its own (synced) row — we update it and emit a sync
/// event so other devices follow. Everything else (external containers,
/// plus local contact lists, which don't event-log-sync) stores the
/// binding as a host-local override that the read path stamps on top.
#[tauri::command]
pub async fn set_container_color_label(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    event_log: State<'_, Arc<EventLogWriter>>,
    db: State<'_, DbHandle>,
    container_id: String,
    kind: ContainerKind,
    color_label_id: Option<String>,
) -> CommandResult<()> {
    let account = match kind {
        ContainerKind::Calendar => registry.account_for_calendar(&container_id),
        ContainerKind::TaskList => registry.account_for_task_list(&container_id),
        ContainerKind::ContactList => registry.account_for_contact_list(&container_id),
    }
    .unwrap_or_else(|| LOCAL_ID.to_string());
    let is_local = account == LOCAL_ID;
    let label = color_label_id.clone().map(ColorLabelId);

    if is_local && matches!(kind, ContainerKind::Calendar) {
        if let Some(mut cal) = adapter.get_calendar_by_id(&container_id)? {
            cal.color_label = label;
            let updated = adapter.update_calendar(cal)?;
            if let Ok(fields) = serde_json::to_value(&updated) {
                event_log.append(SyncEvent::CalendarUpdated(EventPayload {
                    id: updated.id.clone(),
                    fields,
                }));
            }
        }
        return Ok(());
    }
    if is_local && matches!(kind, ContainerKind::TaskList) {
        if let Some(mut list) = adapter.get_task_list_by_id(&container_id)? {
            list.color_label = label;
            let updated = adapter.update_task_list(list)?;
            if let Ok(fields) = serde_json::to_value(&updated) {
                event_log.append(SyncEvent::TaskListUpdated(EventPayload {
                    id: updated.id.clone(),
                    fields,
                }));
            }
        }
        return Ok(());
    }

    // External containers (and local contact lists) — host-local override.
    let shared = db.shared();
    let repo = OverridesRepo::new(&shared);
    match color_label_id {
        Some(id) => repo.set_color_label(&container_id, kind, &id)?,
        None => repo.clear_color_label(&container_id, kind)?,
    }
    Ok(())
}

/// Set / clear a section's color label. Mirrors `set_container_color_label`:
/// a LOCAL section carries the binding on its own (synced) row — update it
/// and emit a `SectionUpdated` event so other devices follow. An EXTERNAL
/// section (Todoist section / Vikunja kanban bucket) has no provider color
/// field, so the binding is a host-local override the read path
/// (`get_sections`) stamps on top. `list_id` routes the call.
#[tauri::command]
pub async fn set_section_color(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    event_log: State<'_, Arc<EventLogWriter>>,
    db: State<'_, DbHandle>,
    section_id: String,
    list_id: String,
    color_label_id: Option<String>,
) -> CommandResult<()> {
    let account = registry
        .account_for_task_list(&list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        if let Some(mut section) = adapter.get_section_by_id(&section_id)? {
            section.color_label = color_label_id.map(ColorLabelId);
            let updated = adapter.update_section(section)?;
            if let Ok(fields) = serde_json::to_value(&updated) {
                event_log.append(SyncEvent::SectionUpdated(EventPayload {
                    id: updated.id.clone(),
                    fields,
                }));
            }
        }
        return Ok(());
    }
    // External section — host-local override (no event log).
    let shared = db.shared();
    let repo = OverridesRepo::new(&shared);
    match color_label_id {
        Some(id) => repo.set_section_color_label(&section_id, &id)?,
        None => repo.clear_section_color_label(&section_id)?,
    }
    Ok(())
}

/// Set / clear the host-local color override for an EXTERNAL event whose
/// calendar can't store a per-event color (iCloud, Graph, EWS, and any
/// CalDAV account without RFC 7986 COLOR support). The frontend routes
/// *color-capable* targets — local events and color-capable calendars —
/// through `update_event` instead (the color rides the event there), so
/// this command never touches the provider and never errors on the wire.
/// `event_id` is the series master id (the color applies to the series).
#[tauri::command]
pub async fn set_event_color(
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    db: State<'_, DbHandle>,
    event_id: String,
    calendar_id: String,
    color_label_id: Option<String>,
) -> CommandResult<()> {
    let account = registry
        .account_for_calendar(&calendar_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    // A color-capable external calendar (RFC 7986 COLOR) stores the color
    // natively through update_event, exactly like a local event keeps it on
    // its row. The frontend routes both through update_event, so this command
    // is a no-op for them — and short-circuiting here means even a stray call
    // can't create a host-local override that would compete with the native
    // value on read.
    let color_capable_external = account != LOCAL_ID
        && cache
            .read_calendars(&account)
            .ok()
            .into_iter()
            .flatten()
            .find(|c| c.id == calendar_id)
            .map(|c| c.supports_event_color)
            .unwrap_or(false);
    if account == LOCAL_ID || color_capable_external {
        return Ok(());
    }
    let shared = db.shared();
    let repo = OverridesRepo::new(&shared);
    match color_label_id {
        Some(id) => repo.set_event_color_label(&event_id, &id)?,
        None => repo.clear_event_color_label(&event_id)?,
    }
    Ok(())
}

/// Unified rename entry point. Returns a small status object so the
/// frontend can show "renamed at source" vs. "saved locally only".
#[derive(Debug, serde::Serialize)]
pub struct RenameOutcome {
    /// Whether the new name reached the source server. False means
    /// the adapter declared `Unsupported` and we wrote a local
    /// override instead — the frontend can use this to nudge the
    /// user about read-only sources.
    pub synced_to_source: bool,
}

#[tauri::command]
pub async fn rename_container(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    local: State<'_, LocalAdapter>,
    container_id: String,
    kind: ContainerKind,
    name: String,
) -> CommandResult<RenameOutcome> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CommandError {
            code: "invalid_input",
            message: "name must not be empty".into(),
        });
    }

    // Route by the same map the read paths use. If a container is
    // unknown to the registry (e.g. a route note was never recorded),
    // assume it belongs to the local adapter — matches the legacy
    // behaviour for pre-6b containers.
    let account = match kind {
        ContainerKind::Calendar => registry
            .account_for_calendar(&container_id)
            .unwrap_or_else(|| LOCAL_ID.to_string()),
        ContainerKind::TaskList => registry
            .account_for_task_list(&container_id)
            .unwrap_or_else(|| LOCAL_ID.to_string()),
        ContainerKind::ContactList => registry
            .account_for_contact_list(&container_id)
            .unwrap_or_else(|| LOCAL_ID.to_string()),
    };

    // Address books have no source-rename path here — only calendars and
    // task lists round-trip a rename to their adapter.
    let unsupported_contact_rename =
        || cal_core::Error::Unsupported("renaming address books is not supported".into());

    let push_result: cal_core::Result<()> = if account == LOCAL_ID {
        // Local SQLite — typed adapter handle, not a trait object.
        match kind {
            ContainerKind::Calendar => local.rename_calendar(&container_id, trimmed).await,
            ContainerKind::TaskList => local.rename_task_list(&container_id, trimmed).await,
            ContainerKind::ContactList => Err(unsupported_contact_rename()),
        }
    } else {
        match kind {
            ContainerKind::Calendar => {
                if let Some(ext) = registry.calendar_adapter(&account) {
                    ext.rename_calendar(&container_id, trimmed).await
                } else {
                    Err(cal_core::Error::NotFound(format!(
                        "no adapter registered for account '{account}'"
                    )))
                }
            }
            ContainerKind::TaskList => {
                if let Some(ext) = registry.task_adapter(&account) {
                    ext.rename_task_list(&container_id, trimmed).await
                } else {
                    Err(cal_core::Error::NotFound(format!(
                        "no adapter registered for account '{account}'"
                    )))
                }
            }
            ContainerKind::ContactList => Err(unsupported_contact_rename()),
        }
    };

    let shared = db.shared();
    let repo = OverridesRepo::new(&shared);

    match push_result {
        Ok(()) => {
            // Source accepted the rename. Clear any stale override
            // so the source name (now matching) is the single
            // truth. Failures here are non-fatal — the source is
            // already updated; a stale override would merely shadow
            // it with the same string.
            if let Err(err) = repo.clear(&container_id, kind) {
                tracing::warn!(
                    ?err,
                    container_id = %container_id,
                    "clearing override after server rename failed; non-fatal"
                );
            }
            Ok(RenameOutcome {
                synced_to_source: true,
            })
        }
        Err(cal_core::Error::Unsupported(_)) => {
            // Read-only source. Fall back to a local override —
            // that's the only place the new name can live.
            repo.set(&container_id, kind, trimmed)?;
            Ok(RenameOutcome {
                synced_to_source: false,
            })
        }
        Err(other) => Err(other.into()),
    }
}

impl From<OverridesError> for CommandError {
    fn from(err: OverridesError) -> Self {
        match err {
            OverridesError::EmptyName => CommandError {
                code: "invalid_input",
                message: err.to_string(),
            },
            OverridesError::Sqlite(e) => CommandError {
                code: "internal",
                message: e.to_string(),
            },
        }
    }
}
