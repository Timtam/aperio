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
  /** Present with an RRULE when the row belongs to a recurring series. */
  recurrence?: { rrule?: string | null } | null;
}

/** Whether the row belongs to a recurring series. */
function recurs(event: LinkableEvent): boolean {
  return (event.recurrence?.rrule ?? '').trim() !== '';
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
 * A link is only acted on when it identifies exactly ONE meeting and exactly
 * ONE appointment. A standing meeting room reused by two unrelated
 * appointments makes the identity ambiguous — and an ambiguous identity is a
 * resemblance again. Then nothing happens: a wrong group is worse than none,
 * because it still looks authoritative.
 *
 * ## What counts as one appointment
 *
 * Two things, and both of them matter more than they look:
 *
 * **A series counts once.** A recurring appointment renders one row per day, so
 * a week's rows hold five of it. Counting rows would make every recurring
 * meeting permanently ambiguous — and it would depend on how wide a range
 * happened to be open, which is not a property of the data. Membership is keyed
 * by the series master, so that is what is counted.
 *
 * **A group counts once.** Copies of one appointment in several calendars each
 * carry the same join URL — that is what a forwarded invitation does. Counting
 * them separately would refuse exactly the case this feature exists for: an
 * appointment the user has ALREADY declared to be one thing. "Which appointment
 * is this meeting?" has one answer there, and the group is that answer. The
 * meeting joins the group rather than starting a second one.
 *
 * A claimant OUTSIDE the group still makes it ambiguous, and still stops
 * everything. The rule is not "ignore the extras", it is "count appointments,
 * not rows".
 *
 * `declines` is what stops this being a daily nuisance. Taking a member out of
 * a group, or dissolving one, writes exactly those marks (`EventGroupsRepo`),
 * and they sync — so a pair the user has pulled apart on any device stays
 * apart on all of them. Checked against EVERY copy of the appointment, because
 * a refusal names the pair the user actually saw.
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

  /** What the row is, as far as membership is concerned. */
  const memberKey = (event: E) =>
    eventGroupMemberKey(event.calendar_id, seriesId(event));
  /**
   * Which appointment the row belongs to: its group when it has one, else
   * itself. This is the identity the counting below is about.
   */
  const appointmentOf = (event: E) =>
    grouped.get(memberKey(event))?.id ?? `alone:${memberKey(event)}`;

  // Keyed by member key on both sides, so a series' own days collapse into the
  // one thing they are.
  const byUrl = new Map<string, { meetings: Map<string, E>; entries: Map<string, E> }>();
  for (const event of events) {
    const raw = meetingJoinUrl(event);
    if (!raw) continue;
    const url = normalizeJoinUrl(raw);
    if (url === '') continue;
    const bucket = byUrl.get(url) ?? { meetings: new Map(), entries: new Map() };
    const side = isMeetingCalendarEvent(event) ? bucket.meetings : bucket.entries;
    if (!side.has(memberKey(event))) side.set(memberKey(event), event);
    byUrl.set(url, bucket);
  }

  const out: MeetingLinkPair<E>[] = [];
  for (const [joinUrl, bucket] of byUrl) {
    if (bucket.meetings.size !== 1) continue;
    const entries = [...bucket.entries.values()];
    if (entries.length === 0) continue;
    // One appointment, however many rows say so.
    if (new Set(entries.map(appointmentOf)).size !== 1) continue;
    // A recurring appointment is left alone, and that is not caution.
    //
    // A group's members are SERIES. A provider that lists a recurring meeting
    // as one row per occurrence (Webex) has no series for one to name, and a
    // provider whose meeting does NOT recur while the appointment does has a
    // meeting that genuinely is not there on most of the days. Either way the
    // group would claim a copy that does not exist on the day being read, on
    // every day but one. The filter goes on hiding the duplicate, exactly as
    // before, and grouping by hand is still there for whoever wants it.
    if (entries.some(recurs)) continue;
    const meeting = [...bucket.meetings.values()][0];
    const event = entries[0];
    const a = { calendar_id: meeting.calendar_id, event_id: seriesId(meeting) };
    const meetingGroup = grouped.get(memberKey(meeting));
    const appointmentGroup = grouped.get(memberKey(event));
    // Already said: both sides in a group. The same one means there is nothing
    // to do; two different ones would be a merge, which only the user can ask
    // for — `group_events` refuses it for that reason.
    if (meetingGroup && appointmentGroup) continue;
    // ONE meeting per account per appointment, and this is load-bearing.
    //
    // A provider may list a recurring meeting as one row per occurrence, each
    // with an id of its own — Webex does, and its list response carries no
    // series id for us to collapse them by. Without this rule the day view
    // would hand over a different meeting id every morning, each one a new
    // member: the group would grow by one a day, forever, and the count on the
    // row would climb with it. An appointment has one meeting per account; once
    // this calendar is represented in the group, the job is done.
    if (
      appointmentGroup?.members.some(
        (m) => m.calendar_id === meeting.calendar_id,
      )
    ) {
      continue;
    }
    // Already refused, against any copy of the appointment.
    const refused = entries.some((entry) =>
      declined.has(
        suggestionPairKey(a, {
          calendar_id: entry.calendar_id,
          event_id: seriesId(entry),
        }),
      ),
    );
    if (refused) continue;
    out.push({ meeting, event, joinUrl });
  }
  return out;
}
