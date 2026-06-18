import type { TaskList, TaskStatus } from '@aperio/shared';

import { getUserPref, setUserPref } from '../api/prefs';

// The global task-behaviour knobs (Settings → Tasks), SYNCED across the user's
// devices via the `tasks.*` user-prefs (all §19.2.1 always-sync keys) — the
// mobile twin of the desktop TaskCascadeProvider's GLOBAL settings. Drives the
// check-off gesture in TasksScreen + WeekScreen. (Per-list overrides
// [`tasks.listOverrides`], carry-over, and the day-start trigger are deferred —
// the latter two feed the day-start review checkers, which mobile doesn't host
// yet.) Best-effort throughout: a read failure falls back to the desktop
// defaults; a write failure still applies for this session.

export type CheckoffMode = 'toggle' | 'cycle';

export interface TaskBehaviour {
  /** Couple parent/subtask status — when off, the planner does a single-row
   *  write (no up/down cascade). Default on. */
  cascadeEnabled: boolean;
  /** Pin a dateless task to today when it enters in_progress. Default on. */
  autoDate: boolean;
  /** What a check-off does: flip open↔completed, or cycle through in_progress. */
  checkoffMode: CheckoffMode;
}

const CASCADE_KEY = 'tasks.cascadeStatusCoupling';
const AUTO_DATE_KEY = 'tasks.autoDateOnStart';
const CHECKOFF_KEY = 'tasks.checkoffMode';

/** Boolean prefs default ON; only the literal `"false"` turns them off (matches
 *  the desktop TaskCascadeProvider hydration). */
function parseBool(stored: string | null): boolean {
  return stored !== 'false';
}

function parseCheckoff(stored: string | null): CheckoffMode {
  return stored === 'cycle' ? 'cycle' : 'toggle';
}

/** Read the three synced knobs, or the desktop defaults when unset/unreadable. */
export async function readTaskBehaviour(): Promise<TaskBehaviour> {
  try {
    const [cascade, autoDate, checkoff] = await Promise.all([
      getUserPref(CASCADE_KEY),
      getUserPref(AUTO_DATE_KEY),
      getUserPref(CHECKOFF_KEY),
    ]);
    return {
      cascadeEnabled: parseBool(cascade),
      autoDate: parseBool(autoDate),
      checkoffMode: parseCheckoff(checkoff),
    };
  } catch {
    return { cascadeEnabled: true, autoDate: true, checkoffMode: 'toggle' };
  }
}

async function writeBest(key: string, value: string): Promise<void> {
  try {
    await setUserPref(key, value);
  } catch {
    // Ignore — the chosen value still applies via local state this session.
  }
}

export const writeCascadeEnabled = (v: boolean): Promise<void> =>
  writeBest(CASCADE_KEY, v ? 'true' : 'false');
export const writeAutoDate = (v: boolean): Promise<void> =>
  writeBest(AUTO_DATE_KEY, v ? 'true' : 'false');
export const writeCheckoffMode = (m: CheckoffMode): Promise<void> =>
  writeBest(CHECKOFF_KEY, m);

/** Whether the owning provider stores `in_progress` as a distinct state
 *  (`task_capabilities.supports_in_progress`, absent → true). When false, the
 *  cycle drops the in_progress step (it would revert to open on read-back) and
 *  auto-date is skipped (nothing would persist). */
export function canStoreInProgress(list: TaskList | undefined): boolean {
  return list?.task_capabilities?.supports_in_progress ?? true;
}

/**
 * Next status for a check-off, honouring the mode. Ported verbatim from the
 * desktop useTaskStatusToggle:
 *   - toggle (default): completed → open; anything else → completed.
 *   - cycle: open → in_progress → completed → open; a cancelled task re-enters
 *     at open. Drops the in_progress step where the provider can't store it.
 */
export function nextCheckoffStatus(
  current: TaskStatus,
  mode: CheckoffMode,
  canInProgress: boolean,
): TaskStatus {
  if (mode === 'cycle') {
    switch (current) {
      case 'open':
        return canInProgress ? 'in_progress' : 'completed';
      case 'in_progress':
        return 'completed';
      case 'completed':
        return 'open';
      default:
        return 'open';
    }
  }
  return current === 'completed' ? 'open' : 'completed';
}
