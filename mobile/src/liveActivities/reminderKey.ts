import { triggerKeyId } from '@aperio/shared';

import type { UpcomingReminder } from '../api/reminders';

/** A reminder of the notification list as a trigger key id: the plan and the
 *  list spell the same instant differently, so they meet as numbers. */
export function reminderKeyId(r: UpcomingReminder): string {
  return triggerKeyId({
    item_id: r.item_id,
    calendar_id: r.container_id,
    occurrence_start: r.start,
    fire_at: r.trigger_at,
  });
}
