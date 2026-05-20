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
    Ok(scheduler.upcoming(OVERVIEW_LIMIT))
}
