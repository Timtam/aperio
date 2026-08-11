import { useEffect, useRef, useState } from 'react';

import type { Task } from '@aperio/shared';
import {
  joinSuggestionPasses,
  rankTitleSuggestions,
  UNFINISHED_TASK_STATUSES,
} from '@aperio/shared';

import type { CalendarEvent } from '../api/calendar';
import { search } from '../api/search';

/**
 * Earlier events (or tasks) whose TITLE matches what is being typed.
 *
 * The RN twin of the desktop hook, with the same three limits — only while
 * creating, not before two characters, and matched on the title alone — so
 * both platforms offer the same things for the same reasons.
 *
 * The history it draws on is the same on both platforms: the local tables AND
 * the external snapshot cache, so an appointment that has only ever lived in
 * iCloud, Google or Exchange is offered here as readily as a local one.
 *
 * What it cannot offer is something never cached — a calendar whose older
 * months this device has never actually looked at has no rows to match. That
 * is a property of the cache, not of the search: nothing prunes it, so it
 * deepens as the app is used.
 */
/** How long the text must sit still before the history is searched. */
const SEARCH_IDLE_MS = 500;

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
    // A pause turns a burst of round trips into one; nobody reads a list that
    // is being rebuilt under them anyway.
    //
    // Long enough to sit out a spoken phrase, not just a fast typist. iOS
    // dictation reports its recognition continuously, so a 200 ms window fired
    // a search — and a state update, and a render — several times into every
    // utterance, while the keyboard still owned the field. Waiting for a
    // genuine pause costs a suggestion list half a second nobody was reading
    // yet, and keeps the app out of the way while someone is still speaking.
    const timer = setTimeout(() => {
      // TWO passes for tasks, one for events — see `joinSuggestionPasses`.
      // The index answers with the best 200 matches, and for a title whose
      // history is mostly completion records (one per tick of a repeating
      // task, forever) the live task can be pushed out of its own result set
      // by its own past.
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
          // No offers this round. The title field is a title field first.
          if (round.current === mine) setMatches([]);
        });
    }, SEARCH_IDLE_MS);
    return () => clearTimeout(timer);
  }, [query, kind, enabled]);

  return matches;
}

/** Offers from earlier appointments, ordered by when the appointment WAS. */
export function rankEventSuggestions(events: readonly CalendarEvent[], query: string) {
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
