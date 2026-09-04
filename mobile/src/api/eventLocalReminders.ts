import type { Reminder } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi/src/CalFfiModule';
import { scheduleBackgroundPush } from './syncTriggers';

/**
 * Reminders Aperio keeps for ONE event and tells no provider about
 * (migration 0043).
 *
 * A reminder normally rides on the event: Aperio writes it into the
 * appointment, the provider stores it, and every other client of that calendar
 * rings too — the iOS Calendar app, a voice assistant reading the account out
 * loud. These do not. They fire in Aperio, travel to the user's other devices
 * through Aperio's own sync, and reach nobody else on a shared calendar.
 *
 * Rows are keyed by the SERIES master id, like colour overrides and group
 * membership: a recurring appointment is reminded of as a series.
 */
export interface EventLocalReminders {
  calendar_id: string;
  event_id: string;
  /** An empty list is a decision ("none of my own here"), not an absence. */
  reminders: Reminder[];
  /** The title the event had when this was set. Half of the SIGNATURE, so the
   *  row can find its event again after the provider remints the id. */
  title: string;
  /** The start it had then. The other half. */
  starts_at: string;
  updated_at: string;
}

/** Every row. Small by nature — one per event with a private reminder. */
export const listEventLocalReminders = async (): Promise<EventLocalReminders[]> =>
  JSON.parse(await CalFfi.eventLocalRemindersJson()) as EventLocalReminders[];

/** Write one event's private reminders. `title`/`startsAt` are the event's
 *  CURRENT signature. An empty list is stored, not deleted. */
export const setEventLocalReminders = async (payload: {
  calendar_id: string;
  event_id: string;
  reminders: Reminder[];
  title: string;
  starts_at: string;
}): Promise<EventLocalReminders> => {
  const row = JSON.parse(
    await CalFfi.setEventLocalRemindersJson(
      payload.calendar_id,
      payload.event_id,
      JSON.stringify(payload.reminders),
      payload.title,
      payload.starts_at,
    ),
  ) as EventLocalReminders;
  // A synced record: push it now rather than at the next periodic round, the
  // way every other mobile mutation does.
  scheduleBackgroundPush();
  return row;
};

/** Point a row at the id its event carries now.
 *
 *  A repair of Aperio's own bookkeeping — the same appointment before and
 *  after — so it is applied silently by whichever view noticed and never
 *  announced as a change the user made. */
export const healEventLocalReminders = (payload: {
  calendar_id: string;
  old_event_id: string;
  new_event_id: string;
}): Promise<boolean> =>
  CalFfi.healEventLocalReminders(
    payload.calendar_id,
    payload.old_event_id,
    payload.new_event_id,
  );

/** Write down what the event looks like now, so the signature keeps matching
 *  after the user renames or moves the appointment. Silent, like the repair. */
export const refreshEventLocalReminderSignature = (payload: {
  calendar_id: string;
  event_id: string;
  title: string;
  starts_at: string;
}): Promise<void> =>
  CalFfi.refreshEventLocalReminderSignature(
    payload.calendar_id,
    payload.event_id,
    payload.title,
    payload.starts_at,
  );
