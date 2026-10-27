import { useEffect, useState } from 'react';
import { alreadyHeld, importSpan, type HeldCheck, type ImportedEvent } from '@aperio/shared';

import { getEvents } from '../api/client';

const NOTHING_HELD: HeldCheck = { status: 'known', held: new Set() };
const UNKNOWABLE: HeldCheck = { status: 'unknowable' };

/**
 * Asks `calendarId` what it already holds of `events`, by their UIDs, and asks
 * again whenever the calendar changes. An answer carries the calendar it is
 * about, so one that arrives after a change never stands for the new one.
 * Appointments without a UID need no asking: nothing can say they are there.
 */
export function useHeldOnImport(
  calendarId: string,
  events: readonly ImportedEvent[],
  keepsUid: boolean | undefined,
): HeldCheck {
  const [answer, setAnswer] = useState<{ calendarId: string; check: HeldCheck } | null>(null);
  const span = importSpan(events);
  const nothingToAsk = !calendarId || !span || !events.some((event) => event.uid?.trim());
  const spanStart = span?.start;
  const spanEnd = span?.end;
  // A calendar that keeps no file's UID could find nothing: not asked. Not
  // known yet whether it does (its catalog still loads): asked once it is.
  const ask = !nothingToAsk && keepsUid === true;
  useEffect(() => {
    if (!ask || !spanStart || !spanEnd) return;
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
  }, [calendarId, events, ask, spanStart, spanEnd]);
  if (nothingToAsk) return NOTHING_HELD;
  if (keepsUid === false) return UNKNOWABLE;
  return answer?.calendarId === calendarId ? answer.check : { status: 'checking' };
}
