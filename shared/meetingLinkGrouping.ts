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
import { eventGroupMemberKey, indexEventGroups } from './eventGroups';
import { suggestionPairKey } from './groupSuggestions';
import { isMeetingCalendarEvent, meetingJoinUrl } from './meetingEvents';
import type { SuggestionDecline } from './types';

/** The least an event needs for this. */
export interface LinkableEvent {
  calendar_id: string;
  location?: string | null;
  description?: string | null;
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
 * Fold a join URL to what two spellings of the same link agree on.
 *
 * Only what is safe: case in the scheme and host, surrounding space, one
 * trailing slash. NOT the query string — a meeting link's id and password live
 * there, and two links that differ in the query are two different meetings.
 */
export function normalizeJoinUrl(url: string): string {
  const trimmed = url.trim();
  if (trimmed === '') return '';
  try {
    const parsed = new URL(trimmed);
    const path = parsed.pathname.replace(/\/+$/, '');
    return `${parsed.protocol.toLowerCase()}//${parsed.host.toLowerCase()}${path}${parsed.search}`;
  } catch {
    // Not a URL we can parse — compare it as it stands rather than inventing a
    // normalisation. Two identical strings still match.
    return trimmed;
  }
}

/**
 * The (meeting, appointment) pairs that should become groups.
 *
 * The same shape as its siblings `findGroupSuggestions` and
 * `findStaleSignatures`: one window's rows, the groups behind them, and the
 * refusals the user has already made. Only a view has all of a window's rows
 * in hand, which is why this lives here and not in an adapter.
 *
 * A link is only acted on when it identifies exactly ONE meeting row and
 * exactly ONE appointment. A standing meeting room reused across a series, or
 * two appointments pointing at the same link, make the identity ambiguous —
 * and an ambiguous identity is a resemblance again. Then nothing happens: a
 * wrong group is worse than none, because it still looks authoritative.
 *
 * `declines` is what stops this being a daily nuisance. Taking a member out of
 * a group, or dissolving one, writes exactly those marks (`EventGroupsRepo`),
 * and they sync — so a pair the user has pulled apart on any device stays
 * apart on all of them.
 */
export function findMeetingLinkPairs<E extends LinkableEvent>(
  events: readonly E[],
  groups: readonly EventGroup[],
  declines: readonly SuggestionDecline[],
  seriesId: (event: E) => string,
): MeetingLinkPair<E>[] {
  const grouped = indexEventGroups(groups);
  const declined = new Set(
    declines.map((d) =>
      suggestionPairKey(
        { calendar_id: d.calendar_a, event_id: d.event_a },
        { calendar_id: d.calendar_b, event_id: d.event_b },
      ),
    ),
  );

  const byUrl = new Map<string, { meetings: E[]; entries: E[] }>();
  for (const event of events) {
    const raw = meetingJoinUrl(event);
    if (!raw) continue;
    const url = normalizeJoinUrl(raw);
    if (url === '') continue;
    const bucket = byUrl.get(url) ?? { meetings: [], entries: [] };
    (isMeetingCalendarEvent(event) ? bucket.meetings : bucket.entries).push(event);
    byUrl.set(url, bucket);
  }

  const out: MeetingLinkPair<E>[] = [];
  const spokenFor = new Set<string>();
  for (const [joinUrl, bucket] of byUrl) {
    if (bucket.meetings.length !== 1 || bucket.entries.length !== 1) continue;
    const meeting = bucket.meetings[0];
    const event = bucket.entries[0];
    const a = { calendar_id: meeting.calendar_id, event_id: seriesId(meeting) };
    const b = { calendar_id: event.calendar_id, event_id: seriesId(event) };
    const keyA = eventGroupMemberKey(a.calendar_id, a.event_id);
    const keyB = eventGroupMemberKey(b.calendar_id, b.event_id);
    // A recurring appointment renders one row per day, so the same pair can
    // arrive several times over a range. Group it once.
    if (spokenFor.has(keyA) || spokenFor.has(keyB)) continue;
    const groupA = grouped.get(keyA);
    const groupB = grouped.get(keyB);
    // Already said, or already refused.
    if (groupA && groupB) continue;
    if (declined.has(suggestionPairKey(a, b))) continue;
    spokenFor.add(keyA);
    spokenFor.add(keyB);
    out.push({ meeting, event, joinUrl });
  }
  return out;
}
