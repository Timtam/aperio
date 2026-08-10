import { useEffect, useRef, useState } from 'react';

import type { Task } from '@aperio/shared';
import { rankTitleSuggestions } from '@aperio/shared';

import type { CalendarEvent } from '../api/calendar';
import { search } from '../api/search';

/**
 * Earlier events (or tasks) whose TITLE matches what is being typed.
 *
 * The RN twin of the desktop hook, with the same three limits — only while
 * creating, not before two characters, and matched on the title alone — so
 * both platforms offer the same things for the same reasons.
 *
 * One difference is honest rather than chosen: mobile's search covers the
 * LOCAL tables only. The external snapshot cache the desktop also searches
 * needs the SWR layer mobile does not have yet, so an appointment that only
 * ever lived in iCloud will not be offered here. It is a known parity gap of
 * the search itself, not of this feature.
 */
export function useTitleSuggestions<K extends 'events' | 'tasks'>(
  query: string,
  kind: K,
  enabled: boolean,
): (K extends 'events' ? CalendarEvent : Task)[] {
  type Item = K extends 'events' ? CalendarEvent : Task;
  const [matches, setMatches] = useState<Item[]>([]);
  const round = useRef(0);

  useEffect(() => {
    const trimmed = query.trim();
    if (!enabled || trimmed.length < 2) {
      setMatches([]);
      return;
    }
    const mine = (round.current += 1);
    // A short pause turns a burst of round trips into one; nobody reads a list
    // that is being rebuilt under them anyway.
    const timer = setTimeout(() => {
      void search(trimmed, { kind })
        .then((results) => {
          if (round.current !== mine) return;
          setMatches((kind === 'events' ? results.events : results.tasks) as Item[]);
        })
        .catch(() => {
          // No offers this round. The title field is a title field first.
          if (round.current === mine) setMatches([]);
        });
    }, 200);
    return () => clearTimeout(timer);
  }, [query, kind, enabled]);

  return matches;
}

/** Offers from earlier appointments, ordered by when the appointment WAS. */
export function rankEventSuggestions(events: readonly CalendarEvent[], query: string) {
  return rankTitleSuggestions(events, query, (e) => e.start);
}

/** The task twin — ordered by when the task was last touched. */
export function rankTaskSuggestions(tasks: readonly Task[], query: string) {
  return rankTitleSuggestions(tasks, query, (t) => t.updated_at ?? t.created_at);
}
