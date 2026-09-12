// Carrying a change to the other copies (DESIGN-event-groups.md, Stufe 2) —
// this surface's door into `cal_core::group_carry`.
//
// A group says several events mean the same appointment. When the appointment
// moves, all of them have to move — that is the second thing groups are for,
// after not being read out four times. Doing it by hand is what the feature
// exists to end: whoever forgets a copy has calendars that contradict each
// other and finds out when somebody turns up at the wrong time.
//
// Two things are deliberately narrow, and both are decided in the core now.
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
//
// The core answers with FIELD VALUES, never with rows: a caller's own row
// carries far more than these six fields, and the core has no business knowing
// about them. The door lays the answer over what the caller already holds,
// which is also what keeps a copy's reminder untouched.

import type { EventGroup } from './eventGroups';

// The wire types are generated from `cal_core::group_carry` by
// `cargo xtask ts-types`; nothing here mirrors them by hand, so a field the
// Rust side renames is a type error on this side, not a silent `undefined`.
export type { CarryableFields } from './generated/CarryableFields';
export type { CarryField } from './generated/CarryField';
export type { CarryTarget } from './generated/CarryTarget';
export type { CarryPlan } from './generated/CarryPlan';
import type { CarryableFields } from './generated/CarryableFields';
import type { CarryPlan } from './generated/CarryPlan';

/**
 * Which occurrences an edit — and therefore its carry — is about.
 *
 * `series` updates each copy's row. `occurrence` does to each copy what the
 * edit did to the anchor: EXDATE the series at that instant and put a
 * standalone event in its place. `future` splits each copy's own series at the
 * same point, the way the anchor's was split — an update there would move
 * EVERY occurrence of that copy because one of them was edited, which is the
 * outcome the scope question exists to prevent.
 *
 * A caller-side discriminator: it decides WHICH rule to ask, so it stays here.
 */
export type CarryScope = 'series' | 'occurrence' | 'future';

/** This surface's door into `cal_core::group_carry`. */
export interface GroupCarryRules {
  planCarryJson(inputJson: string): string;
  occurrenceCarryFieldsJson(inputJson: string): string;
  futureCarryFieldsJson(inputJson: string): string;
  carryOntoFieldsJson(inputJson: string): string;
}

let installedRules: GroupCarryRules | null = null;

/** Bind this surface's door into the core. */
export function installGroupCarryRules(rules: GroupCarryRules): void {
  installedRules = rules;
}

function rules(): GroupCarryRules {
  if (installedRules === null) {
    // Loud, not a local fallback. A fallback here would be the second
    // implementation all over again, and its failure is the one this feature
    // exists to prevent: calendars that contradict each other, found out when
    // somebody turns up at the wrong time.
    throw new Error(
      'group carry used before installGroupCarryRules() — the surface must ' +
        'bind its door into cal_core::group_carry at startup',
    );
  }
  return installedRules;
}

/** The six fields, and nothing of the caller's own row. */
function onlyCarryable(fields: CarryableFields): CarryableFields {
  return {
    title: fields.title,
    start: fields.start,
    end: fields.end,
    all_day: fields.all_day,
    location: fields.location ?? null,
    description: fields.description ?? null,
  };
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
 * already. `isWritable` and `titleOf` are the two things only a surface knows,
 * so the door resolves them per member before crossing.
 */
export function planCarry(
  group: EventGroup,
  anchor: { calendar_id: string; event_id: string },
  before: CarryableFields,
  after: CarryableFields,
  isWritable: (calendarId: string) => boolean,
  titleOf: (calendarId: string, eventId: string) => string,
): CarryPlan {
  const answer = rules().planCarryJson(
    JSON.stringify({
      members: group.members.map((m) => ({
        calendar_id: m.calendar_id,
        event_id: m.event_id,
        title: titleOf(m.calendar_id, m.event_id),
        writable: isWritable(m.calendar_id),
      })),
      anchor_calendar_id: anchor.calendar_id,
      anchor_event_id: anchor.event_id,
      before: onlyCarryable(before),
      after: onlyCarryable(after),
    }),
  );
  return JSON.parse(answer) as CarryPlan;
}

/** Whether the plan is worth asking the user about at all. */
export function worthCarrying(plan: CarryPlan): boolean {
  return plan.worth_carrying;
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
 * `null` when the occurrence instant cannot be read: this copy could not be
 * carried, and the caller reports it rather than writing a row at an instant
 * nobody chose.
 */
export function occurrenceCarryRow<T extends CarryableFields>(
  master: T,
  occurrenceIso: string,
  after: CarryableFields,
  changed: readonly (keyof CarryableFields)[],
): T | null {
  const answer = rules().occurrenceCarryFieldsJson(
    JSON.stringify({
      master: onlyCarryable(master),
      at: occurrenceIso,
      before: onlyCarryable(master),
      after: onlyCarryable(after),
      changed,
    }),
  );
  const fields = JSON.parse(answer) as CarryableFields | null;
  return fields == null ? null : ({ ...master, ...fields } as T);
}

/**
 * The row a carried "this and all following" edit creates in a member's calendar.
 *
 * Unlike `occurrenceCarryRow` this one carries the MOVE, not the instant — "and
 * all following" cuts each copy at ITS own next occurrence, which need not be
 * the anchor's. The whole reasoning, and the two narrowings for all-day copies,
 * are in `cal_core::group_carry::future_carry_fields`.
 *
 * `null` when the cut point cannot be read. The TypeScript this replaces THREW
 * there, abandoning the rest of the carry mid-loop rather than reporting the one
 * copy; both callers already keep a list of members they could not write.
 */
export function futureCarryRow<T extends CarryableFields>(
  master: T,
  anchorIso: string,
  before: CarryableFields,
  after: CarryableFields,
  changed: readonly (keyof CarryableFields)[],
): T | null {
  const answer = rules().futureCarryFieldsJson(
    JSON.stringify({
      master: onlyCarryable(master),
      at: anchorIso,
      before: onlyCarryable(before),
      after: onlyCarryable(after),
      changed,
    }),
  );
  const fields = JSON.parse(answer) as CarryableFields | null;
  return fields == null ? null : ({ ...master, ...fields } as T);
}

/**
 * Apply the carried fields onto one member's own current values.
 *
 * A member keeps everything else it has — its calendar, its colour, and above
 * all its reminders. Only what the appointment IS travels.
 */
export function carryOnto<T extends CarryableFields>(
  member: T,
  after: CarryableFields,
  changed: readonly (keyof CarryableFields)[],
): T {
  const answer = rules().carryOntoFieldsJson(
    JSON.stringify({
      master: onlyCarryable(member),
      at: '',
      before: onlyCarryable(member),
      after: onlyCarryable(after),
      changed,
    }),
  );
  return { ...member, ...(JSON.parse(answer) as CarryableFields) } as T;
}
