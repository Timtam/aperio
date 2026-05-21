import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/Announcer';
import type { Task } from '../api/types';
import {
  readFiredDayKey,
  shouldFireToday,
  useCurrentDayKey,
  writeFiredDayKey,
} from '../hooks/useCurrentDayKey';
import { todayIsoKey } from '../intl/taskDay';
import { useDialogState } from '../state/DialogState';
import {
  useTaskCascadeEnabled,
  type CarryOverDefault,
} from '../state/TaskCascadeProvider';
import { useTasks } from '../state/useTasks';
import {
  actionableDescendants,
  filterCarriedOver,
  isCarryOverSnoozed,
} from './CarryOverDialog';

/**
 * Day-start gate for the carry-over flow. The Settings → Tasks
 * "Übernahme-Standard" preference decides what happens:
 *
 *   - `ask`  (default) — pushes the carry-over dialog so the user
 *     can decide per row.
 *   - `today` — silently sets `scheduled_date = today` on every
 *     slipped task (and, when cascade-coupling is on, its actionable
 *     descendants). Announces the result via the live region; no
 *     dialog flashes.
 *   - `backlog` — same as above but clears `scheduled_date`.
 *
 * **Re-trigger semantics.** Used to be mount-once. Now driven by
 * `useCurrentDayKey()` so an always-on app re-checks when the local
 * date rolls over. The `firedRef` stores the date key we last fired
 * on; a different key means a new day and we try again. The Settings
 * → Tasks `dayStartTrigger` preference further gates *when* on the
 * new day to fire (immediately at midnight, at a fixed morning hour,
 * or only on app start — see `shouldFireToday`).
 *
 * **Snooze.** The four-hour suppress flag from the dialog's "Später
 * erinnern" button silences every mode equally. Crucially the snooze
 * bail does NOT mark `firedRef` — the poller retries on the next
 * tick, so the moment the snooze expires the gate runs again.
 *
 * **Dialog guard.** If any modal is already open (e.g. the user is
 * editing a task), we skip this tick. The poller will try again. This
 * avoids silently rewriting fields under an open editor.
 */
export function CarryOverChecker() {
  const { tasks, loading } = useTasks();
  const { mode: dialogMode, openCarryOver, invalidateData } = useDialogState();
  const announce = useAnnouncer();
  const { t } = useTranslation();
  const {
    enabled: cascadeEnabled,
    carryOverDefault,
    dayStartTrigger,
    hydrating,
  } = useTaskCascadeEnabled();
  const todayKey = useCurrentDayKey();
  // Stores the YYYY-MM-DD we last fired on, or null when never fired.
  // Hydrated from localStorage on mount so a mid-day app restart
  // doesn't re-run the silent batch (and re-announce) for a day we
  // already processed. `shouldFireToday` consumes this together with
  // the trigger preference to decide if a new fire is due.
  const firedRef = useRef<string | null>(readFiredDayKey('carryOver'));

  useEffect(() => {
    // Wait for both the task catalog and the preferences round-trip
    // — otherwise a default-`ask` from the pre-hydration state would
    // open the dialog even for users who opted into auto-today /
    // auto-backlog.
    if (loading || hydrating) return;
    if (!shouldFireToday(dayStartTrigger, firedRef.current, todayKey)) {
      return;
    }
    // Don't pile a second carry-over dialog (or silent batch) on top
    // of whatever the user already has open. Tick again later when
    // the modal closes.
    if (dialogMode.kind !== 'none') return;
    // Snooze respects the user's "Später erinnern" choice. Do NOT
    // mark fired — when the snooze expires, the next tick should
    // run the gate properly.
    if (isCarryOverSnoozed()) return;

    const slipped = filterCarriedOver(tasks, { cascadeEnabled });
    // Even on an empty day we record the fire — the gate's only job
    // is "review for this day". If new slipped rows appear later
    // (sync, manual edit), this tick wouldn't have caught them
    // either; the user explicitly re-running carry-over via the
    // pending UI later is the answer there.
    firedRef.current = todayKey;
    writeFiredDayKey('carryOver', todayKey);
    if (slipped.length === 0) return;

    if (carryOverDefault === 'ask') {
      openCarryOver();
      return;
    }

    void runAutoBatch({
      action: carryOverDefault,
      slippedRoots: slipped,
      allTasks: tasks,
      cascadeEnabled,
      announce,
      t,
      invalidateData,
    });
  }, [
    loading,
    hydrating,
    tasks,
    cascadeEnabled,
    carryOverDefault,
    dayStartTrigger,
    todayKey,
    dialogMode.kind,
    openCarryOver,
    invalidateData,
    announce,
    t,
  ]);

  return null;
}

/**
 * Run a silent carry-over batch action. Collects every dialog row
 * plus, when cascade-coupling is on, its actionable descendants — the
 * same target set the dialog's "Alle auf heute" / "Alle in Backlog"
 * buttons would touch.
 */
async function runAutoBatch(args: {
  action: Exclude<CarryOverDefault, 'ask'>;
  slippedRoots: Task[];
  allTasks: Task[];
  cascadeEnabled: boolean;
  announce: (message: string) => void;
  t: (key: string, values?: Record<string, unknown>) => string;
  invalidateData: () => void;
}): Promise<void> {
  const { action, slippedRoots, allTasks, cascadeEnabled } = args;
  const collected = new Map<string, Task>();
  for (const root of slippedRoots) {
    collected.set(root.id, root);
    if (!cascadeEnabled) continue;
    for (const desc of actionableDescendants(root.id, allTasks)) {
      collected.set(desc.id, desc);
    }
  }
  const targets = [...collected.values()];
  if (targets.length === 0) return;

  const newDate = action === 'today' ? todayIsoKey() : null;
  const now = new Date().toISOString();
  try {
    await Promise.all(
      targets.map((task) =>
        invoke<Task>('update_task', {
          task: {
            ...task,
            scheduled_date: newDate,
            updated_at: now,
          },
        }),
      ),
    );
    // The count we announce is the number of *root* rows the user
    // would have seen in the dialog — descendants under coupling are
    // implementation detail and shouldn't pad the human-facing count.
    const announceKey =
      action === 'today'
        ? 'dialogs.carryOver.autoCarriedToday'
        : 'dialogs.carryOver.autoSentToBacklog';
    args.announce(args.t(announceKey, { count: slippedRoots.length }));
    args.invalidateData();
    // The per-day `firedRef` already prevents re-firing within the
    // same calendar day — no need to set the carry-over snooze on
    // top. (Re-launching the app mid-day used to require the snooze
    // to avoid a second batch; the firedRef now lives inside the
    // checker for the full day instead, which is more accurate.)
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn('carry-over batch update_task failed', err);
  }
}
