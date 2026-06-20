import { useCallback, useEffect, useRef, useState } from 'react';

import type { Reminder } from '@aperio/shared';

import { deleteUserPref, getUserPrefJson, setUserPrefJson } from '../api/prefs';
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

export interface CalendarDefaultRemindersBinding {
  value: Reminder[];
  loading: boolean;
  save: (next: Reminder[]) => void;
}

export function useCalendarDefaultReminders(
  calendarId: string,
): CalendarDefaultRemindersBinding {
  const [value, setValue] = useState<Reminder[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void getUserPrefJson<Reminder[]>(prefKey(calendarId))
      .then((arr) => {
        if (!cancelled) setValue(Array.isArray(arr) ? arr : []);
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
          ? deleteUserPref(prefKey(calendarId))
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

  return { value, loading, save };
}
