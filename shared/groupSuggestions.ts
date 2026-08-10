// Offering a group the user has not asked for yet
// (DESIGN-event-groups.md, Stufe 3: „erkannt und VORGESCHLAGEN").
//
// `suggestGroupMate` answers "which of these is a copy of THAT one" for a user
// who already opened the grouping dialog. This answers the question nobody
// asked: are there copies in this day at all?
//
// The difference matters, because this one speaks unprompted. Three rules keep
// it from becoming noise:
//
//   - the same strict match as everywhere else (same name, same start, another
//     calendar) — a near miss offered every morning is worse than no offer;
//   - never about events that are already grouped, which is the answer to the
//     question already given;
//   - never about a pair the user has DECLINED. That record is what turns a
//     suggestion into a question asked once instead of a daily interruption,
//     and it is why migration 0037 exists.

import type { EventGroup } from './eventGroups';
import { eventGroupMemberKey, indexEventGroups } from './eventGroups';
import { isMeetingCalendarEvent } from './meetingEvents';
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

function normalizeTitle(title: string): string {
  return title.trim().toLowerCase().replace(/\s+/g, ' ');
}

function whenKey(event: SuggestibleEvent): string {
  return event.all_day ? event.start.slice(0, 10) : new Date(event.start).toISOString();
}

/**
 * Whether a stored refusal is currently in force.
 *
 * The later statement wins, and a tie goes to the refusal — the same rule the
 * host applies in `SuggestionDecline::is_declined` and in `read_declines`'
 * WHERE clause, stated once on this side of the FFI. The host already filters
 * before handing rows over, so for today's callers this is a defence: a future
 * caller feeding raw snapshot rows must not resurrect a refusal the user took
 * back.
 */
export function isDeclineInForce(d: SuggestionDecline): boolean {
  return d.cleared_at == null || d.declined_at >= d.cleared_at;
}

/** The pair, in the canonical order the decline record uses, as one string. */
export function suggestionPairKey(
  a: { calendar_id: string; event_id: string },
  b: { calendar_id: string; event_id: string },
): string {
  const first = eventGroupMemberKey(a.calendar_id, a.event_id);
  const second = eventGroupMemberKey(b.calendar_id, b.event_id);
  return first <= second ? `${first}\n${second}` : `${second}\n${first}`;
}

/**
 * Pairs in this day that look like one appointment and have never been
 * answered about.
 *
 * ONE DAY's rows, like the folding rule and for the same reason: a recurring
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
  const grouped = indexEventGroups(groups);
  const declined = new Set(
    declines.filter(isDeclineInForce).map((d) =>
      suggestionPairKey(
        { calendar_id: d.calendar_a, event_id: d.event_a },
        { calendar_id: d.calendar_b, event_id: d.event_b },
      ),
    ),
  );

  const out: GroupSuggestion<E>[] = [];
  const spokenFor = new Set<string>();
  // A meeting row is never OFFERED on a resemblance.
  //
  // This whole function guesses from a name and a time, which is why its answer
  // is an offer rather than a group. A videoconference meeting is the one row
  // that does not need guessing: it carries the join URL its provider issued,
  // and `findMeetingLinkPairs` pairs it on that identity. Offering it here put
  // the two mechanisms in each other's way — Aperio writes the event's own
  // title into the meeting it creates, so "same name, same time" is nearly
  // guaranteed for the wrong reasons, and the office full of "Team meeting" at
  // 10:00 that this module's own comment warns about is exactly where a meeting
  // would be offered against a stranger.
  //
  // Answering that offer wrote a refusal NAMING the meeting, and a refusal is
  // forever. The user was asked the wrong question and their answer was kept.
  const offerable = (event: E) => !isMeetingCalendarEvent(event);
  for (let i = 0; i < events.length; i += 1) {
    const first = events[i];
    const firstId = seriesId(first);
    const firstKey = eventGroupMemberKey(first.calendar_id, firstId);
    if (spokenFor.has(firstKey) || grouped.has(firstKey)) continue;
    if (!offerable(first)) continue;
    const title = normalizeTitle(first.title);
    if (title === '') continue;
    const when = whenKey(first);

    for (let j = i + 1; j < events.length; j += 1) {
      const second = events[j];
      const secondId = seriesId(second);
      const secondKey = eventGroupMemberKey(second.calendar_id, secondId);
      if (spokenFor.has(secondKey) || grouped.has(secondKey)) continue;
      if (!offerable(second)) continue;
      if (second.calendar_id === first.calendar_id) continue;
      if (normalizeTitle(second.title) !== title) continue;
      if (whenKey(second) !== when) continue;
      if (
        declined.has(
          suggestionPairKey(
            { calendar_id: first.calendar_id, event_id: firstId },
            { calendar_id: second.calendar_id, event_id: secondId },
          ),
        )
      ) {
        continue;
      }
      out.push({ first, second });
      spokenFor.add(firstKey);
      spokenFor.add(secondKey);
      break;
    }
  }
  return out;
}
