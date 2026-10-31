import { useEffect, useState } from 'react';

import { resolveCalendarUserEmail } from './currentUserEmail';

/**
 * The account's address in `calendarId`, which tells an imported meeting the
 * account organizes from one someone else organizes (DESIGN-file-import.md,
 * I6): `undefined` while it is asked, `null` when the calendar has none to
 * give or could not be asked. Asked only when `needed`. An answer carries the
 * calendar it is about, so one that arrives after a change of calendar never
 * stands for the new one. Mirrors the desktop hook.
 */
export function useCalendarOwnAddress(calendarId: string, needed: boolean): string | null | undefined {
  const [answer, setAnswer] = useState<{ calendarId: string; address: string | null } | null>(
    null,
  );
  useEffect(() => {
    if (!needed || !calendarId) return;
    let live = true;
    resolveCalendarUserEmail(calendarId)
      .then((address) => {
        if (live) setAnswer({ calendarId, address });
      })
      .catch(() => {
        if (live) setAnswer({ calendarId, address: null });
      });
    return () => {
      live = false;
    };
  }, [calendarId, needed]);
  if (!needed || !calendarId) return null;
  return answer?.calendarId === calendarId ? answer.address : undefined;
}
