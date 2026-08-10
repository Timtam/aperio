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
 * The history it draws on is the same on both platforms: the local tables AND
 * the external snapshot cache, so an appointment that has only ever lived in
 * iCloud, Google or Exchange is offered here as readily as a local one.
 *
 * What it cannot offer is something never cached — a calendar whose older
 * months this device has never actually looked at has no rows to match. That
 * is a property of the cache, not of the search: nothing prunes it, so it
 * deepens as the app is used.
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
