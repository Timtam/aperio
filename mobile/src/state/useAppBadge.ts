import { useCallback, useEffect } from 'react';
import { AppState } from 'react-native';
import { setBadgeCountAsync } from 'expo-notifications';

import {
  eventCoversDay,
  expandAll,
  filterTasksOnDay,
  todayIsoKey,
} from '@aperio/shared';
import type { Task } from '@aperio/shared';

import { getEvents, listCalendars, type CalendarEvent } from '../api/calendar';
import { getTasks, listTaskLists } from '../api/client';
import { useCurrentDayKey } from '../hooks/useCurrentDayKey';
import { appBadgeEnabled, loadAppBadgePref, subscribeAppBadge } from './appBadge';
import { useCacheReload } from './cacheObserver';
import { subscribeCalendarChanged } from './calendarMutations';
import { whenStartupSettled } from './startupGate';
import { useTaskStore } from './taskStoreContext';

// The app-icon badge: how much is on the plate TODAY = open tasks due today +
// events still ahead today. Computed at the app root and pushed to the OS via
// expo-notifications. Semantics reuse the SAME shared helpers the Day view uses,
// so the badge always matches what "today" shows in-app:
//   - tasks: filterTasksOnDay (scheduled_date OR deadline_date == today),
//     excluding subtasks, cancelled, and completed — across ALL lists, not just
//     the selected ones (the badge is a whole-app signal).
//   - events: today's occurrences (recurrence expanded via expandAll) that are
//     all-day OR haven't ended yet ("running or upcoming"), across ALL calendars.
// All date maths is LOCAL wall-clock (todayIsoKey / local-midnight Date), never a
// toISOString().slice that would shift the day across the UTC boundary.
//
// The badge needs notification permission (iOS); it piggybacks on the permission
// the reminders scheduler already requests — it never prompts on its own. With
// no permission, setBadgeCountAsync is a silent no-op.

/** Open tasks (across every list) that land on today. */
async function countTodayTasks(today: string): Promise<number> {
  const lists = await listTaskLists();
  const per = await Promise.all(
    lists.map((l) => getTasks(l.id).catch(() => [] as Task[])),
  );
  // isCompletedVisible => false so completed (and cancelled) never count; the
  // helper also drops subtasks (parent_id set).
  return filterTasksOnDay(per.flat(), today, () => false).length;
}

/** Today's events that are all-day or still ahead (running or upcoming), across
 *  every calendar. Recurrence is expanded client-side (the backend returns the
 *  unexpanded master), so a daily series counts its today occurrence. */
async function countTodayEvents(
  dayStart: Date,
  dayEnd: Date,
  now: Date,
): Promise<number> {
  const cals = await listCalendars();
  const start = dayStart.toISOString();
  const end = dayEnd.toISOString();
  const per = await Promise.all(
    cals.map((c) =>
      getEvents({ calendar_id: c.id, start, end }).catch(() => [] as CalendarEvent[]),
    ),
  );
  const expanded = expandAll(per.flat(), { start: dayStart, end: dayEnd });
  const nowMs = now.getTime();
  // Distinct occurrence ids (expandAll yields unique `master@iso` ids); a single
  // event belongs to one calendar so cross-calendar dups don't arise, but the
  // Set guards against any accidental double-count.
  const ids = new Set<string>();
  for (const ev of expanded) {
    // On today? eventCoversDay spreads BOTH all-day and timed events across
    // every covered day — so a meeting that started at 23:00 yesterday and is
    // still running at 00:30 today counts toward today (its tail covers today)
    // until it ends, which is intended for a "what's on the plate today" badge.
    // Same rule the Day view uses.
    if (!eventCoversDay(ev, now)) continue;
    // Timed events that already ended today drop out ("upcoming/running" only);
    // all-day events have no intraday end, so they count all day.
    if (!ev.all_day && new Date(ev.end).getTime() <= nowMs) continue;
    ids.add(ev.id);
  }
  return ids.size;
}

async function computeBadgeCount(): Promise<number> {
  const today = todayIsoKey();
  const now = new Date();
  const dayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 0, 0, 0, 0);
  const dayEnd = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 23, 59, 59, 999);
  const [tasks, events] = await Promise.all([
    countTodayTasks(today),
    countTodayEvents(dayStart, dayEnd, now),
  ]);
  return tasks + events;
}

// One compute at a time. A trigger that arrives mid-compute sets `rerun` so the
// guard loops once more with fresh data instead of DROPPING the update — every
// run carries fresh counts (unlike the day-start fire-marker, where a dropped
// run is a safe idempotent no-op), so a dropped run genuinely loses the latest
// state. The load-bearing case: a toggle-off that races an in-flight compute
// would otherwise leave the badge stuck non-zero.
let inFlight = false;
let rerun = false;

async function runBadge(): Promise<void> {
  if (inFlight) {
    rerun = true;
    return;
  }
  inFlight = true;
  try {
    do {
      rerun = false;
      try {
        const count = appBadgeEnabled() ? await computeBadgeCount() : 0;
        await setBadgeCountAsync(count);
      } catch {
        // Best-effort — a bridge hiccup or a denied permission must never crash
        // the app shell; the badge keeps its last value. Loop again if a newer
        // trigger arrived during the failed pass.
      }
    } while (rerun);
  } finally {
    inFlight = false;
  }
}

/**
 * Mount ONCE inside the TaskStore provider (it reads dataVersion). Keeps the
 * app-icon badge in sync with today's load: recomputes on local task mutation,
 * local event write, background-cache refresh, foreground-resume (time advanced
 * / day rolled), the local-midnight flip, and the device-local toggle.
 */
export function useAppBadge(): void {
  const { dataVersion } = useTaskStore();
  const dayKey = useCurrentDayKey();

  // Every trigger routes through the startup gate: during the launch
  // window the (many) mount + cache-flush triggers collapse into ONE
  // deferred compute, so the badge's full-catalog fan-out doesn't queue
  // ahead of the visible screen's first read on the serial native queue.
  // Once the gate is open this is a plain pass-through.
  const recompute = useCallback(() => {
    whenStartupSettled('appBadge', () => void runBadge());
  }, []);

  // Load the stored toggle on mount, then recompute; and recompute whenever the
  // toggle flips in Settings (subscribeAppBadge fires on persist).
  useEffect(() => {
    void loadAppBadgePref().then(() => recompute());
    return subscribeAppBadge(recompute);
  }, [recompute]);

  // Local task mutation (dataVersion) + midnight rollover (dayKey) re-evaluate.
  useEffect(() => {
    recompute();
  }, [dataVersion, dayKey, recompute]);

  // Foreground-resume: time has advanced (events may have ended; the day may
  // have rolled while backgrounded). iOS suspends background JS, so this is the
  // moment the badge the user sees on the icon gets refreshed.
  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') recompute();
    });
    return () => sub.remove();
  }, [recompute]);

  // Local EVENT writes (the task store's dataVersion doesn't cover the event
  // editor / delete paths) + external-cache warm passes (fresh tasks / events).
  useEffect(() => subscribeCalendarChanged(recompute), [recompute]);
  useCacheReload('tasks', recompute);
  useCacheReload('calendar', recompute);
}
