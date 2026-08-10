import { useEffect, useRef } from 'react';

import { setUserPref } from '../api/client';

/**
 * Persist one preference, debounced, and ONLY when it differs from what
 * storage already holds.
 *
 * ## Why the "only when it differs" is the point
 *
 * Every settings provider here has the same shape: read the keys once, hold
 * them in state, and write them back from an effect whenever the state
 * changes. That effect also runs the moment hydration finishes — and the value
 * it writes then is the value it has just READ. Harmless on its own; not
 * harmless at all in a synced app.
 *
 * `set_user_pref` appends a `SettingsUpdated` event for every whitelisted key
 * it is handed, without comparing values, and the applier resolves conflicts
 * by "later wins". So a launch stamps a fresh timestamp on an old choice: turn
 * a setting on from the phone, start the desktop before it has pulled that
 * event, and the desktop's startup echo is the newer statement. It wins, the
 * phone flips back on its next read, and nothing anywhere reports that a
 * choice was overwritten.
 *
 * A device that also fails to READ the key (the providers swallow read errors
 * and fall back to the default) would broadcast the DEFAULT over everyone
 * else's real value — the same accident, one step worse.
 *
 * So this hook keeps a baseline of what storage holds and stays silent unless
 * the value genuinely moved.
 *
 * ## The baseline
 *
 * Seeded on the first render after hydration, from the value the provider is
 * holding at that moment — which is by construction what hydration produced,
 * i.e. what storage holds (or, for an absent key, the default that an absent
 * key reads as; writing that default explicitly would be another echo).
 * Updated whenever we write, so the baseline is always this device's best
 * knowledge of the stored content.
 *
 * ## When the value arrives from ANOTHER device
 *
 * A provider that re-reads its keys after a sync round (see
 * `useUserPrefsChanged`) hands this hook a value it did not ask for. Writing
 * that back would restate a peer's change as our own, with our newer
 * timestamp — the same echo, one round later, and every device would repeat
 * every change it received.
 *
 * So the caller bumps `revision` whenever the value came from storage rather
 * than from the user. On a bump the baseline is re-seeded and nothing is
 * written; the next genuine edit compares against the new baseline.
 *
 * @param key         the `user_prefs` key
 * @param serialized  the value AS STORED — the caller does its own
 *                    serialisation, so the comparison is the same string
 *                    comparison the store would make
 * @param hydrating   true until the provider's initial read has been applied
 * @param debounceMs  quiet period before writing, so a flurry of clicks in a
 *                    settings panel is one write
 * @param revision    increment to say "this value came from storage"; the
 *                    default 0 never changes, so a caller that never re-reads
 *                    passes nothing
 */
export function useDebouncedPrefWrite(
  key: string,
  serialized: string,
  hydrating: boolean,
  debounceMs: number,
  revision = 0,
): void {
  /** What storage holds, as far as this device knows. `null` = not seeded. */
  const stored = useRef<string | null>(null);
  const timer = useRef<number | null>(null);
  /** The revision the baseline was last seeded at. */
  const seenRevision = useRef(revision);

  useEffect(() => {
    if (hydrating) return;
    if (seenRevision.current !== revision) {
      // A re-read landed: adopt it as the baseline, say nothing. A pending
      // write from just before it is dropped — the storage value is newer
      // than what the user was doing, and re-stating it would restart the
      // echo this hook exists to stop.
      seenRevision.current = revision;
      stored.current = serialized;
      if (timer.current !== null) {
        window.clearTimeout(timer.current);
        timer.current = null;
      }
      return;
    }
    if (stored.current === null) {
      // First look after hydration: this IS the stored value. Remember it and
      // say nothing.
      stored.current = serialized;
      return;
    }
    if (stored.current === serialized) return;
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      timer.current = null;
      // Record before the await: a second change arriving while this write is
      // in flight compares against what we are storing, not against the old
      // value, so it cannot queue a duplicate of the same write.
      stored.current = serialized;
      void setUserPref(key, serialized);
    }, debounceMs);
    return () => {
      if (timer.current !== null) {
        window.clearTimeout(timer.current);
        timer.current = null;
      }
    };
  }, [key, serialized, hydrating, debounceMs, revision]);
}
