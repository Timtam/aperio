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
// carries far more than these fields, and the core has no business knowing
// about them. The door lays the answer over what the caller already holds,
// which is also what keeps a copy's reminder untouched. A series' zone is
// named and never copied: each copy is asked how it takes it (`zoneCarry`,
// below).

import type { EventGroup } from './eventGroups';
import { endsByDateOf } from './recurrence';
import { calendarZonesOf, type ZoneMove } from './seriesZoneForm';
import { zoneCityName } from './zoneFormText';

// The wire types are generated from `cal_core::group_carry` by
// `cargo xtask ts-types`; nothing here mirrors them by hand, so a field the
// Rust side renames is a type error on this side, not a silent `undefined`.
export type { CarryableFields } from './generated/CarryableFields';
export type { CarriedZone } from './generated/CarriedZone';
export type { CarryField } from './generated/CarryField';
export type { CarryTarget } from './generated/CarryTarget';
export type { CarryPlan } from './generated/CarryPlan';
export type { CopyFacts } from './generated/CopyFacts';
export type { SeriesFacts } from './generated/SeriesFacts';
export type { ZoneCarry } from './generated/ZoneCarry';
export type { ZoneCarryQuestion } from './generated/ZoneCarryQuestion';
export type { ZoneCarryRefusal } from './generated/ZoneCarryRefusal';
import type { CarryableFields } from './generated/CarryableFields';
import type { CarryField } from './generated/CarryField';
import type { CarryPlan } from './generated/CarryPlan';
import type { CarryTarget } from './generated/CarryTarget';
import type { ZoneCarry } from './generated/ZoneCarry';
import type { ZoneCarryQuestion } from './generated/ZoneCarryQuestion';
import type { ZoneCarryRefusal } from './generated/ZoneCarryRefusal';

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
  /** What one copy gets when a series' zone change is carried to it
   *  (`cal_core::zone_carry`): a question as JSON in, the answer as JSON
   *  out. */
  zoneCarryJson(questionJson: string): string;
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

/** The carried fields, and nothing of the caller's own row. The zone of a
 *  timed series goes only where the caller set it (stage 13); a row without
 *  one sends none. */
