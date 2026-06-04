import { useCallback, useEffect, useState } from 'react';

import {
  deleteUserPref,
  getUserPref,
  invalidateReminders,
  setUserPref,
} from '../api/client';
import type { SoundConfig } from '../api/types';

/**
 * Load + persist a single notification-sound override (DESIGN.md
 * §14.4) stored in `user_prefs` under one of the `sound.*` keys
 * (`sound.global`, `sound.calendar.{id}`, `sound.tasklist.{id}`,
 * `sound.item.{id}`).
 *
 * `null` value means "no override at this level" — the row is deleted
 * and resolution falls through to the next level up. After every write
 * we ping `invalidateReminders` so the scheduler re-resolves promptly
 * (the resolved sound is baked into cached triggers at build time, so
 * without the nudge a change wouldn't take effect until the next scan).
 *
 * Pass `key: null` to disable the hook (e.g. an item picker on a
 * not-yet-saved item) — it stays at `null` and writes are no-ops.
 */
export function useSoundPref(key: string | null) {
  const [value, setValue] = useState<SoundConfig | null>(null);
  const [hydrating, setHydrating] = useState(true);

  useEffect(() => {
    let cancelled = false;
    if (!key) {
      setValue(null);
      setHydrating(false);
      return;
    }
    setHydrating(true);
    getUserPref(key)
      .then((raw) => {
        if (cancelled) return;
        setValue(raw ? safeParse(raw) : null);
        setHydrating(false);
      })
      .catch(() => {
        if (cancelled) return;
        setValue(null);
        setHydrating(false);
      });
    return () => {
      cancelled = true;
    };
  }, [key]);

  const update = useCallback(
    (next: SoundConfig | null) => {
      setValue(next);
      if (!key) return;
      const done = () => {
        void invalidateReminders();
      };
      if (next === null) {
        void deleteUserPref(key).then(done).catch(done);
      } else {
        void setUserPref(key, JSON.stringify(next)).then(done).catch(done);
      }
    },
    [key],
  );

  return { value, setValue: update, hydrating };
}

/** Tolerant parse: a corrupt pref value resolves to "no override"
 *  rather than throwing into the render. */
function safeParse(raw: string): SoundConfig | null {
  try {
    const parsed = JSON.parse(raw) as SoundConfig;
    return parsed && typeof parsed === 'object' && 'source' in parsed
      ? parsed
      : null;
  } catch {
    return null;
  }
}
