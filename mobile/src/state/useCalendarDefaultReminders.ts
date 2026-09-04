import { useCallback, useEffect, useRef, useState } from 'react';

import type { DefaultReminder } from '@aperio/shared';

import { getUserPrefJson, setUserPref, setUserPrefJson } from '../api/prefs';
import { scheduleBackgroundPush } from '../api/syncTriggers';
import { refreshRemindersSoon } from '../reminders/scheduler';
import { subscribeCacheReload } from './cacheObserver';

// Per-calendar default reminders — the mobile twin of the desktop
// useCalendarDefaultReminders, scoped to ONE calendar (the editor edits one).
// Each entry says where it lives (see the JSDoc below): an entry that stays in
// Aperio is applied at notification time and never written into an event, the
// way iOS treats its own "Default Alert Times"; an attach entry is written
// into new appointments as their own reminder. The Host's reminder computation
// reads `calendar.{id}.defaultReminders` (host-core reminders.rs), so what is
// set here genuinely fires.
//
// Storage is the synced `calendar.{id}.defaultReminders` user-pref, value =
// DefaultReminder[] JSON (empty ⇒ the key is dropped). Writes are debounced (the
// minute/date inputs emit an onChange per keystroke) and the latest value is
// flushed on unmount so a quick edit-then-close still lands; each persisted
// write nudges the reminder scheduler (refreshRemindersSoon — itself debounced)
// so a freshly-set default reaches the firing loop without waiting for a cache
// TTL. The mobile twin of the desktop's invalidateReminders nudge.

const WRITE_DEBOUNCE_MS = 150;

const prefKey = (calendarId: string): string => `calendar.${calendarId}.defaultReminders`;

/**
 * Each entry also says where it lives (`attach`, see `DefaultReminder`):
 * without the flag it stays in Aperio — the Host fires it for every event of
 * the calendar on top of the event's own reminders; with it the Host writes it
 * into every new appointment (`use_calendar_defaults`) as the appointment's
 * own reminder, so other clients of the calendar ring too. The flag travels
 * inside the same synced pref; this hook only stores the list.
 */
export interface CalendarDefaultRemindersBinding {
  value: DefaultReminder[];
  loading: boolean;
  save: (next: DefaultReminder[]) => void;
}

export function useCalendarDefaultReminders(
  calendarId: string,
): CalendarDefaultRemindersBinding {
  const [value, setValue] = useState<DefaultReminder[]>([]);
  const [loading, setLoading] = useState(true);
  // Bumped when a sync round applied a peer's data, so an open editor re-reads
  // instead of holding the list it hydrated with. The pref rides the calendar
  // category (it is a per-calendar setting), like the signature list next door.
  const [syncedVersion, setSyncedVersion] = useState(0);
  useEffect(
    () => subscribeCacheReload('calendar', () => setSyncedVersion((n) => n + 1)),
    [],
  );

  useEffect(() => {
    let cancelled = false;
    // Bumped by every local edit — see `writeGeneration` below.
    const generation = writeGeneration.current;
    // No calendar to read for (the event editor passes '' while creating, where
    // the overlay doesn't apply) — resolve empty without an FFI round-trip.
    if (!calendarId) {
      setValue([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    void getUserPrefJson<DefaultReminder[]>(prefKey(calendarId))
      .then((arr) => {
        // An edit happened while this read was in flight — it already holds
        // the newer list, and its own write is on its way to the Host.
        if (cancelled || generation !== writeGeneration.current) return;
        setValue(Array.isArray(arr) ? arr : []);
      })
      .catch(() => {
        if (!cancelled) setValue([]);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [calendarId, syncedVersion]);

  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pending = useRef<DefaultReminder[] | null>(null);
  // Bumped by every local edit. A re-read that started BEFORE one must not
  // land after it: the writes are debounced, so a sync round arriving mid-edit
  // would otherwise put the pre-edit list back over the rows on screen.
  const writeGeneration = useRef(0);

  const persist = useCallback(
    (next: DefaultReminder[]) => {
      const p =
        next.length === 0
          ? // An empty MARKER, not a deletion — the desktop writes the same.
            // Since birthday calendars fall back to a built-in default when
            // nothing was ever stored, deleting the key would read as "never
            // configured" and bring the reminder straight back: the off
            // switch would turn itself on again.
            setUserPref(prefKey(calendarId), '')
          : setUserPrefJson(prefKey(calendarId), next);
      void p
        .then(() => {
          refreshRemindersSoon();
          // A synced setting: push it now rather than at the next periodic
          // round, the way every other mobile mutation does. Without this the
          // change sat here until the next timer or app-exit flush.
          scheduleBackgroundPush();
        })
        .catch(() => {
        // Pref write failed; the in-memory value already reflects intent and the
        // next open re-reads from disk. Nothing to invalidate.
      });
    },
    [calendarId],
  );

  const save = useCallback(
    (next: DefaultReminder[]) => {
      writeGeneration.current += 1;
      setValue(next);
      pending.current = next;
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => {
        timer.current = null;
        const v = pending.current;
        pending.current = null;
        if (v != null) persist(v);
      }, WRITE_DEBOUNCE_MS);
    },
    [persist],
  );

  // Flush a pending write on unmount so a sub-debounce edit-then-close lands.
  useEffect(
    () => () => {
      if (timer.current) {
        clearTimeout(timer.current);
        timer.current = null;
        if (pending.current != null) {
          persist(pending.current);
          pending.current = null;
        }
      }
    },
    [persist],
  );

  return { value, loading, save };
}
