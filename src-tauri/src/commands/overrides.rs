//! Container name override commands.
//!
//! Thin Tauri wrapper around [`crate::overrides::OverridesRepo`]. Two
//! verbs are enough:
//!
//!   - `set_container_name_override(id, kind, name)` — rename, upsert.
//!   - `clear_container_name_override(id, kind)` — revert to the
//!     source name.
//!
//! Both flush nothing to the source server; pushing the rename out
//! to CalDAV / Google / local is the follow-up task that hangs off
//! the future `rename_calendar` trait.

use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
use crate::overrides::{ContainerKind, OverridesError, OverridesRepo};

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
