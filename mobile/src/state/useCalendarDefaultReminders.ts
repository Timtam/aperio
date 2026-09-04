import { useCallback, useEffect, useRef, useState } from 'react';

import type { Reminder } from '@aperio/shared';

import { getUserPref, getUserPrefJson, setUserPref, setUserPrefJson } from '../api/prefs';
import { refreshRemindersSoon } from '../reminders/scheduler';

// Per-calendar default reminders — the mobile twin of the desktop
// useCalendarDefaultReminders, scoped to ONE calendar (the editor edits one).
// Mirrors iOS "Default Alert Times": the alert is applied at notification time,
// never written into the event body. The Host's reminder computation already
// reads `calendar.{id}.defaultReminders` (host-core reminders.rs), so what's set
// here genuinely fires for events in that calendar that carry no own reminder.
//
// Storage is the synced `calendar.{id}.defaultReminders` user-pref, value =
// Reminder[] JSON (empty ⇒ the key is dropped). Writes are debounced (the
// minute/date inputs emit an onChange per keystroke) and the latest value is
// flushed on unmount so a quick edit-then-close still lands; each persisted
// write nudges the reminder scheduler (refreshRemindersSoon — itself debounced)
// so a freshly-set default reaches the firing loop without waiting for a cache
// TTL. The mobile twin of the desktop's invalidateReminders nudge.

const WRITE_DEBOUNCE_MS = 150;

const prefKey = (calendarId: string): string => `calendar.${calendarId}.defaultReminders`;
const modeKey = (calendarId: string): string => `calendar.${calendarId}.defaultRemindersMode`;

/**
 * Where a calendar's default reminders live — the desktop's twin. `local` is
 * the overlay above; `attach` writes them into every NEW appointment created
 * in the calendar as its own reminders, so other clients of the same calendar
 * (the iOS Calendar app, a voice assistant reading iCloud) ring too. The Host
 * applies it on create (`use_calendar_defaults`); this hook only stores the
 * choice, under the synced `calendar.{id}.defaultRemindersMode`.
 */
export type DefaultReminderMode = 'local' | 'attach';
export const DEFAULT_REMINDER_MODES: readonly DefaultReminderMode[] = ['local', 'attach'];

export interface CalendarDefaultRemindersBinding {
  value: Reminder[];
  loading: boolean;
  save: (next: Reminder[]) => void;
  /** Where the defaults live; `local` until chosen otherwise. */
  mode: DefaultReminderMode;
  /** Choose where the defaults live and persist it (one tap, one write). */
  setMode: (next: DefaultReminderMode) => void;
}

export function useCalendarDefaultReminders(
  calendarId: string,
): CalendarDefaultRemindersBinding {
  const [value, setValue] = useState<Reminder[]>([]);
  const [mode, setModeState] = useState<DefaultReminderMode>('local');
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    // No calendar to read for (the event editor passes '' while creating, where
    // the overlay doesn't apply) — resolve empty without an FFI round-trip.
    if (!calendarId) {
      setValue([]);
      setModeState('local');
      setLoading(false);
      return;
    }
    setLoading(true);
    void Promise.all([
      getUserPrefJson<Reminder[]>(prefKey(calendarId)),
      getUserPref(modeKey(calendarId)),
    ])
      .then(([arr, rawMode]) => {
        if (cancelled) return;
        setValue(Array.isArray(arr) ? arr : []);
        // Only the exact marker attaches; anything else is the overlay.
        setModeState(rawMode === 'attach' ? 'attach' : 'local');
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
  }, [calendarId]);

  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pending = useRef<Reminder[] | null>(null);

  const persist = useCallback(
    (next: Reminder[]) => {
      const p =
        next.length === 0
          ? // An empty MARKER, not a deletion — the desktop writes the same.
            // Since birthday calendars fall back to a built-in default when
            // nothing was ever stored, deleting the key would read as "never
            // configured" and bring the reminder straight back: the off
            // switch would turn itself on again.
            setUserPref(prefKey(calendarId), '')
          : setUserPrefJson(prefKey(calendarId), next);
      void p.then(() => refreshRemindersSoon()).catch(() => {
        // Pref write failed; the in-memory value already reflects intent and the
        // next open re-reads from disk. Nothing to invalidate.
      });
    },
    [calendarId],
  );

  const save = useCallback(
    (next: Reminder[]) => {
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

  // `local` is written as a value rather than a deletion so the choice travels
  // as a synced answer, the way the desktop writes it.
  const setMode = useCallback(
    (next: DefaultReminderMode) => {
      setModeState(next);
      void setUserPref(modeKey(calendarId), next).catch(() => {
        // The on-screen choice already reflects intent; the next open re-reads
        // from disk, so a failed write shows up as the old value then.
      });
    },
    [calendarId],
  );

  return { value, loading, save, mode, setMode };
}
