import { useEffect, useRef, useState } from 'react';

import {
  joinSuggestionPasses,
  rankTitleSuggestions,
  UNFINISHED_TASK_STATUSES,
} from '@aperio/shared';

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
      // TWO passes for tasks, one for events — see `joinSuggestionPasses`.
      // The index answers with the best 200 matches, and for a title whose
      // history is mostly completion records (one per tick of a repeating
      // task, forever) the live task can be pushed out of its own result set
      // by its own past. Asking for the unfinished rows separately is what
      // guarantees the row the user actually meant is there to be offered.
      const lookup =
        kind === 'events'
          ? search(trimmed, { kind }).then((r) => r.events as Item[])
          : Promise.all([
              search(trimmed, {
                kind,
                task_statuses: [...UNFINISHED_TASK_STATUSES],
              }),
              search(trimmed, { kind }),
            ]).then(
              ([live, history]) =>
                joinSuggestionPasses(live.tasks, history.tasks) as Item[],
            );
      void lookup
        .then((items) => {
          if (round.current !== mine) return;
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
  return rankTitleSuggestions(
    events,
    query,
    (e) => e.start,
    undefined,
    // A cancelled appointment is a poor template for the same reason.
    (e) => (e.cancelled ? 1 : 0),
  );
}

/** The task twin — ordered by when the task was last touched. */
export function rankTaskSuggestions(tasks: readonly Task[], query: string) {
  return rankTitleSuggestions(
    tasks,
    query,
    (t) => t.updated_at ?? t.created_at,
    undefined,
    // A finished task is a worse template than a living one — and for a
    // REPEATING task it is the wrong one outright: the completion record left
    // behind on every tick carries no repetition and no reminders by design,
    // and being the newest row of its name it used to win every time.
    (t) => (t.status === 'completed' || t.status === 'cancelled' ? 1 : 0),
  );
}
