import { useCallback, useEffect, useState } from 'react';

import {
  getUserPref,
  invalidateReminders,
  setUserPref,
} from '../api/client';
import type { Reminder } from '../api/types';

/**
 * Per-calendar default reminders.
 *
 * Mirrors iOS's "Default Alert Times" (Settings → Calendar →
 * Standardhinweise) at calendar granularity. iOS never writes those
 * defaults into the VEVENT body — the alert is applied locally at
 * notification time. CalDAV adapters therefore have nothing to parse,
 * and an iCloud event created without an explicit per-event alarm
 * comes back from `get_events` with `reminders: []` even when the
 * user "set a default" on the iPhone. The EventDialog then renders
 * an empty reminders list and the user sees a discrepancy: the
 * iPhone shows a reminder, Aperio doesn't.
 *
 * This hook keeps the gap addressable per-calendar (some calendars
 * want a default, some don't — e.g. "Privat" yes, "Geburtstage" no).
 * Storage is `user_prefs` keyed by `calendar.{calendarId}.defaultReminders`,
 * value = JSON-stringified `Reminder[]`. We load every calendar's
 * value on mount; subsequent reads are a pure Map lookup.
 *
 * Writes are debounced 150 ms so an inline editor that emits one
 * `onChange` per keystroke doesn't hammer the wire — same idiom as
 * `useTaskListShowCompleted`.
 */

const KEY_PREFIX = 'calendar.';
const KEY_SUFFIX = '.defaultReminders';
const WRITE_DEBOUNCE_MS = 150;

export type DefaultRemindersMap = Record<string, Reminder[]>;

export interface CalendarDefaultReminders {
  /** Default reminders for `calendarId`, or empty array when none. */
  getDefaultsFor: (calendarId: string) => Reminder[];
  /** Set the defaults for one calendar and persist asynchronously. */
  setDefaultsFor: (calendarId: string, reminders: Reminder[]) => void;
  /** True until the initial hydration round-trip returns. */
  hydrating: boolean;
}

const EMPTY: Reminder[] = [];

function prefKey(calendarId: string): string {
  return `${KEY_PREFIX}${calendarId}${KEY_SUFFIX}`;
}

export function useCalendarDefaultReminders(
  calendarIds: readonly string[],
): CalendarDefaultReminders {
  const [map, setMap] = useState<DefaultRemindersMap>({});
  const [hydrating, setHydrating] = useState(true);

  // Hydrate every visible calendar's default once we know the id
  // list. The Settings panel passes the full `calendars` list from
  // `useCalendarStore`, so on first render this fans out one
  // `getUserPref` per calendar — typically a handful, and the backend
  // resolves them all from the same SQLite table.
  //
  // The id list is stable across renders only by reference; we
  // collapse to a sorted-joined string so a re-render with the same
  // ids in a different order doesn't trigger a redundant fetch.
  const idsKey = [...calendarIds].sort().join('|');
  useEffect(() => {
    let cancelled = false;
    setHydrating(true);
    void (async () => {
      const next: DefaultRemindersMap = {};
      await Promise.all(
        calendarIds.map(async (id) => {
          try {
            const raw = await getUserPref(prefKey(id));
            if (!raw) return;
            const parsed = JSON.parse(raw) as unknown;
            if (Array.isArray(parsed)) {
              // Trust the shape — the writer is us and the type is
              // pinned by Reminder. A future schema change can re-
              // version the pref key (`...defaultReminders.v2`) so the
              // old data degrades to "no default" rather than
              // exploding on a Reminder shape change.
              next[id] = parsed as Reminder[];
            }
          } catch {
            // Backend unreachable or bad JSON: leave id absent so
            // `getDefaultsFor` returns the empty fallback.
          }
        }),
      );
      if (!cancelled) {
        setMap(next);
        setHydrating(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idsKey]);

  // Debounced persistence — RemindersEditor emits an onChange per
  // keystroke for the minute input; without the debounce we'd write
  // SQLite once per character.
  const pendingWrites = useState<Map<string, number>>(
    () => new Map(),
  )[0];
  useEffect(() => {
    return () => {
      pendingWrites.forEach((timer) => window.clearTimeout(timer));
      pendingWrites.clear();
    };
  }, [pendingWrites]);

  const getDefaultsFor = useCallback(
    (calendarId: string): Reminder[] => map[calendarId] ?? EMPTY,
    [map],
  );

  const setDefaultsFor = useCallback(
    (calendarId: string, reminders: Reminder[]) => {
      setMap((prev) => {
        // Empty list → drop the key so the pref store stays minimal
        // and `getDefaultsFor` returns the same `EMPTY` reference.
        if (reminders.length === 0) {
          if (prev[calendarId] === undefined) return prev;
          const next = { ...prev };
          delete next[calendarId];
          return next;
        }
        return { ...prev, [calendarId]: reminders };
      });

      // Restart the debounce window for this calendar's write.
      const existing = pendingWrites.get(calendarId);
      if (existing !== undefined) {
        window.clearTimeout(existing);
      }
      const timer = window.setTimeout(() => {
        pendingWrites.delete(calendarId);
        const writePromise =
          reminders.length === 0
            ? setUserPref(prefKey(calendarId), '')
            : setUserPref(prefKey(calendarId), JSON.stringify(reminders));
        // Once the pref lands in SQLite, nudge the reminder scheduler
        // so it drops its external-trigger cache and re-scans on the
        // next tick. Without this nudge, a default reminder added
        // while Aperio is running wouldn't reach the firing loop
        // until the scheduler's TTL cache expires (~5 min) — meaning
        // "a meeting in 30 min with a freshly-set 15 min default"
        // would silently not fire even though the catch-up logic
        // would have caught it.
        void writePromise
          .then(() => invalidateReminders())
          .catch((err) => {
            // Pref write itself failed; nothing to invalidate. The
            // in-memory map already reflects the user's intent, the
            // next mount will overwrite from disk anyway. Log so a
            // pattern of failures surfaces in dev.
            // eslint-disable-next-line no-console
            console.warn('default-reminder pref write failed', err);
          });
      }, WRITE_DEBOUNCE_MS);
      pendingWrites.set(calendarId, timer);
    },
    [pendingWrites],
  );

  return { getDefaultsFor, setDefaultsFor, hydrating };
}
