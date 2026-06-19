import { useCallback, useEffect, useState } from 'react';

import type { SoundConfig } from '@aperio/shared';

import { deleteUserPref, getUserPrefJson, setUserPrefJson } from '../api/prefs';
import { refreshRemindersSoon } from '../reminders/scheduler';

// Bind a `sound.*` user-pref key (sound.global / sound.calendar.{id} /
// sound.tasklist.{id}) to a SoundConfig, the §14.4 sound hierarchy's storage.
// `null` = the key is unset → inherit to the next level (or System for the
// global root). The value is the cal_core SoundConfig JSON the host-core
// resolver reads. Saving writes/deletes the (synced) pref and kicks a reminder
// reschedule so already-scheduled OS notifications re-resolve their sound — the
// mobile twin of the desktop `invalidateReminders()`.

export interface SoundPrefBinding {
  value: SoundConfig | null;
  loading: boolean;
  save: (next: SoundConfig | null) => Promise<void>;
}

/** `prefKey` may be `null` (e.g. a per-item key whose item isn't loaded yet, or
 *  create mode): the binding then holds `null`, isn't loading, and `save` is a
 *  no-op — so the hook can be called unconditionally (Rules of Hooks) while the
 *  picker that consumes it stays hidden until a real key exists. */
export function useSoundPref(prefKey: string | null): SoundPrefBinding {
  const [value, setValue] = useState<SoundConfig | null>(null);
  const [loading, setLoading] = useState(prefKey != null);

  useEffect(() => {
    if (prefKey == null) {
      setValue(null);
      setLoading(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    void getUserPrefJson<SoundConfig>(prefKey)
      .then((cfg) => {
        if (!cancelled) setValue(cfg);
      })
      .catch(() => {
        if (!cancelled) setValue(null);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [prefKey]);

  const save = useCallback(
    async (next: SoundConfig | null) => {
      if (prefKey == null) return;
      if (next == null) {
        await deleteUserPref(prefKey);
      } else {
        await setUserPrefJson(prefKey, next);
      }
      setValue(next);
      // Re-resolve already-scheduled reminder sounds against the new pref.
      refreshRemindersSoon();
    },
    [prefKey],
  );

  return { value, loading, save };
}
