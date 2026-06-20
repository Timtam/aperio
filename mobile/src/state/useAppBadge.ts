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
    // On today? (all-day events spread across covered days; timed events anchor
    // to their start day — same rule the Day view uses.)
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

// One compute at a time across all the triggers (a burst of them coalesces).
let inFlight = false;

/**
 * Mount ONCE inside the TaskStore provider (it reads dataVersion). Keeps the
 * app-icon badge in sync with today's load: recomputes on local mutation, on a
 * background-cache refresh, on foreground-resume (time advanced / day rolled),
 * on the local-midnight flip, and whenever the device-local toggle changes.
 */
export function useAppBadge(): void {
  const { dataVersion } = useTaskStore();
  const dayKey = useCurrentDayKey();

  const recompute = useCallback(async () => {
    if (inFlight) return;
    inFlight = true;
    try {
      const count = appBadgeEnabled() ? await computeBadgeCount() : 0;
      await setBadgeCountAsync(count);
    } catch {
      // Best-effort — a bridge hiccup or a denied permission must never crash
      // the app shell; the badge just stays at its last value.
    } finally {
      inFlight = false;
    }
  }, []);

  // Load the stored toggle on mount, then recompute; and recompute whenever the
  // toggle flips in Settings (subscribeAppBadge fires on persist).
  useEffect(() => {
    void loadAppBadgePref().then(() => recompute());
    return subscribeAppBadge(() => void recompute());
  }, [recompute]);

  // Local mutation (dataVersion) + midnight rollover (dayKey) re-evaluate today.
  useEffect(() => {
    void recompute();
  }, [dataVersion, dayKey, recompute]);

  // Foreground-resume: time has advanced (events may have ended; the day may
  // have rolled while backgrounded). iOS suspends background JS, so this is the
  // moment the badge the user sees on the icon gets refreshed.
  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') void recompute();
    });
    return () => sub.remove();
  }, [recompute]);

  // An external-cache warm pass brought fresh tasks / events.
  useCacheReload('tasks', recompute);
  useCacheReload('calendar', recompute);
}
