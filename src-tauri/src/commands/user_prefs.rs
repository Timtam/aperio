//! Generic user-preferences key/value commands.
//!
//! Three verbs:
//!
//!   - `get_user_pref(key)` — returns the stored value, or `null`
//!     when nothing is set.
//!   - `set_user_pref(key, value)` — upsert.
//!   - `delete_user_pref(key)` — drop the row.
//!
//! Values are opaque strings. The frontend serialises JSON when it
//! needs structure (sidebar tree state, view defaults, …); the
//! command layer doesn't interpret the content.

use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
use crate::user_prefs::{UserPrefsError, UserPrefsRepo};

#[tauri::command]
pub async fn get_user_pref(
    db: State<'_, DbHandle>,
    key: String,
) -> CommandResult<Option<String>> {
    let shared = db.shared();
    let repo = UserPrefsRepo::new(&shared);
    Ok(repo.get(&key)?)
}

#[tauri::command]
pub async fn set_user_pref(
    db: State<'_, DbHandle>,
    key: String,
    value: String,
) -> CommandResult<()> {
    let shared = db.shared();
    let repo = UserPrefsRepo::new(&shared);
    repo.set(&key, &value)?;
    Ok(())
}

#[tauri::command]
pub async fn delete_user_pref(
    db: State<'_, DbHandle>,
    key: String,
) -> CommandResult<()> {
    let shared = db.shared();
    let repo = UserPrefsRepo::new(&shared);
    repo.delete(&key)?;
    Ok(())
}

impl From<UserPrefsError> for CommandError {
    fn from(err: UserPrefsError) -> Self {
        match err {
            UserPrefsError::Sqlite(e) => CommandError {
                code: "internal",
                message: e.to_string(),
            },
        }
    }
}
