// Folding a group into one row (DESIGN-event-groups.md, Stufe 1).
//
// A group says several events mean the same appointment. Until now it said so
// only in a dialog; here it changes what a day looks like. Four rows that are
// one commitment become one row that names the calendars it spans — the
// largest everyday gain of the feature, and for a screen-reader user not a
// cosmetic one: it is three fewer things to walk past every time.
//
// Shared, because a day that reads differently on the phone than on the
// desktop would be worse than not folding at all.

import type { EventGroup } from './eventGroups';
import { eventGroupMemberKey, indexEventGroups } from './eventGroups';
import { isMeetingCalendarEvent } from './meetingEvents';

/** The minimum a row has to carry to be foldable. */
export interface CollapsibleEvent {
  id: string;
  calendar_id: string;
  start?: string | null;
  all_day?: boolean;
}

/** One rendered row after folding. */
export interface CollapsedRow<E> {
  /** The row to draw. For a group, the member that stood first. */
  event: E;
  /** The group this row stands for, when it stands for one. */
  group?: EventGroup;
  /**
   * How many OTHER events the group holds — from the group itself, not from
   * what happened to be on screen. A copy in a calendar the user has switched
   * off is still a copy, and saying "with 2 others" is what makes the count
   * match their own memory of how many they keep.
   */
  otherMembers: number;
  /** The calendars the group spans, this row's own first. */
  calendarIds: string[];
  /**
   * The members in view disagree about when the appointment is.
   *
   * Then the group is a claim that has stopped being true — one copy was moved
   * and the others were not. Folding it silently would hide exactly the
   * problem the user needs to see, so the row says so and the divergent
   * members are NOT folded away.
   */
  diverged: boolean;
}

/** The moment, as an INSTANT rather than as the string it arrived in.
 *
 *  The same instant reaches this module spelled two ways: `expandAll` rewrites
 *  every recurring occurrence through `toISOString()` ("…T08:00:00.000Z")
 *  while a one-off passes through with the backend's own serialisation
 *  ("…T08:00:00Z"), and some providers add sub-second precision. Comparing the
 *  raw strings made a series grouped with a single event permanently
 *  "diverged": it never folded, and BOTH copies announced "which is now at a
 *  different time" — every day, for two events at the identical instant. The
 *  sibling modules (`suggestGroupMate`, `groupSuggestions`, `healEventGroups`)
 *  all normalise for this reason; this one did not. */
function startKey(event: CollapsibleEvent): string {
  if (event.all_day) return `day:${(event.start ?? '').slice(0, 10)}`;
  const at = new Date(event.start ?? '').getTime();
  return `at:${Number.isFinite(at) ? at : (event.start ?? '')}`;
}

/**
 * Fold each group's members into a single row, keeping the input order.
 *
 * The representative is the member that comes FIRST in the list handed in —
 * i.e. in whatever order the view had already decided on. That keeps the day's
 * sorting intact: folding removes rows, it never moves one.
 *
 * ## Call this with ONE DAY's rows
 *
 * Not a whole week, and the reason is the divergence check. A recurring
 * appointment renders one row per day, so a group of two series over a
 * five-day range hands in ten rows with five different start times — read
 * across the range that looks exactly like "the copies have drifted apart",
 * and nothing would ever fold. Within one day the question is the right one
 * again: two copies of the same appointment, today, at different times.
 *
 * Every view already buckets by day before it renders, so this costs nothing.
 *
 * `seriesId` maps a rendered row back to the id membership is keyed by (the
 * series master). Every caller has one already; it is a parameter so this
 * module does not have to know how ids encode occurrences.
 */
