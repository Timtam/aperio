// Carrying a change to the other copies (DESIGN-event-groups.md, Stufe 2).
//
// A group says several events mean the same appointment. When the appointment
// moves, all of them have to move — that is the second thing groups are for,
// after not being read out four times. Doing it by hand is what the feature
// exists to end: whoever forgets a copy has calendars that contradict each
// other and finds out when somebody turns up at the wrong time.
//
// Two things are deliberately narrow here.
//
// WHICH FIELDS travel: what the appointment IS (title, when, where, what it
// says), and nothing that is a property of the copy. Reminders above all — the
// private copy usually exists precisely because it has a reminder the work one
// does not, and carrying those across would delete the reason for the copy.
// Colour, calendar and attendees are per-copy for the same kind of reason.
//
// WHICH MEMBERS travel: only those Aperio may write. A colleague's calendar is
// read-only, and the design is explicit that "carry to all" must SAY which
// members it could not do rather than skip them quietly — otherwise it
// produces exactly the contradiction it set out to prevent.

import type { EventGroup } from './eventGroups';
import { eventGroupMemberKey } from './eventGroups';

/** The fields a change is carried in. */
export interface CarryableFields {
  title: string;
  start: string;
  end: string;
  all_day: boolean;
  location: string | null;
  description: string | null;
}

/** One member of the group, as the caller knows it. */
export interface CarryTarget {
  calendar_id: string;
  event_id: string;
  /** The name to show when reporting what happened to it. */
  title: string;
  /** Whether Aperio may write to the calendar it lives in. */
  writable: boolean;
}

/**
 * Which occurrences an edit — and therefore its carry — is about.
 *
 * `series` updates each copy's row. `occurrence` does to each copy what the
 * edit did to the anchor: EXDATE the series at that instant and put a
 * standalone event in its place. `future` splits each copy's own series at the
 * same point, the way the anchor's was split — an update there would move
 * EVERY occurrence of that copy because one of them was edited, which is the
 * outcome the scope question exists to prevent.
 */
export type CarryScope = 'series' | 'occurrence' | 'future';

/** What carrying would do, decided before anything is written. */
export interface CarryPlan {
  /** The fields that actually differ from the saved event. */
  changed: (keyof CarryableFields)[];
  /** Members Aperio can and will write. */
  targets: CarryTarget[];
  /** Members it must leave alone, and the user has to be told about. */
  skipped: CarryTarget[];
}

const CARRIED: (keyof CarryableFields)[] = [
  'title',
  'start',
  'end',
  'all_day',
  'location',
  'description',
];

/** Treat empty and absent as the same thing — providers disagree about which
 *  they return for a field the user never filled in, and a change from `null`
 *  to `""` is not a change anybody made. */
function same(a: unknown, b: unknown): boolean {
  const norm = (v: unknown) => (v == null || v === '' ? null : v);
  return norm(a) === norm(b);
}

/** The same, for the two fields that are INSTANTS.
 *
 *  The editor rebuilds start/end through `toIso`, which is not the spelling the
 *  backend sent ("…T08:00:00.000Z" vs "…T08:00:00Z"). Compared as strings, every
 *  save looked like it had moved the appointment — so a reminder-only edit asked
 *  to carry, and carrying rewrote start and end onto every copy. */
function sameInstant(a: string, b: string): boolean {
  const at = new Date(a).getTime();
  const bt = new Date(b).getTime();
  if (Number.isFinite(at) && Number.isFinite(bt)) return at === bt;
  return same(a, b);
}

/**
 * What carrying this edit to the group's other copies would do.
 *
 * Decided from the data, before the question is even asked: with nothing
 * changed there is nothing to carry and no reason to ask, and with every other
 * member read-only the honest answer is "this cannot be carried" rather than a
 * dialog that does nothing.
 *
 * `anchor` is the copy that was edited — it is excluded, having been saved
 * already.
 */
export function planCarry(
  group: EventGroup,
  anchor: { calendar_id: string; event_id: string },
  before: CarryableFields,
  after: CarryableFields,
  isWritable: (calendarId: string) => boolean,
  titleOf: (calendarId: string, eventId: string) => string,
): CarryPlan {
  const changed = CARRIED.filter((field) =>
    field === 'start' || field === 'end'
      ? !sameInstant(String(before[field]), String(after[field]))
      : !same(before[field], after[field]),
  );
  const anchorKey = eventGroupMemberKey(anchor.calendar_id, anchor.event_id);
  const targets: CarryTarget[] = [];
  const skipped: CarryTarget[] = [];
  for (const member of group.members) {
    if (eventGroupMemberKey(member.calendar_id, member.event_id) === anchorKey) {
      continue;
    }
    const target: CarryTarget = {
      calendar_id: member.calendar_id,
      event_id: member.event_id,
      title: titleOf(member.calendar_id, member.event_id),
      writable: isWritable(member.calendar_id),
    };
    (target.writable ? targets : skipped).push(target);
  }
  return { changed, targets, skipped };
}

