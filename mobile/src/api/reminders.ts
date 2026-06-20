// Mobile reminders api-client — JSON passthrough over the cal-ffi Host's
// `upcoming_reminders_json`, which runs the SAME `host_core::reminders`
// enumeration the desktop scheduler uses (one source of truth for what fires
// when). The on-device scheduler turns these into OS local notifications.

import CalFfi from '../../modules/cal-ffi';

/** A notification sound source (the cal_core `SoundSource` wire shape). `custom`
 *  references an audio file by content hash in the sync asset store (which the
 *  mobile host doesn't carry yet — the scheduler falls back to the default
 *  sound). */
export type SoundSource =
  | { type: 'system' }
  | { type: 'silent' }
  | { type: 'custom'; sha256: string };

/** Effective notification sound (the cal_core `SoundConfig`), already resolved
 *  through the §14.4 hierarchy (reminder → item → container → global). `volume`
 *  is desktop-only — OS notifications use the system volume. */
export interface SoundConfig {
  source: SoundSource;
  volume: number;
}

/** One upcoming reminder occurrence (the Host wire shape). */
export interface UpcomingReminder {
  item_id: string;
  item_kind: 'event' | 'task';
  /** The owning container — a task's list id / an event's calendar id. Routes a
   *  tap on the overview row to the underlying item's editor. */
  container_id: string;
  title: string;
  body: string;
  /** RFC-3339 UTC instant the notification should fire at. */
  trigger_at: string;
  /** The effective notification sound for this trigger. */
  sound: SoundConfig;
}

/** Upcoming reminder triggers within `horizonMinutes` from now — local +
 *  external sources, sorted ascending, future-only, deduped. */
export const upcomingReminders = async (
  horizonMinutes: number,
): Promise<UpcomingReminder[]> =>
  JSON.parse(await CalFfi.upcomingRemindersJson(horizonMinutes)) as UpcomingReminder[];
