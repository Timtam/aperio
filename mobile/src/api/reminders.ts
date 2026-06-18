// Mobile reminders api-client — JSON passthrough over the cal-ffi Host's
// `upcoming_reminders_json`, which runs the SAME `host_core::reminders`
// enumeration the desktop scheduler uses (one source of truth for what fires
// when). The on-device scheduler turns these into OS local notifications.

import CalFfi from '../../modules/cal-ffi';

/** One upcoming reminder occurrence (the Host wire shape). */
export interface UpcomingReminder {
  item_id: string;
  item_kind: 'event' | 'task';
  title: string;
  body: string;
  /** RFC-3339 UTC instant the notification should fire at. */
  trigger_at: string;
}

/** Upcoming reminder triggers within `horizonMinutes` from now — local +
 *  external sources, sorted ascending, future-only, deduped. */
export const upcomingReminders = async (
  horizonMinutes: number,
): Promise<UpcomingReminder[]> =>
  JSON.parse(await CalFfi.upcomingRemindersJson(horizonMinutes)) as UpcomingReminder[];