function onlyCarryable(fields: CarryableFields): CarryableFields {
  return {
    title: fields.title,
    start: fields.start,
    end: fields.end,
    all_day: fields.all_day,
    location: fields.location ?? null,
    description: fields.description ?? null,
    ...(fields.zone ? { zone: { tzid: fields.zone.tzid ?? null } } : {}),
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

// ─── A series' zone, carried to its copies (stage 13) ─────────────────────
//
// DESIGN-series-time-zone.md, section 23b; decisions 143a to 146a. The zone
// goes in the one carry question, first among the changes (143a). Each
// writable copy is asked of the core before the offer opens, and again in
// the carry loop: how it takes the zone, or why it cannot. A copy that
// cannot still gets its title, location and description (146a).

/** A copy's refusal of the zone, `not_a_series` included. */
export type ZoneCarryRefused = Extract<ZoneCarry, { outcome: 'refused' }>;

/** A refusal the offer names with its reason: a series that does not get
 *  the zone (146a). A copy that is no series there is named apart
 *  (`ZoneOffer.notASeries`). */
export type ZoneSeriesRefused = ZoneCarryRefused & {
  reason: Exclude<ZoneCarryRefusal, 'not_a_series'>;
};

/**
 * What one copy gets when a series' zone change is carried to it
 * (`cal_core::zone_carry`): unchanged, the copy as written on its new clock
 * with how far its occurrences move (`shiftMinutes`) and how many days for
 * the zone (`zoneDays`, negative is earlier), or refused with a reason.
 * One rule for the offer and the carry, so both name the same reason.
 */
export function zoneCarry(question: ZoneCarryQuestion): ZoneCarry {
  return JSON.parse(rules().zoneCarryJson(JSON.stringify(question))) as ZoneCarry;
}

/** A copy as the carry reads it, and the calendar it is in. */
interface CopyRow {
  start: string;
  end: string;
  all_day: boolean;
  recurrence?: { rrule: string; exceptions?: string[] | null; tzid?: string | null } | null;
}
type CopyCalendar = Parameters<typeof calendarZonesOf>[0] & Parameters<typeof endsByDateOf>[0];

/**
 * What one copy, as just read, gets from a series' zone move: the core's
 * verdict (`zoneCarry`) on the copy's own facts and its calendar's (the
 * zones it stores, whether it stores an end as a date). The offer asks it of
 * every copy before it opens, the carry again of the copy it is about to
 * write, so both name the same reason. `today` is the surface's clock.
 */
export function zoneVerdictFor(
  move: ZoneMove,
  copy: CopyRow,
  calendar: CopyCalendar,
  today: string,
): ZoneCarry {
  return zoneCarry({
    ...move,
    copy: {
      rrule: copy.recurrence?.rrule ?? null,
      start: copy.start,
      end: copy.end,
      exceptions: copy.recurrence?.exceptions ?? [],
      tzid: copy.recurrence?.tzid ?? null,
      allDay: copy.all_day,
    },
    timeZones: calendarZonesOf(calendar),
    endsByDate: endsByDateOf(calendar),
    today,
  });
}

/** The key a copy's verdict is kept under, for `zoneOffer`. */
export function carryTargetKey(target: { calendar_id: string; event_id: string }): string {
  return JSON.stringify([target.calendar_id, target.event_id]);
}

const TEXT_FIELDS: readonly CarryField[] = ['title', 'location', 'description'];

/** The changed fields that go to a copy whatever its zone does (146a):
 *  title, location and description, in the plan's order. */
export function textFields(changed: readonly CarryField[]): CarryField[] {
  return changed.filter((field) => TEXT_FIELDS.includes(field));
}

/** What the offer after a zone change says and does, from the plan and each
 *  copy's verdict. */
export interface ZoneOffer {
  /** The copies the carry goes to: every writable copy but those it would
   *  write nothing to — one already standing as the series does
   *  (`unchanged`) or that is no series there, with no text field changed.
   *  A refused series stays: it gets the text fields, or its reason is
   *  reported. */
  targets: CarryTarget[];
  /** The series that do not get the zone, with their verdict (146a). */
  refused: { target: CarryTarget; verdict: ZoneSeriesRefused }[];
  /** The writable copies that are no series there: the zone does not apply
   *  to them, and the offer says so once. Those with a changed text field
   *  are targets too. */
  notASeries: CarryTarget[];
  /** How far the written copies' occurrences move for the zone, in days,
   *  grouped, earliest first: one sentence each (negative is earlier, "vor",
   *  138a). Copies whose days stay are left out. */
  days: { days: number; count: number }[];
  /** No copy would be written: the offer can only close. */
  nothingToCarry: boolean;
  /** Nothing to say at all: every copy already stands as the series does,
   *  no text field changed, and no copy was skipped. No dialog opens. */
  silent: boolean;
}

/**
 * The offer after a series' zone change (stage 13), for both surfaces: from
 * the plan and the verdict of each writable copy, keyed by `carryTargetKey`.
 * A copy without a verdict (its read failed) counts as a target that would
 * be written; the carry asks again once it has read it.
 *
 * A copy that is no series there gets its title, location and description
 * (146a), never the series' start or end: those are instants of the series,
 * and a single event elsewhere would land on the series' first day, or end
 * before it starts. Without a changed text field it gets nothing: it is named
 * in `notASeries` only, never in `targets`, and counts as not written, so a
 * zone-only save with a single-event copy offers no carry and writes nothing.
 */
export function zoneOffer(plan: CarryPlan, verdicts: ReadonlyMap<string, ZoneCarry>): ZoneOffer {
  const texts = textFields(plan.changed).length > 0;
  const targets: CarryTarget[] = [];
  const refused: ZoneOffer['refused'] = [];
  const notASeries: CarryTarget[] = [];
  const days = new Map<number, number>();
  let written = 0;
  let everyUnchanged = true;
  for (const target of plan.targets) {
    const verdict = verdicts.get(carryTargetKey(target));
    if (verdict === undefined) {
      everyUnchanged = false;
      targets.push(target);
      written += 1;
      continue;
    }
    switch (verdict.outcome) {
      case 'unchanged':
        if (texts) {
          targets.push(target);
          written += 1;
        }
        break;
      case 'changed':
        everyUnchanged = false;
        targets.push(target);
        written += 1;
        if (verdict.zoneDays !== 0) days.set(verdict.zoneDays, (days.get(verdict.zoneDays) ?? 0) + 1);
        break;
      case 'refused':
        everyUnchanged = false;
        if (verdict.reason === 'not_a_series') {
          notASeries.push(target);
          if (texts) {
            targets.push(target);
            written += 1;
          }
        } else {
          refused.push({ target, verdict: verdict as ZoneSeriesRefused });
          targets.push(target);
          if (texts) written += 1;
        }
        break;
    }
  }
  return {
    targets,
    refused,
    notASeries,
    days: [...days].sort(([a], [b]) => a - b).map(([moved, count]) => ({ days: moved, count })),
    nothingToCarry: written === 0,
    silent: everyUnchanged && !texts && plan.skipped.length === 0,
  };
}

type Translate = (key: string, values?: Record<string, unknown>) => string;

/**
 * Why a series does not get the zone, as the clause the offer and the report
 * put after "bekommt die Zeitzone nicht:". `zone` is the series' new zone,
 * named by its city. A rule that cannot move says the rule's own reason, as
 * the zone question does (`dialogs.moveScope.refusal.*`).
 */
export function zoneRefusalText(verdict: ZoneSeriesRefused, zone: string | null, t: Translate): string {
  const city = zoneCityName(zone);
  if (verdict.reason === 'rule_cannot_move') {
    return t(`dialogs.moveScope.refusal.${verdict.rule ?? 'unreadable'}`, { clock: city });
  }
  return t(`dialogs.eventGroupCarry.zoneRefusal.${verdict.reason}`, { zone: city });
}
