// Grouping a provider's meeting with the appointment it belongs to
// (DESIGN-event-groups.md, Stufe 4).
//
// A videoconference account contributes a read-only calendar of its own
// meetings. Most of those meetings also have a calendar entry — Aperio's own,
// or the invitation Outlook wrote — so they appeared twice, and
// `withoutDuplicateMeetings` dropped the meeting row to hide it. That works
// most of the time, and when it does not, a meeting the user really has simply
// vanishes with nothing to say so.
//
// A group does the same job honestly: both rows stay, folding shows one, the
// mark says "2×", and a divergence becomes visible instead of being discarded.
// That last one is the case the filter can never handle: when an appointment
// is moved and its meeting is not, the join URL still matches, so the filter
// goes on hiding the meeting exactly when the two have stopped agreeing.
//
// ## Why this may group by itself when nothing else may
//
// Stage 3 refuses to group automatically, and the reason stands: an office full
// of "Team meeting" at 10:00 would produce groups nobody asked for. That reason
// is about GUESSING — about treating a resemblance as evidence.
//
// A meeting and its calendar entry are not related by resemblance. They carry
// the same JOIN URL, issued by the provider, and that is an identity. The
// existing filter already uses exactly it, for exactly the reasons written down
// beside it: Aperio writes the event's own title into the meeting it creates,
// so title equality says almost nothing, and the times drift apart precisely
// when an event is moved — which is when the two most need to be seen as one.
//
// Grouping on an identity is a different proposition from grouping on a
// likeness. Everything here rests on that distinction, so the rules below are
// strict about staying on the identity's side of it.

import type { EventGroup } from './eventGroups';
import type { SuggestionDecline } from './types';

/** The least an event needs for this. */
export interface LinkableEvent {
  calendar_id: string;
  location?: string | null;
  description?: string | null;
  /** Present with an RRULE when the row belongs to a recurring series. */
  recurrence?: { rrule?: string | null } | null;
}

/** A meeting row and the appointment it belongs to. */
export interface MeetingLinkPair<E> {
  /** The row from the meetings calendar. */
  meeting: E;
  /** The ordinary calendar entry for the same meeting. */
  event: E;
  /** The join URL both carry — the identity this rests on. */
  joinUrl: string;
}

/**
 * This surface's door into `cal_core::meeting_link_grouping`.
 *
 * The whole window crosses at once and the answer is POSITIONS, because the
 * caller is holding the rows already.
 */
export interface MeetingLinkRules {
  /** `{events[], groups[], declines[]}` in, `[{meeting, event, join_url}]`
   *  out, with positions in `events`. */
  findMeetingLinkPairsJson(inputJson: string): string;
  /** One join URL in, the folded identity out. */
  normalizeJoinUrl(url: string): string;
}

let installedRules: MeetingLinkRules | null = null;

/** Bind this surface's door into the core. */
export function installMeetingLinkRules(rules: MeetingLinkRules): void {
  installedRules = rules;
}

function rules(): MeetingLinkRules {
  if (installedRules === null) {
    // Loud, not a local fallback. A fallback here would be the second
    // implementation all over again, and its failure is quiet: a meeting and
    // its appointment silently stop being offered as one.
    throw new Error(
      'meeting-link grouping used before installMeetingLinkRules() — the ' +
        'surface must bind its door into cal_core at startup',
    );
  }
  return installedRules;
}

/**
 * Fold a join URL to what two spellings of the same link agree on.
 *
 * Case in the scheme and host, surrounding space, trailing slashes on the
 * path, a default port, dot segments, and the punycode form of an
 * internationalised host — but NOT the query string, where a meeting's id and
 * password live.
 */
export function normalizeJoinUrl(url: string): string {
  return rules().normalizeJoinUrl(url);
}

/**
 * The (meeting, appointment) pairs that should become groups.
 *
 * One window's rows, the groups behind them, and the refusals the user has
 * already made. Only a view has all of a window's rows in hand, which is why
 * this is called from here and not from an adapter.
 *
 * The rule is `cal_core::meeting_link_grouping::find_meeting_link_pairs`, and
 * everything it decides — counting appointments rather than rows, refusing an
 * ambiguous link, leaving a recurring appointment alone, what a refusal covers
 * — is written down there. This is the door.
 */
export function findMeetingLinkPairs<E extends LinkableEvent>(
  events: readonly E[],
  groups: readonly EventGroup[],
  declines: readonly SuggestionDecline[],
  seriesId: (event: E) => string,
): MeetingLinkPair<E>[] {
  const answer = rules().findMeetingLinkPairsJson(
    JSON.stringify({
      events: events.map((ev) => ({
        calendar_id: ev.calendar_id,
        series_id: seriesId(ev),
        location: ev.location ?? null,
        description: ev.description ?? null,
        recurs: (ev.recurrence?.rrule ?? '').trim() !== '',
      })),
      groups,
      declines,
    }),
  );
  const pairs = JSON.parse(answer) as {
    meeting: number;
    event: number;
    join_url: string;
  }[];
  return pairs.map(({ meeting, event, join_url }) => ({
    meeting: events[meeting],
    event: events[event],
    joinUrl: join_url,
  }));
}
