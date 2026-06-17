import { useEffect, useState } from 'react';
import { AppState } from 'react-native';

import { todayIsoKey } from '@aperio/shared';

/**
 * Local `YYYY-MM-DD` that flips when the calendar day actually changes.
 *
 * Mirrors the desktop's `useCurrentDayKey` (60s poll), plus an `AppState`
 * `'active'` recompute: a backgrounded RN app has its timers throttled or
 * suspended, so the common rollover case is the user foregrounding the app
 * after midnight. Keeps the Upcoming/Deferred gate (DESIGN §9.12) and
 * `describeDue`'s "Resurfaces" text fresh across midnight without a reload.
 */
export function useCurrentDayKey(): string {
  const [key, setKey] = useState(todayIsoKey);

  useEffect(() => {
    const sync = () => {
      const next = todayIsoKey();
      setKey((prev) => (prev === next ? prev : next));
    };
    const interval = setInterval(sync, 60_000);
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') sync();
    });
    return () => {
      clearInterval(interval);
      sub.remove();
    };
  }, []);

  return key;
}
