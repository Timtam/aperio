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
pub async fn invalidate_reminders(scheduler: State<'_, SchedulerHandle>) -> CommandResult<()> {
    scheduler.invalidate_external_cache();
    scheduler.invalidate();
    Ok(())
}

/// Push the notification wording to the reminder scheduler.
///
/// The host has no i18n — the same reason `set_tray_labels` exists. Reminders
/// are where that showed as a WRONG statement rather than an English one: every
/// body was formatted `%H:%M`, and an all-day event starts at local midnight,
/// so the desktop announced "00:00" for a birthday. Mobile has said "Ganztägig"
/// since it shipped.
///
/// Month names come from the frontend's `Intl` and `day_month` carries the
/// order ("24. Juni" against "June 24"), because deriving either here would
/// mean a date-formatting library and a second answer to "which language is
/// this" — and the frontend already holds both.
///
/// Called once i18n is ready and again on every language change, like
/// `set_tray_labels`. Until it arrives an all-day body is omitted rather than
/// guessed.
#[tauri::command]
pub async fn set_reminder_labels(
    scheduler: State<'_, SchedulerHandle>,
    all_day: String,
    all_day_range: String,
    day_month: String,
    months: Vec<String>,
) -> CommandResult<()> {
    scheduler.set_labels(crate::reminders::ReminderLabels {
        all_day,
        all_day_range,
        day_month,
        months,
    });
    Ok(())
}

/// Push the set of hidden (sidebar-unchecked) calendar ids to the reminder
/// scheduler so event reminders on those calendars are suppressed — hiding a
/// calendar silences its reminders too. The frontend calls this on startup and
/// whenever the sidebar calendar selection changes. The set is device-local (it
/// mirrors the localStorage selection) and never synced.
#[tauri::command]
pub async fn set_reminder_hidden_calendars(
    scheduler: State<'_, SchedulerHandle>,
    hidden_calendar_ids: Vec<String>,
) -> CommandResult<()> {
    scheduler.set_hidden_calendars(hidden_calendar_ids);
    Ok(())
}
