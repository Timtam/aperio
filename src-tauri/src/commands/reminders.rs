//! Reminders overview command.

use tauri::State;

use super::CommandResult;
use crate::reminders::{SchedulerHandle, UpcomingReminder};

/// Maximum number of triggers the overview dialog will display in
/// one go. The user can refine the visible date range later; for
/// the first iteration a comfortable scrolling length is enough.
const OVERVIEW_LIMIT: usize = 100;

#[tauri::command]
pub async fn list_upcoming_reminders(
    scheduler: State<'_, SchedulerHandle>,
) -> CommandResult<Vec<UpcomingReminder>> {
    // Pull the Arc out of the State guard before awaiting — the State
    // borrow can't cross the await point. Cloning the Arc is a refcount
    // bump; the underlying scheduler is the same.
    let scheduler = SchedulerHandle::clone(&scheduler);
    Ok(scheduler.upcoming(OVERVIEW_LIMIT).await)
}
