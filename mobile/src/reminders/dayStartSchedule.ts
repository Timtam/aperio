import AsyncStorage from '@react-native-async-storage/async-storage';

import type { Task, TaskUser } from '@aperio/shared';
import { buildReminderGroups, reminderCount, todayIsoKey } from '@aperio/shared';

import { getTasks } from '../api/client';
import { currentUserForList } from '../state/currentUser';
import { readDayStartSnoozeUntil } from '../state/dayStartSnooze';
import { STORAGE_KEY, type PersistedSelection } from '../state/selection';
import { readTaskBehaviour } from '../state/taskBehaviour';

// Ahead-of-time DAY-START notifications. The in-app day-start checks
// (useDayStartChecks) only run while the app is OPEN — with the app closed at
// the dayStartTrigger time, nothing fired and the "today's tasks" reminder
// arrived only when the user next opened the app. This module computes, for
// each day in the scheduler's horizon whose HH:MM day-start instant is still in
// the future, the de-duplicated reminder count for that day (the SAME shared
// buildReminderGroups the in-app review + desktop checker use, anchored to that
// day), so the reminder scheduler can register them as OS notifications that
// fire with the app killed.
//
// The count is computed from the data available at SCHEDULING time (launch /
// foreground / after a mutation) — a later edit can make it slightly stale by
// the time it fires. That's fine: the notification is the wake-up nudge;
// opening the app runs the live in-app review (announce + dialog) with the
// real numbers, exactly as before. 'app-start' triggers schedule nothing (that
// mode means "fire when I open the app", which the in-app check already does).

export interface DayStartNotification {
  /** The local day-start instant to fire at (strictly in the future). */
  triggerAt: Date;
  /** De-duplicated task count across the three reminder groups for that day. */
  count: number;
}

/**
 * Whether `trigger` gets ahead-of-time OS notifications: an explicit `HH:MM`
 * other than `'00:00'`. `'00:00'` is the DEFAULT ("at the date rollover") and
 * on mobile has always meant "on the first open of the day" — pre-scheduling
 * it would push a literal MIDNIGHT notification (with sound) at every
 * default-config user, nightly; the morning `HH:MM` options exist precisely so
 * nobody is woken at midnight. `'app-start'` fires on open by definition. The
 * in-app check keeps posting its own immediate notification for BOTH excluded
 * modes, so OS delivery has exactly one owner per mode.
 */
export function dayStartPreschedulesOsNotification(trigger: string): boolean {
  if (trigger === '00:00') return false;
  const m = /^(\d{1,2}):(\d{2})$/.exec(trigger);
  if (!m) return false;
  return Number(m[1]) <= 23 && Number(m[2]) <= 59;
}

/** The persisted task-list selection (the day-start review's scope — mirrors
 *  useTasks/useDayStartChecks reading the SELECTED lists). Empty/unreadable ⇒
 *  no selection ⇒ no notifications (the in-app review is gated the same way). */
async function readSelectedListIds(): Promise<string[]> {
  try {
    const raw = await AsyncStorage.getItem(STORAGE_KEY);
    if (raw == null) return [];
    const parsed = JSON.parse(raw) as PersistedSelection;
    return parsed.taskLists ?? [];
  } catch {
    return [];
  }
}

/** Resolve "me" per list (session-cached) for the ownership filter — only my
 *  own / unassigned tasks are counted. Mirrors useDayStartChecks.meForTasks. */
async function meForTasks(tasks: Task[]): Promise<(listId: string) => TaskUser | null> {
  const ids = Array.from(new Set(tasks.map((task) => task.list_id)));
  const entries = await Promise.all(
    ids.map(async (id) => [id, await currentUserForList(id)] as const),
  );
  const map = Object.fromEntries(entries) as Record<string, TaskUser | null>;
  return (listId: string) => map[listId] ?? null;
}

/** `YYYY-MM-DD` for `offsetDays` after today (local calendar). */
function dayKeyPlus(offsetDays: number): string {
  const [y, m, d] = todayIsoKey().split('-').map(Number);
  const date = new Date(y, m - 1, d + offsetDays);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** The local instant of `HH:MM` on the `YYYY-MM-DD` day. */
function dayInstant(dayKey: string, hours: number, minutes: number): Date {
  const [y, m, d] = dayKey.split('-').map(Number);
  return new Date(y, m - 1, d, hours, minutes, 0, 0);
}

/**
 * The day-start notifications to pre-schedule for the next `horizonDays` days
 * (today included when its instant hasn't passed). Empty when the trigger is
 * 'app-start'/unparseable, every reminder toggle is off, no lists are
 * selected, or no day in the horizon has anything to remind about.
 */
export async function upcomingDayStartNotifications(
  horizonDays: number,
): Promise<DayStartNotification[]> {
  const behaviour = await readTaskBehaviour();
  const trigger = behaviour.dayStartTrigger;
  // 'app-start' / the '00:00' default / unparseable: the in-app check owns
  // delivery (see dayStartPreschedulesOsNotification). This early-out also
  // spares default-config users the per-reschedule task fan-out below.
  if (!dayStartPreschedulesOsNotification(trigger)) return [];
  const m = /^(\d{1,2}):(\d{2})$/.exec(trigger);
  if (!m) return [];
  const hours = Number(m[1]);
  const minutes = Number(m[2]);
  const settings = {
    remindUntimedToday: behaviour.remindUntimedToday,
    remindDeadlineArrived: behaviour.remindDeadlineArrived,
    remindDeadlineCountdown: behaviour.remindDeadlineCountdown,
    deadlineCountdownDays: behaviour.deadlineCountdownDays,
  };
  if (
    !settings.remindUntimedToday &&
    !settings.remindDeadlineArrived &&
    !settings.remindDeadlineCountdown
  ) {
    return [];
  }

  // NB: this reads the raw persisted selection blob; the in-app review uses
  // the store's RECONCILED selection (which auto-selects never-seen lists).
  // On a fresh install / right after a peer syncs a NEW list in, the two can
  // briefly differ — it heals on the next reschedule pass after the store
  // persists. Accepted: the notification is a nudge, not the truth.
  const listIds = await readSelectedListIds();
  if (listIds.length === 0) return [];
  const per = await Promise.all(
    listIds.map((id) => getTasks(id).catch(() => [] as Task[])),
  );
  const tasks = per.flat();
  if (tasks.length === 0) return [];
  const meFor = await meForTasks(tasks);

  const now = new Date();
  // An active "remind me later" snooze defers the whole in-app batch — don't
  // let the pre-scheduled notification fire into that window either (a tap
  // would open an app whose review stays silent until the snooze ends).
  // Best-effort: a snooze set AFTER this pass isn't seen until the next one.
  const snoozeUntil = (await readDayStartSnoozeUntil()) ?? 0;
  const out: DayStartNotification[] = [];
  for (let offset = 0; offset <= horizonDays; offset += 1) {
    const dayKey = dayKeyPlus(offset);
    const triggerAt = dayInstant(dayKey, hours, minutes);
    if (triggerAt.getTime() <= now.getTime()) continue;
    if (triggerAt.getTime() <= snoozeUntil) continue;
    const count = reminderCount(buildReminderGroups(tasks, settings, meFor, dayKey));
    if (count > 0) out.push({ triggerAt, count });
  }
  return out;
}