export function collapseEventGroups<E extends CollapsibleEvent>(
  events: readonly E[],
  groups: readonly EventGroup[],
  seriesId: (event: E) => string,
  /**
   * Whether this row is one the user can ACT on — the tie-breaker for which
   * member a folded group shows.
   *
   * Position alone decided it before, and position is not a property of the
   * data: the members of a folded group are at the identical instant (a
   * difference marks the group diverged and nothing folds), so the sort is a
   * tie and the order falls through to whatever the calendar fan-out happened
   * to produce — arrival order, or a HashMap's iteration order. The row could
   * therefore be the read-only videoconference copy: no editor, no delete, no
   * move, and on mobile not even a button. Which one won could differ between
   * two launches with the same data.
   *
   * Worse for a screen reader, it changed UNDER the user: at first paint the
   * meeting row is filtered out and the appointment is the row; a beat later
   * the groups arrive, the meeting is re-admitted, and the row at the same
   * index silently becomes a different event.
   *
   * The default answers for the only rows Aperio has that cannot be acted on.
   * A caller that knows more — which calendars are read-only, say — passes its
   * own.
   */
  actionable: (event: E) => boolean = (event) => !isMeetingCalendarEvent(event),
): CollapsedRow<E>[] {
  if (groups.length === 0) {
    return events.map((event) => ({ event, otherMembers: 0, calendarIds: [event.calendar_id], diverged: false }));
  }
  const byMember = indexEventGroups(groups);

  // First pass: which groups are represented in this range, and do their
  // members here agree about when the appointment is. Divergence has to be
  // known BEFORE the first member is folded, because it decides whether to
  // fold at all.
  const startsByGroup = new Map<string, Set<string>>();
  for (const event of events) {
    const group = byMember.get(eventGroupMemberKey(event.calendar_id, seriesId(event)));
    if (!group) continue;
    const seen = startsByGroup.get(group.id) ?? new Set<string>();
    seen.add(startKey(event));
    startsByGroup.set(group.id, seen);
  }

  // Which member each group SHOWS: the first actionable one, else the first at
  // all. Decided over the whole window before anything is emitted, because the
  // row that stands for the group has to be chosen from all of its members and
  // not from the one that happened to come first.
  const showFor = new Map<string, E>();
  for (const event of events) {
    const group = byMember.get(eventGroupMemberKey(event.calendar_id, seriesId(event)));
    if (!group) continue;
    const current = showFor.get(group.id);
    if (current == null || (!actionable(current) && actionable(event))) {
      showFor.set(group.id, event);
    }
  }

  const represented = new Set<string>();
  const out: CollapsedRow<E>[] = [];
  for (const event of events) {
    const group = byMember.get(eventGroupMemberKey(event.calendar_id, seriesId(event)));
    if (!group) {
      out.push({ event, otherMembers: 0, calendarIds: [event.calendar_id], diverged: false });
      continue;
    }
    const diverged = (startsByGroup.get(group.id)?.size ?? 1) > 1;
    // A group whose members have drifted apart is not folded: every copy stays
    // visible, each marked, because that disagreement is the thing to act on.
    if (!diverged && represented.has(group.id)) continue;
    represented.add(group.id);
    // The SLOT is this row's (folding removes rows, it never moves one); the
    // row SHOWN is the group's chosen member. They are the same event unless
    // an unactionable copy came first.
    const shown = diverged ? event : (showFor.get(group.id) ?? event);
    out.push({
      event: shown,
      group,
      otherMembers: Math.max(0, group.members.length - 1),
      calendarIds: [
        shown.calendar_id,
        ...group.members
          .map((m) => m.calendar_id)
          .filter((id) => id !== shown.calendar_id),
      ].filter((id, i, all) => all.indexOf(id) === i),
      diverged,
    });
  }
  return out;
}

/**
 * The mark a SIGHTED user sees on a row that stands for more than itself.
 *
 * Folding was audible and invisible: a screen reader heard "an appointment
 * with 2 others, in Work and Private", and the screen showed an ordinary row
 * — so a sighted user could not tell a folded row from a plain one, and had
 * no way of knowing that a group had drifted apart either. The count reads
 * "3×" (this copy and its two others), and a group whose copies no longer
 * agree about the time adds "≠", the one glyph that says disagreement without
 * a word of any language.
 *
 * Returns `null` for a row that stands only for itself, so a caller can drop
 * the element entirely rather than draw an empty one. The string is decoration
 * for the eye alone — every caller marks it `aria-hidden`, because the row's
 * label already says all of this in words.
 */
export function groupBadge<E>(row: Pick<CollapsedRow<E>, 'group' | 'otherMembers' | 'diverged'>): string | null {
  if (!row.group) return null;
  const copies = row.otherMembers + 1;
  return row.diverged ? `${copies}× ≠` : `${copies}×`;
}
