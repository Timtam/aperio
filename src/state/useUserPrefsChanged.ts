import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';

/**
 * Run `onChanged` when a sync round wrote one of `keys` into `user_prefs`.
 *
 * The settings providers read their keys once, at startup, and hold them in
 * memory. Without this, a setting changed on another device sat in SQLite
 * while this app went on showing the old value until the next launch — the
 * one thing "synced setting" is supposed not to mean.
 *
 * The backend emits `user-prefs-changed` with the keys a round actually
 * applied (`SyncRoundReport::settings_keys`), and only when there were any, so
 * a quiet round costs nothing here. A listener filters for its own keys — the
 * providers own disjoint sets, and re-reading fourteen preferences because a
 * calendar's default reminders changed would be noise.
 *
 * `keys` may be a fresh array on every render; it is read through a ref, so
 * the listener is subscribed once and callers need no `useMemo`. Same for the
 * handler.
 */
export function useUserPrefsChanged(
  keys: readonly string[],
  onChanged: (changed: string[]) => void,
): void {
  const keysRef = useRef(keys);
  keysRef.current = keys;
  const handlerRef = useRef(onChanged);
  handlerRef.current = onChanged;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen<string[]>('user-prefs-changed', (event) => {
      const changed = Array.isArray(event.payload) ? event.payload : [];
      const mine = changed.filter((key) => keysRef.current.includes(key));
      if (mine.length > 0) handlerRef.current(mine);
    })
      .then((fn) => {
        // The subscription can resolve after this effect was cleaned up
        // (StrictMode's double-mount, or a fast unmount) — drop it right away
        // instead of leaking a listener that outlives its component.
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // No Tauri host (tests, a browser preview). Nothing to listen to, and
        // a missing re-read is not worth an error the user cannot act on.
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
}
