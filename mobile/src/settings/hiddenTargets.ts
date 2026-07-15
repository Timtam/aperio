import { useEffect, useState } from 'react';

import { getUserPref, setUserPref } from '../api/prefs';

// "Offer hidden calendars / task lists as assignment targets" — SYNCED across the
// user's devices via `pickers.showHiddenCalendarTargets` /
// `pickers.showHiddenTaskListTargets` (the mobile twin of the desktop ViewState
// prefs). When ON (the default) a container hidden in the sidebar can still be
// picked as the target when creating / editing / moving an item; when OFF only
// currently-shown containers are offered. The pickers (editors, quick-add,
// move/copy) read these and pass `includeHidden` to the shared
// `selectableEventCalendars` / `selectableTaskLists`. Settings toggles live on
// GeneralSettingsScreen (calendars) and TaskSettingsScreen (task lists).

const CALENDAR_PREF = 'pickers.showHiddenCalendarTargets';
const TASK_LIST_PREF = 'pickers.showHiddenTaskListTargets';

/** Read a synced boolean pref; defaults to true when unset or on any value other
 *  than the literal "false". */
async function readBool(key: string): Promise<boolean> {
  try {
    return (await getUserPref(key)) !== 'false';
  } catch {
    return true;
  }
}

/** Persist a synced boolean pref. Best-effort — a write failure leaves the
 *  caller's local toggle reflecting the choice for this session. */
async function writeBool(key: string, value: boolean): Promise<void> {
  try {
    await setUserPref(key, value ? 'true' : 'false');
  } catch {
    // Ignore — applies on the next picker open.
  }
}

export const readShowHiddenCalendarTargets = (): Promise<boolean> =>
  readBool(CALENDAR_PREF);
export const writeShowHiddenCalendarTargets = (value: boolean): Promise<void> =>
  writeBool(CALENDAR_PREF, value);
export const readShowHiddenTaskListTargets = (): Promise<boolean> =>
  readBool(TASK_LIST_PREF);
export const writeShowHiddenTaskListTargets = (value: boolean): Promise<void> =>
  writeBool(TASK_LIST_PREF, value);

/** Reactively read a synced boolean pref (defaults to `true` until it loads),
 *  for a picker that just needs the current value on open. */
function useSyncedBool(read: () => Promise<boolean>): boolean {
  const [value, setValue] = useState(true);
  useEffect(() => {
    let mounted = true;
    void read().then((v) => {
      if (mounted) setValue(v);
    });
    return () => {
      mounted = false;
    };
  }, [read]);
  return value;
}

/** True when hidden calendars should be offered as event targets (default on). */
export const useShowHiddenCalendarTargets = (): boolean =>
  useSyncedBool(readShowHiddenCalendarTargets);

/** True when hidden task lists should be offered as task targets (default on). */
export const useShowHiddenTaskListTargets = (): boolean =>
  useSyncedBool(readShowHiddenTaskListTargets);
