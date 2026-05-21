import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/Announcer';
import type { Task } from '../api/types';
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
  snoozeCarryOver,
} from './CarryOverDialog';

/**
 * Mount-once gate that handles the day-start carry-over flow. The
 * Settings → Tasks "Übernahme-Standard" preference decides what
 * happens:
 *
 *   - `ask`  (default) — pushes the carry-over dialog so the user
 *     can decide per row.
 *   - `today` — silently sets `scheduled_date = today` on every
 *     slipped task (and, when cascade-coupling is on, its actionable
 *     descendants). Announces the result via the live region; no
 *     dialog flashes.
 *   - `backlog` — same as above but clears `scheduled_date`.
 *
 * Snooze (the four-hour suppress flag from the dialog's "Später
 * erinnern" button) silences every mode equally: a snoozed user
 * doesn't want either a dialog OR a silent batch action mid-window.
 * After running the silent action we set the snooze ourselves so a
 * re-launch within the window doesn't pick the same task family up
 * a second time.
 */
export function CarryOverChecker() {
  const { tasks, loading } = useTasks();
  const { openCarryOver, invalidateData } = useDialogState();
  const announce = useAnnouncer();
  const { t } = useTranslation();
  const {
    enabled: cascadeEnabled,
    carryOverDefault,
    hydrating,
  } = useTaskCascadeEnabled();
  const firedRef = useRef(false);

  useEffect(() => {
    if (firedRef.current) return;
    // Wait for both the task catalog and the preferences round-trip
    // — otherwise a default-`ask` from the pre-hydration state would
    // open the dialog even for users who opted into auto-today /
    // auto-backlog.
    if (loading || hydrating) return;
    if (isCarryOverSnoozed()) {
      firedRef.current = true;
      return;
    }
    const slipped = filterCarriedOver(tasks, { cascadeEnabled });
    if (slipped.length === 0) {
      firedRef.current = true;
      return;
    }
    firedRef.current = true;

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
    // Set the snooze ourselves so a re-launch within the four-hour
    // window doesn't fire the batch again on the same rows.
    snoozeCarryOver(4);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn('carry-over batch update_task failed', err);
  }
}
