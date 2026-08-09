// Recognising a copy (DESIGN-event-groups.md, Stufe 3).
//
// The design lists three ways membership could come about and picks one:
// detected and SUGGESTED, confirmed once, then remembered. This is the
// detection, and it is deliberately the strictest of the three readings.
//
// Automatic grouping was rejected for a concrete reason: an office full of
// "Team meeting" at 10:00 would have two different meetings declared one
// appointment, and a wrong group is worse than a missed one — it hides a real
// commitment behind a copy of something else. So the rule here is precision
// over recall, and what it produces is never applied, only offered.

/** The minimum a row needs to be considered. */
export interface SuggestableEvent {
  id: string;
  calendar_id: string;
  title: string;
  start: string;
  all_day?: boolean;
}

/** Case, padding and inner spacing are not part of what a title says. */
function normalizeTitle(title: string): string {
  return title.trim().toLowerCase().replace(/\s+/g, ' ');
}

/** All-day events agree on a DAY; timed ones on an instant. */
function whenKey(event: SuggestableEvent): string {
  return event.all_day ? event.start.slice(0, 10) : new Date(event.start).toISOString();
}

/**
 * The event that most looks like a copy of `anchor`, or `null`.
 *
 * Three conditions, all required:
 *
 * - the SAME title, ignoring case and padding — a copy is made by copying,
 *   so a near-match is far more often two different things than one;
 * - the SAME start (the same day for all-day events) — "overlapping" would
 *   catch the meeting before this one, which is exactly the wrong answer;
 * - a DIFFERENT calendar — two rows in one calendar are a duplicate to clean
 *   up, not an appointment that lives in several places.
 *
 * Ties go to the first candidate in the order the caller supplied, which is
 * the order the user sees. Nothing here writes anything: a suggestion is an
 * offer, and the user's confirmation is what makes a group.
 */
export function suggestGroupMate<E extends SuggestableEvent>(
  anchor: SuggestableEvent,
  candidates: readonly E[],
): E | null {
  const title = normalizeTitle(anchor.title);
  if (title === '') return null;
  const when = whenKey(anchor);
  return (
    candidates.find(
      (candidate) =>
        candidate.calendar_id !== anchor.calendar_id &&
        normalizeTitle(candidate.title) === title &&
        whenKey(candidate) === when,
    ) ?? null
  );
}