/** Whether the plan is worth asking the user about at all. */
export function worthCarrying(plan: CarryPlan): boolean {
  return plan.changed.length > 0 && plan.targets.length > 0;
}

/**
 * The standalone row a carried OCCURRENCE edit creates in a member's calendar.
 *
 * An occurrence edit is not an update: the series gets an EXDATE and a single
 * event is created in its place. Carrying it means doing that on each copy, and
 * the row created there is NOT the anchor's — it is the member's own occurrence
 * with the carried fields laid over it. So a private copy keeps its own
 * reminder, its own colour and its own calendar; what travels is what the
 * appointment IS.
 *
 * Start and end come from the member's own occurrence unless the edit MOVED it:
 * a title-only edit must not drag the copy to the master's start, and a moved
 * occurrence must land where the user put it. The copies are aligned by the
 * premise of the group, so the anchor's new instants are the right ones.
 */
export function occurrenceCarryRow<T extends CarryableFields>(
  master: T,
  occurrenceIso: string,
  after: CarryableFields,
  changed: readonly (keyof CarryableFields)[],
): T {
  const masterStart = new Date(master.start).getTime();
  const masterEnd = new Date(master.end).getTime();
  const durationMs = Number.isFinite(masterStart) && Number.isFinite(masterEnd)
    ? Math.max(0, masterEnd - masterStart)
    : 0;
  const occurrenceStart = new Date(occurrenceIso);
  const row = {
    ...master,
    start: occurrenceIso,
    end: new Date(occurrenceStart.getTime() + durationMs).toISOString(),
  } as T;
  return carryOnto(row, after, changed);
}

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * The row a carried "this and all following" edit creates in a member's calendar.
 *
 * Unlike `occurrenceCarryRow` this one carries the MOVE, not the instant. The
 * two scopes differ exactly there: an occurrence edit lands on one instant that
 * every copy shares, while "and all following" cuts each copy at ITS own next
 * occurrence — which need not be the anchor's, because a copy may run to a
 * different pattern, or have that one occurrence deleted.
 *
 * Writing the anchor's new instant onto such a copy was wrong twice over: the
 * head was truncated before the copy's own occurrence while the tail began at
 * the anchor's, so the two halves did not meet — the copy lost a real
 * appointment, gained one on a day it never had, and every occurrence after it
 * fell out of phase. So what travels is the SHIFT the user made (start moved by
 * so much, duration is now so long), applied to the copy's own cut point.
 *
 * Two deliberate narrowings:
 *   - an ALL-DAY copy moves in whole days, whatever the anchor's shift was to
 *     the minute; a start that is not local midnight is not an all-day event;
 *   - the anchor's new DURATION is adopted only when both agree about being
 *     all-day, so an hour-long edit cannot shrink an all-day copy to an hour.
 *
 * The duration is always derived, never an instant taken from the anchor: an
 * end lifted verbatim onto a copy cut at a different point could precede its
 * own start, which no provider will accept and some will accept and mangle.
 */
export function futureCarryRow<T extends CarryableFields>(
  master: T,
  anchorIso: string,
  before: CarryableFields,
  after: CarryableFields,
  changed: readonly (keyof CarryableFields)[],
): T {
  const anchorMs = new Date(anchorIso).getTime();
  const ownDuration = Math.max(
    0,
    new Date(master.end).getTime() - new Date(master.start).getTime(),
  );
  const movedBy = changed.includes('start')
    ? new Date(after.start).getTime() - new Date(before.start).getTime()
    : 0;
  const shift = master.all_day
    ? Math.round(movedBy / DAY_MS) * DAY_MS
    : movedBy;
  const sameKind = master.all_day === after.all_day;
  const duration =
    changed.includes('end') && sameKind
      ? Math.max(0, new Date(after.end).getTime() - new Date(after.start).getTime())
      : ownDuration;
  const start = Number.isFinite(anchorMs) ? anchorMs + shift : anchorMs;
  const row = {
    ...master,
    start: new Date(start).toISOString(),
    end: new Date(start + duration).toISOString(),
  } as T;
  // Everything else travels as it does everywhere else; start and end were just
  // decided above and must not be overwritten with the anchor's instants.
  return carryOnto(
    row,
    after,
    changed.filter((field) => field !== 'start' && field !== 'end'),
  );
}

/** Apply the carried fields onto one member's own current values.
 *
 *  A member keeps everything else it has — its calendar, its colour, and above
 *  all its reminders. Only what the appointment IS travels. */
export function carryOnto<T extends CarryableFields>(
  member: T,
  after: CarryableFields,
  changed: readonly (keyof CarryableFields)[],
): T {
  const next = { ...member };
  for (const field of changed) {
    (next as CarryableFields)[field] = after[field] as never;
  }
  return next;
}
