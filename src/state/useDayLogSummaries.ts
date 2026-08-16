import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import {
  compactDaySummary,
  dayLogsByDay,
  spokenDaySummary,
  type DayLog,
} from '@aperio/shared';

import { getDayLogsInRange } from '../api/client';
import { useDayMarkers } from './useDayMarkers';
import { useDayMarkersChanged } from './dayMarkersChanged';

/**
 * What a window of days was marked with, ready to hang on their headings.
 *
 * One RANGE read per window rather than one call per day: a month view asks
 * once for thirty-one days and gets back only the ones that say something.
 *
 * Returns two forms per day because they are for different readers and must
 * not be confused. `symbols` is decoration — callers render it hidden from the
 * accessibility tree. `spoken` is the truth, appended to the day heading's own
 * accessible NAME so the overview costs no extra focus stop.
 */
export function useDayLogSummaries(
  dayKeys: readonly string[],
  lead: string,
): {
  symbolsFor: (day: string) => string;
  spokenFor: (day: string) => string | null;
  refresh: () => Promise<void>;
} {
  const { markers } = useDayMarkers();
  const [logs, setLogs] = useState<Map<string, DayLog>>(new Map());

  // The window's bounds, so the effect re-runs when the view MOVES rather than
  // on every render that rebuilds the key array.
  const from = useMemo(
    () => (dayKeys.length ? dayKeys.reduce((a, b) => (b < a ? b : a)) : null),
    [dayKeys],
  );
  const to = useMemo(
    () => (dayKeys.length ? dayKeys.reduce((a, b) => (b > a ? b : a)) : null),
    [dayKeys],
  );

  // Which read is the current one. Two reads are separately spawned tasks
  // contending for the same DB lock, so they can finish in either order — and
  // because the effect only re-fires when the RANGE moves, a stale map landing
  // last would sit there until the view unmounts, with nothing to correct it.
  const readSeq = useRef(0);

  const refresh = useCallback(async () => {
    if (!from || !to) return;
    const mine = (readSeq.current += 1);
    try {
      const logs = await getDayLogsInRange(from, to);
      if (readSeq.current === mine) setLogs(dayLogsByDay(logs));
    } catch {
      // A day that cannot be read simply says nothing. This is an annotation
      // on a calendar, and a failed read must never take the calendar with it.
    }
  }, [from, to]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // A day ticked here or on another device changes what this window says. The
  // vocabulary half is already covered — `useDayMarkers` listens too — but the
  // LOGS are this hook's own copy and nothing else would refresh them.
  useDayMarkersChanged(() => {
    void refresh();
  });

  const symbolsFor = useCallback(
    (day: string) => compactDaySummary(logs.get(day), markers),
    [logs, markers],
  );
  const spokenFor = useCallback(
    (day: string) => spokenDaySummary(logs.get(day), markers, lead),
    [logs, markers, lead],
  );

  return { symbolsFor, spokenFor, refresh };
}
