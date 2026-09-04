import { useCallback, useEffect, useState } from 'react';

import type { Reminder } from '@aperio/shared';

import { listEventLocalReminders } from '../api/client';

/**
 * The reminders Aperio keeps for ONE event and tells no provider about
 * (migration 0043).
 *
 * A reminder normally rides on the event: Aperio writes it into the
 * appointment, the provider stores it, and every other client of that calendar
 * rings too. These do not — they fire here, travel to the user's other devices
 * through Aperio's own sync, and reach nobody else on a shared calendar. The
 * event dialog shows them beside the event's own reminders, each row saying
 * which kind it is.
 *
 * Reads the whole (small) set and picks this event's row, rather than asking
 * per event: the same shape `list_color_overrides` has, and the dialog opens
 * once per event anyway.
 */
export interface EventLocalRemindersBinding {
  /** This event's private reminders; empty when it has none. */
  reminders: Reminder[];
  /** Whether a row exists at all — an emptied list is a decision, and the
   *  save path must still write it so a peer's older list cannot win. */
  hadRow: boolean;
  /** The stored SIGNATURE, kept so a save that does not describe the keyed
   *  event (an occurrence carved out of a series) leaves it as it was. */
  title: string;
  startsAt: string;
  /** True until the first read returns. */
  loading: boolean;
  /** Re-read after a save, so a reopened dialog shows what was stored. */
  refresh: () => void;
}

/** The empty answer, as ONE array.
 *
 *  A fresh `[]` per read would be a new identity every time, and the event
 *  dialog derives its whole baseline from this list: a new identity there means
 *  a new baseline, which it re-applies, which renders again. Same content, same
 *  array. */
const NONE: Reminder[] = [];

/** The unknown signature, as ONE object — same reason as `NONE`. */
const NO_SIGNATURE = { title: '', startsAt: '' };

/** Whether two lists say the same thing, so the hook can keep the array it
 *  already handed out rather than mint a new one. The lists are a handful of
 *  small values, so comparing them beats making the callers defend themselves
 *  against identity churn. */
function sameReminders(a: readonly Reminder[], b: readonly Reminder[]): boolean {
  return a.length === b.length && JSON.stringify(a) === JSON.stringify(b);
}

export function useEventLocalReminders(
  calendarId: string | null,
  eventId: string | null,
): EventLocalRemindersBinding {
  const [reminders, setReminders] = useState<Reminder[]>(NONE);
  const [hadRow, setHadRow] = useState(false);
  const [signature, setSignature] = useState(NO_SIGNATURE);
  const [loading, setLoading] = useState(true);
  const [generation, setGeneration] = useState(0);

  const refresh = useCallback(() => setGeneration((n) => n + 1), []);

  useEffect(() => {
    let cancelled = false;
    if (!calendarId || !eventId) {
      setReminders(NONE);
      setHadRow(false);
      setSignature(NO_SIGNATURE);
      setLoading(false);
      return;
    }
    setLoading(true);
    void listEventLocalReminders()
      .then((rows) => {
        if (cancelled) return;
        const row = rows.find(
          (r) => r.calendar_id === calendarId && r.event_id === eventId,
        );
        const next = row?.reminders ?? NONE;
        setReminders((prev) => (sameReminders(prev, next) ? prev : next));
        setHadRow(row !== undefined);
        setSignature((prev) => {
          const title = row?.title ?? '';
          const startsAt = row?.starts_at ?? '';
          return prev.title === title && prev.startsAt === startsAt
            ? prev
            : { title, startsAt };
        });
      })
      .catch(() => {
        // Backend unreachable: an empty list reads as "none of its own", and
        // the next open tries again. Never guessed into the event's rows.
        if (!cancelled) {
          setReminders(NONE);
          setHadRow(false);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [calendarId, eventId, generation]);

  return {
    reminders,
    hadRow,
    title: signature.title,
    startsAt: signature.startsAt,
    loading,
    refresh,
  };
}
