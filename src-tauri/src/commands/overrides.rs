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
use cal_core::{CalendarFeature, TasksFeature};
use std::sync::Arc;
use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
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
    };

    let push_result: cal_core::Result<()> = if account == LOCAL_ID {
        // Local SQLite — typed adapter handle, not a trait object.
        match kind {
            ContainerKind::Calendar => {
                local.rename_calendar(&container_id, trimmed).await
            }
            ContainerKind::TaskList => {
                local.rename_task_list(&container_id, trimmed).await
            }
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
