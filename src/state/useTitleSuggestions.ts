import { useEffect, useRef, useState } from 'react';

import { rankTitleSuggestions } from '@aperio/shared';

import { search } from '../api/client';
import type { CalendarEvent, Task } from '../api/types';

/**
 * Earlier events (or tasks) whose TITLE matches what is being typed.
 *
 * The search index is the app's own full-text one, so this costs no new
 * backend: it already spans the local tables and every provider's cached rows,
 * which is exactly the history worth offering — "what have I called this
 * before" is a question about all of it, not about one calendar.
 *
 * The one thing it cannot offer is something never cached: a calendar whose
 * older months this device has never looked at has no rows to match. That is a
 * property of the cache rather than of the search — nothing prunes it, so it
 * deepens as the app is used.
 *
 * Three deliberate limits:
 *   - only while CREATING. Opening an existing item and touching its title
 *     must not offer to overwrite it with an older version of itself;
 *   - not before two characters. One letter matches most of a calendar, and a
 *     list that says everything says nothing;
 *   - matched on the title only. The index also covers description, location
 *     and attendees — right for searching, wrong here, where an offer whose
 *     title has nothing to do with the typed words looks like invention.
 */
export function useTitleSuggestions<K extends 'events' | 'tasks'>(
  query: string,
  kind: K,
  enabled: boolean,
): (K extends 'events' ? CalendarEvent : Task)[] {
  type Item = K extends 'events' ? CalendarEvent : Task;
  const [matches, setMatches] = useState<Item[]>([]);
  // The query the last accepted round was for, so a stale answer arriving late
  // cannot replace a newer one.
  const round = useRef(0);

  useEffect(() => {
    const trimmed = query.trim();
    if (!enabled || trimmed.length < 2) {
      setMatches([]);
      return;
    }
    const mine = (round.current += 1);
    // Typing is not a search each keystroke: a short pause is what turns a
    // burst of round trips into one, and nobody reads a list that is being
    // rebuilt under them anyway.
    const timer = setTimeout(() => {
      void search(trimmed, { kind })
        .then((results) => {
          if (round.current !== mine) return;
          const items = (kind === 'events' ? results.events : results.tasks) as Item[];
          setMatches(items);
        })
        .catch(() => {
          // No offers this round. The title field is a title field first, and
          // a failed lookup beside it is not worth a word.
          if (round.current === mine) setMatches([]);
        });
    }, 200);
    return () => clearTimeout(timer);
  }, [query, kind, enabled]);

  return matches;
}

/** The offers themselves: title matches, one per distinct title, newest first. */
export function rankEventSuggestions(events: readonly CalendarEvent[], query: string) {
  // Ordered by when the appointment WAS, not when its row was written: the
  // most recent time you had this appointment is the one that reflects how it
  // looks now.
  return rankTitleSuggestions(events, query, (e) => e.start);
}

/** The task twin — ordered by when the task was last touched. */
export function rankTaskSuggestions(tasks: readonly Task[], query: string) {
  return rankTitleSuggestions(tasks, query, (t) => t.updated_at ?? t.created_at);
}
