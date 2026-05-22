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

/// Invalidate the reminder scheduler so it re-scans on the next
/// tick. Clears the external-trigger cache too, since the change
/// that triggered the invalidation (most commonly a per-calendar
/// "Standard-Hinweis" edit in Settings → Kalender) affects how
/// external events resolve to Triggers — without the cache flush
/// the new default wouldn't reach the firing loop until the TTL
/// expires (~5 min).
///
/// Cheap on the wire (no payload, no async work besides the
/// fire-and-forget notify) so callers can lean on it whenever
/// they've touched something the scheduler reads.
#[tauri::command]
pub async fn invalidate_reminders(
    scheduler: State<'_, SchedulerHandle>,
) -> CommandResult<()> {
    scheduler.invalidate_external_cache();
    scheduler.invalidate();
    Ok(())
}
