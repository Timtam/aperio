// Folding a group into one row (DESIGN-event-groups.md, Stufe 1) — this
// surface's door into `cal_core::event_group_fold`.
//
// A group says several events mean the same appointment. Here that changes what
// a day looks like: four rows that are one commitment become one row that names
// the calendars it spans — the largest everyday gain of the feature, and for a
// screen-reader user not a cosmetic one: three fewer things to walk past, every
// time.
//
// A day that read differently on the phone than on the desktop would be worse
// than not folding at all, which is why the decision is in the core and this is
// only the way in.

import type { EventGroup } from './eventGroups';

/** The minimum a row has to carry to be foldable. */
export interface CollapsibleEvent {
  id: string;
  calendar_id: string;
  start?: string | null;
  all_day?: boolean;
}

/** One rendered row after folding. */
export interface CollapsedRow<E> {
  /** The row to draw. For a group, the member chosen to stand for it. */
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

/** This surface's door into `cal_core::event_group_fold`. */
export interface EventGroupFold {
  /** `{events[], groups[]}` in, one row per surviving slot out. */
  collapseEventGroupsJson(inputJson: string): string;
}

let installedFold: EventGroupFold | null = null;

/** Bind this surface's door into the core. */
export function installEventGroupFold(fold: EventGroupFold): void {
  installedFold = fold;
}

/**
 * Fold each group's members into a single row, keeping the input order.
 *
 * The representative is chosen over the whole window before anything is
 * emitted — the first member the user can ACT on, else the first at all —
 * because the row that stands for the group has to be picked from all of its
 * members and not from whichever came first. The SLOT is still the first
 * member's: folding removes rows, it never moves one.
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
 *
 */
export function collapseEventGroups<E extends CollapsibleEvent>(
  events: readonly E[],
  groups: readonly EventGroup[],
  seriesId: (event: E) => string,
  /**
   * Whether this row is one the user can ACT on — the tie-breaker for which
   * member a folded group shows.
   *
   * Omit it, and the CORE answers: the only rows Aperio has that cannot be
   * acted on are the read-only videoconference copies, and it recognises
   * those. Every caller in this repository omits it. A caller that knows more
   * — which calendars are read-only, say — can still say so, which is why the
   * hook survived the move into the core rather than being quietly dropped.
   */
  actionable?: (event: E) => boolean,
): CollapsedRow<E>[] {
  if (installedFold === null) {
    // Loud, not a local fallback. A fallback would be the second
    // implementation all over again, and the failure it produces is one a
    // screen-reader user meets head on: a day that folds on one device and
    // not on the other.
    throw new Error(
      'collapseEventGroups used before installEventGroupFold() — the surface ' +
        'must bind its door into cal_core::event_group_fold at startup',
    );
  }
  const byId = new Map(groups.map((group) => [group.id, group]));
  const answer = installedFold.collapseEventGroupsJson(
    JSON.stringify({
      events: events.map((ev) => ({
        calendar_id: ev.calendar_id,
        series_id: seriesId(ev),
        start: ev.start ?? null,
        all_day: ev.all_day ?? false,
        actionable: actionable == null ? null : actionable(ev),
      })),
      groups,
    }),
  );
  const rows = JSON.parse(answer) as {
    event: number;
    group_id?: string;
    other_members: number;
    calendar_ids: string[];
    diverged: boolean;
  }[];
  return rows.map((row) => ({
    event: events[row.event],
    ...(row.group_id == null ? {} : { group: byId.get(row.group_id) }),
    otherMembers: row.other_members,
    calendarIds: row.calendar_ids,
    diverged: row.diverged,
  }));
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
