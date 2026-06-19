import { useEffect } from 'react';
import { AccessibilityInfo, AppState } from 'react-native';

import { filterDeadlinePinTargets, shouldFireToday, todayIsoKey } from '@aperio/shared';
import type { Task } from '@aperio/shared';
import i18n from '../../i18n';

import { getTasks, listTaskLists, updateTask } from '../api/client';
import { readFiredDayKey, writeFiredDayKey } from './dayStartFired';
import { readTaskBehaviour } from './taskBehaviour';
import { useTaskStore } from './taskStoreContext';

// The mobile day-start checks — the screen-reader-first twin of the desktop's
// DeadlinePinChecker (+ later the DayStartReview). The desktop fires from a live
// minute-poller; iOS suspends background JS, so mobile runs the checks on launch
// + every foreground-resume (the same model as the reminder rescheduler), gated
// by the synced dayStartTrigger pref + a per-device per-slot fire-marker so a
// day's batch runs at most once. All best-effort + silent on failure.

/** Every task across the user's lists (for the cross-list selectors). */
async function loadAllTasks(): Promise<Task[]> {
  const lists = await listTaskLists();
  const per = await Promise.all(lists.map((l) => getTasks(l.id).catch(() => [] as Task[])));
  return per.flat();
}

/**
 * Silent "by"-deadline auto-pin: tasks whose deadline is today (and aren't
 * already scheduled for today) get pinned to today so they surface on today's
 * calendar lanes. Gated by dayStartTrigger + the 'deadlinePin' fire-marker. The
 * marker is written BEFORE applying (idempotent — a partial run isn't re-fired).
 */
async function runDeadlinePin(invalidateData: () => void): Promise<void> {
  const behaviour = await readTaskBehaviour();
  const todayKey = todayIsoKey();
  const fired = await readFiredDayKey('deadlinePin');
  if (!shouldFireToday(behaviour.dayStartTrigger, fired, todayKey)) return;
  const all = await loadAllTasks();
  await writeFiredDayKey('deadlinePin', todayKey);
  const targets = filterDeadlinePinTargets(all);
  if (targets.length === 0) return;
  for (const task of targets) {
    // Pin to today; leave scheduled_time untouched ("by 14:30" ≠ "at 14:30").
    await updateTask({ ...task, scheduled_date: todayKey });
  }
  AccessibilityInfo.announceForAccessibility(
    i18n.t('dialogs.deadlinePin.announce', { count: targets.length }),
  );
  invalidateData();
}

// One run at a time across launch + the foreground listener.
let inFlight = false;

/** Mount once inside the TaskStore provider: run the day-start checks on launch
 *  + every foreground-resume (the latter catches a date rollover while away). */
export function useDayStartChecks(): void {
  const { invalidateData } = useTaskStore();
  useEffect(() => {
    const run = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        await runDeadlinePin(invalidateData);
      } catch {
        // Best-effort — a bridge hiccup must never crash launch/foreground.
      } finally {
        inFlight = false;
      }
    };
    void run();
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') void run();
    });
    return () => sub.remove();
  }, [invalidateData]);
}
