import { useCallback, useEffect, useState } from 'react';

import { sortDayMarkers, type DayMarker } from '@aperio/shared';

import { listDayMarkers } from '../api/client';

/**
 * The day-marker vocabulary, loaded once per consumer.
 *
 * A hook of its own rather than another field on the calendar store: the
 * vocabulary is a handful of rows that only two surfaces read (the settings
 * panel that edits it, and the dialog that ticks a day with it), and neither
 * is on the launch path. Threading it through the store would put it in every
 * render that store touches for no gain.
 *
 * `loading` starts true so a consumer never renders "no markers yet" during
 * the first read — that message is an invitation to create one, and showing it
 * to somebody who has ten would be a lie.
 */
export function useDayMarkers(): {
  markers: DayMarker[];
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  /** Apply a locally-known list without a round trip — what a reorder uses so
   *  the list does not visibly jump while the writes land. */
  replace: (next: DayMarker[]) => void;
} {
  const [markers, setMarkers] = useState<DayMarker[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setMarkers(sortDayMarkers(await listDayMarkers()));
      setError(null);
    } catch (err) {
      // Keep whatever is already on screen: an empty list here would read as
      // "you have no markers", which is a different statement from "the read
      // failed".
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const replace = useCallback((next: DayMarker[]) => {
    setMarkers(sortDayMarkers(next));
  }, []);

  return { markers, loading, error, refresh, replace };
}
