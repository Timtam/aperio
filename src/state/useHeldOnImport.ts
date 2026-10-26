import { useEffect, useState } from 'react';
import { alreadyHeld, importSpan, type ImportedEvent } from '@aperio/shared';

import { getEvents } from '../api/client';

/** What a calendar already holds of a file's appointments (I4): still being
 *  asked, could not be asked, or which ones, by their place in the file. */
export type HeldCheck =
  | { status: 'checking' }
  | { status: 'failed' }
  | { status: 'known'; held: ReadonlySet<number> };

const NOTHING_HELD: HeldCheck = { status: 'known', held: new Set() };

/**
 * Asks `calendarId` what it already holds of `events`, by their UIDs, and asks
 * again whenever the calendar changes. An answer carries the calendar it is
 * about, so one that arrives after a change never stands for the new one.
 * Appointments without a UID need no asking: nothing can say they are there.
 */
export function useHeldOnImport(calendarId: string, events: readonly ImportedEvent[]): HeldCheck {
  const [answer, setAnswer] = useState<{ calendarId: string; check: HeldCheck } | null>(null);
  const span = importSpan(events);
  const nothingToAsk = !calendarId || !span || !events.some((event) => event.uid?.trim());
  const spanStart = span?.start;
  const spanEnd = span?.end;
  useEffect(() => {
    if (nothingToAsk || !spanStart || !spanEnd) return;
    let live = true;
    getEvents({ calendar_id: calendarId, start: spanStart, end: spanEnd })
      .then((rows) => {
        if (live) {
          setAnswer({ calendarId, check: { status: 'known', held: alreadyHeld(events, rows) } });
        }
      })
      .catch(() => {
        if (live) setAnswer({ calendarId, check: { status: 'failed' } });
      });
    return () => {
      live = false;
    };
  }, [calendarId, events, nothingToAsk, spanStart, spanEnd]);
  if (nothingToAsk) return NOTHING_HELD;
  return answer?.calendarId === calendarId ? answer.check : { status: 'checking' };
}
