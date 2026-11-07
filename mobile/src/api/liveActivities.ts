// Live Activities for events (DESIGN-live-activities.md) — JSON passthrough
// over the cal-ffi Host's `live_activity_plan_json`. One reminder scan answers
// with BOTH the notification list and the plan, so the two can never disagree
// about which reminders the activity has taken over.

import type { LiveActivityPlan } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';
import type { UpcomingReminder } from './reminders';

/** A notification-list entry, as `upcomingReminders` gives it, plus whether a
 *  section of the plan IS this reminder. */
export interface PlannedReminder extends UpcomingReminder {
  /** Decided in the core, on instants. Only once the platform has taken the
   *  plan over may an absorbed reminder be left out of the notifications. */
  absorbed: boolean;
}

export interface LiveActivityAnswer {
  reminders: PlannedReminder[];
  plan: LiveActivityPlan;
}

/** The notification list and the Live Activity plan within `horizonMinutes`
 *  from now. `hiddenCalendars` are the calendars hidden on this device: they
 *  open no window (their reminders stay in the list, as everywhere, and the
 *  scheduler filters them as it always has). */
export const liveActivityPlan = async (
  horizonMinutes: number,
  hiddenCalendars: readonly string[],
): Promise<LiveActivityAnswer> =>
  JSON.parse(
    await CalFfi.liveActivityPlanJson(horizonMinutes, JSON.stringify(hiddenCalendars)),
  ) as LiveActivityAnswer;
