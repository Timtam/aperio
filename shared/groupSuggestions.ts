// Recognising a copy (DESIGN-event-groups.md, Stufe 3) — this surface's door
// into `cal_core::group_suggestion`.
//
// The design lists three ways membership could come about and picks one:
// detected and SUGGESTED, confirmed once, then remembered. Two questions live
// behind this door, and they are deliberately not the same one:
//
//   - `suggestGroupMate` answers "which of these is a copy of THAT one" for a
//     user who has already opened the grouping dialog.
//   - `findGroupSuggestions` answers the question nobody asked — are there
//     copies in this day at all? — and is therefore far more careful, because
//     it speaks unprompted. Three rules keep it from becoming noise: the same
//     strict match, never about events already grouped, and never about a pair
//     the user has DECLINED.
//
// Automatic grouping was rejected for a concrete reason: an office full of
// "Team meeting" at 10:00 would have two different meetings declared one
// appointment, and a wrong group is worse than a missed one — it hides a real
// commitment behind a copy of something else. So the rule is precision over
// recall, and what it produces is never applied, only offered.
//
// # This used to be the rule, and now it carries it
//
// Both functions were TypeScript implementations of a decision the core also
// had to make. They are one implementation now, and this file is what a
// surface installs to reach it — the arrangement `collation.ts` and
// `taskStatus.ts` already have, and for the same reason: both callers ask
// during a RENDER (`useMemo` in the suggestion notice and in the grouping
// dialog), so the door has to answer synchronously.
//
// The signatures are unchanged, deliberately. The door builds the request and
// maps the answer back to the caller's own rows, so no caller had to learn
// anything about JSON or about positions.

import type { EventGroup } from './eventGroups';
import type { SuggestionDecline } from './types';

/** The minimum a row needs to take part. */
export interface SuggestibleEvent {
  id: string;
  calendar_id: string;
  title: string;
  start: string;
  all_day?: boolean;
}

/** Two events that look like one appointment. */
export interface GroupSuggestion<E> {
  first: E;
  second: E;
}

/**
 * This surface's door into `cal_core::group_suggestion`.
 *
 * JSON in, JSON out, both answers being POSITIONS in the input — the caller is
 * holding the rows already, and echoing them back across the boundary would
 * double the payload to say nothing new.
 */
export interface GroupSuggestionRules {
  /** `{events[], groups[], declines[]}` in, `[{first, second}]` out. */
  findGroupSuggestionsJson(inputJson: string): string;
  /** `{anchor, candidates[]}` in, a position or the string `"null"` out. */
  suggestGroupMateJson(inputJson: string): string;
}

let installedRules: GroupSuggestionRules | null = null;

/** Bind this surface's door into the core. */
export function installGroupSuggestionRules(rules: GroupSuggestionRules): void {
  installedRules = rules;
}

function rules(): GroupSuggestionRules {
  if (installedRules === null) {
    // Loud, not a local fallback. A fallback here would be the second
    // implementation all over again, and the failure it produces is quiet:
    // one surface offering a group the other never mentions, which nobody
    // reports because each device looks self-consistent.
    throw new Error(
      'group suggestions used before installGroupSuggestionRules() — the ' +
        'surface must bind its door into cal_core::group_suggestion at startup',
    );
  }
  return installedRules;
}

/** The wire shape of one row, built here so no caller has to know it. */
function wireEvent(
  event: SuggestibleEvent,
  seriesId: string,
): {
  calendar_id: string;
  series_id: string;
  title: string;
  start: string;
  all_day: boolean;
} {
  return {
    calendar_id: event.calendar_id,
    series_id: seriesId,
    title: event.title,
    start: event.start,
    all_day: event.all_day ?? false,
  };
}

/**
 * The event that most looks like a copy of `anchor`, or `null`.
 *
 * Three conditions, all required: the same title ignoring case, padding and
 * the width of the gaps between words; the same start (the same day for
 * all-day rows); and a DIFFERENT calendar. Ties go to the first candidate in
 * the order the caller supplied, which is the order the user sees.
 */
export function suggestGroupMate<E extends SuggestibleEvent>(
  anchor: SuggestibleEvent,
  candidates: readonly E[],
): E | null {
  const answer = rules().suggestGroupMateJson(
    JSON.stringify({
      anchor: wireEvent(anchor, anchor.id),
      candidates: candidates.map((c) => wireEvent(c, c.id)),
    }),
  );
  const at = JSON.parse(answer) as number | null;
  return at == null ? null : (candidates[at] ?? null);
}

/**
 * Copies worth offering, among the rows of ONE day.
 *
 * One day's rows, like the folding rule and for the same reason: a recurring
 * appointment renders a row per day, and across a range its own days would
 * pair up with each other.
 *
 * At most one pair per event, and the list is capped by the caller — a day
 * that somehow produces six suggestions is a day where something is wrong with
 * the matching, and six offers is not the way to find that out.
 */
export function findGroupSuggestions<E extends SuggestibleEvent>(
  events: readonly E[],
  groups: readonly EventGroup[],
  declines: readonly SuggestionDecline[],
  seriesId: (event: E) => string,
): GroupSuggestion<E>[] {
  const answer = rules().findGroupSuggestionsJson(
    JSON.stringify({
      events: events.map((ev) => wireEvent(ev, seriesId(ev))),
      groups,
      declines,
    }),
  );
  const pairs = JSON.parse(answer) as { first: number; second: number }[];
  return pairs.map(({ first, second }) => ({
    first: events[first],
    second: events[second],
  }));
}
