import { useEffect, useMemo, useState } from 'react';

import { eventGroupMemberKey, type EventGroup } from '@aperio/shared';

import { eventGroupsForEvents } from '../api/client';
import type { CalendarEvent } from '../api/types';
import { seriesIdOf } from '../intl/recurrence';
import { useDialogState } from './dialogStateContext';

/**
 * The groups behind the events a view is about to render
 * (`DESIGN-event-groups.md`).
 *
 * One query per rendered range, not one per row: a day with twenty events
 * should cost one call. The answer comes back as WHOLE groups, including the
 * members that fall outside the range — a group only reads as a whole ("this
 * and three others"), and the count has to stay honest even when a copy sits
 * in a calendar the user has switched off.
 *
 * Keyed on the series masters in view, so paging back to a day whose events
 * are already known does not re-ask. Re-runs on `dataVersion`, like everything
 * else that has to notice a mutation.
 */
export function useEventGroups(events: readonly CalendarEvent[]): EventGroup[] {
  const { dataVersion } = useDialogState();
  const [groups, setGroups] = useState<EventGroup[]>([]);

  // The distinct (calendar, series) pairs as one stable string — the same
  // trick `useEvents` uses for its calendar set, and for the same reason: the
  // array identity changes on every render, its contents rarely do.
  //
  // The shared member key is JSON, so joining the entries with a newline
  // cannot collide with anything a provider might put in an id.
  const refsKey = useMemo(() => {
    const seen = new Set<string>();
    for (const ev of events) {
      seen.add(eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev)));
    }
    return [...seen].sort().join('\n');
  }, [events]);

  useEffect(() => {
    if (refsKey === '') {
      setGroups([]);
      return;
    }
    let cancelled = false;
    const refs = refsKey.split('\n').map((entry) => {
      const [calendar_id, event_id] = JSON.parse(entry) as [string, string];
      return { calendar_id, event_id };
    });
    eventGroupsForEvents(refs)
      .then((found) => {
        if (!cancelled) setGroups(found);
      })
      .catch(() => {
        // A failed lookup means no folding this round — which is exactly what
        // the app did before groups existed. Never an empty day.
        if (!cancelled) setGroups([]);
      });
    return () => {
      cancelled = true;
    };
  }, [refsKey, dataVersion]);

  return groups;
}
