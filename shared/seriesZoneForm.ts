// The series clock form model — stage 10 of DESIGN-series-time-zone.md.
//
// What an editor needs to hold a series' zone: where the choice shows and
// which clock the date and time fields stand on (the table under "Wo die
// Auswahl erscheint"), which zone a new series starts with, what choosing
// another zone does to the fields and the rule ("Die Zone wechseln"), and
// which zone a save writes ("Prüfen und Speichern"). The fields themselves are
// `clockFields.ts` (stage 8), the rule for a zone change is the core's
// `series_clock_change` (stage 9, `seriesClock.ts`).
//
// Pure: the editors hold a `SeriesZoneForm` in their state and ask these
// functions; nothing here reads the device or an editor. They start using it
// in stage 12.

import {
  DEVICE_CLOCK,
  fieldAt,
  fieldInstant,
  keepingInstants,
  keepingWallTimes,
  seriesFieldClock,
  type ClockFields,
  type FieldClock,
} from './clockFields';
import {
  canonicalZone,
  seriesClockChange,
  seriesClockZone,
  type ClockChangeAnswer,
  type SeriesClockChange,
} from './seriesClock';

/** What the editor edits, by the rows of the table in "Wo die Auswahl
 *  erscheint". */
export type ZoneEditing =
  /** A new event. */
  | 'new'
  /** A stored single event; it becomes a series when the form repeats. */
  | 'single'
  /** A whole series. */
  | 'series'
  /** "This and all following" (21a). */
  | 'following'
  /** "Only this one". */
  | 'occurrence'
  /** The scope picker inside the form, without the question before it. */
  | 'scope-in-form';

/** The zones the target calendar can store for a series (its manifest's
 *  `recurrence.time_zones`, decision 97a). */
export type CalendarZones = 'all' | 'none' | string[];

/** The zone part of an editor's state. */
export interface SeriesZoneForm {
  editing: ZoneEditing;
  /** The zone the series stored when the editor opened, as stored; `null` for
   *  a series without one; `undefined` for a new or single event. */
  stored: string | null | undefined;
  /** Whether the event was all-day when the editor opened. */
  openedAllDay: boolean;
  /** The zone chosen in this session: `undefined` while untouched, `null`
   *  for UTC. */
  chosen: string | null | undefined;
  /** Whether all-day was switched on in this session. */
  allDayTurnedOn: boolean;
}

/** A form for what the editor opened. */
export function openZoneForm(
  editing: ZoneEditing,
  stored: string | null | undefined,
  openedAllDay: boolean,
): SeriesZoneForm {
  return { editing, stored, openedAllDay, chosen: undefined, allDayTurnedOn: false };
}

/** What the form shows of the zone, and the clock its fields stand on. */
export interface ZoneFormView {
  /** The choice itself, a hint naming the series' zone (21a), or nothing. */
  choice: 'picker' | 'hint' | 'none';
  /** The clock the date and time fields stand on. A series of days stands on
   *  the device's days (48a), and so does every field the table puts on the
   *  device's clock. */
  clock: FieldClock;
  /** The zone the series repeats on as the form stands, as it would be
   *  written (`null` for UTC); `null` where no series zone applies. */
  zone: string | null;
  /** The stored name of a zone the core reads but this device refuses: the
   *  fields stand on UTC, and changed times or a changed rule cannot be saved
   *  (`unusableZoneRefusal`). */
  unusable: string | null;
}

/** The facts of the form a view and a save need. */
export interface ZoneFormFacts {
  /** Whether the form repeats now. */
  repeats: boolean;
  /** Whether the form is all-day now. */
  allDay: boolean;
  /** The target calendar's zones. */
  calendarZones: CalendarZones;
  /** The device's zone as its runtime reports it, `null` for none. */
  deviceZone: string | null;
}

/**
 * The zone a new series starts with: the device's, written canonically
 * (decision 117a, provisional), or UTC where the device reports UTC or a
 * name tzdata does not know (26a) or the calendar cannot store the device's
 * zone (22a). `null` for UTC, and in a calendar that stores no zones.
 */
export function defaultSeriesZone(deviceZone: string | null, calendarZones: CalendarZones): string | null {
  if (calendarZones === 'none') return null;
  const zone = seriesClockZone(deviceZone);
  const canonical = zone === null ? null : canonicalZone(zone);
  if (canonical === null) return null;
  if (calendarZones !== 'all' && !calendarZones.includes(canonical)) return null;
  return canonical;
}

/** Whether the table puts the fields on the series' clock, and shows the
 *  choice there. */
function placement(form: SeriesZoneForm, facts: ZoneFormFacts): 'picker' | 'hint' | 'device' {
  if (!facts.repeats || facts.allDay) return 'device';
  switch (form.editing) {
    case 'new':
    case 'single':
    case 'series':
      // A calendar that stores no zones (the device's calendars) shows no
      // choice; its series stand on the device's clock, as today.
      return facts.calendarZones === 'none' ? 'device' : 'picker';
    case 'following':
      return 'hint';
    default:
      return 'device';
  }
}

/** The zone the series repeats on as the form stands, before a save writes
 *  it: the one chosen, else the stored one, else the new series' default. */
function zoneInEffect(form: SeriesZoneForm, facts: ZoneFormFacts): string | null {
  if (form.chosen !== undefined) return form.chosen;
  if (form.stored !== undefined && !(form.openedAllDay && form.stored === null)) {
    return form.stored;
  }
  return defaultSeriesZone(facts.deviceZone, facts.calendarZones);
}

/** What the form shows of the zone, and the clock its fields stand on. */
export function zoneFormView(form: SeriesZoneForm, facts: ZoneFormFacts): ZoneFormView {
  const placed = placement(form, facts);
  if (placed === 'device') {
    return { choice: 'none', clock: DEVICE_CLOCK, zone: null, unusable: null };
  }
  const zone = zoneInEffect(form, facts);
  const { clock, unusable } = seriesFieldClock(zone);
  return { choice: placed, clock, zone, unusable };
}

/**
 * The zone a save writes for the series, `null` for none ("Prüfen und
 * Speichern"):
 * - a form that does not repeat writes none;
 * - an all-day series writes none where all-day was switched on here, and its
 *   stored value, literally, where it opened all-day (46a);
 * - a chosen zone is written canonically, a chosen UTC as none;
 * - an untouched stored zone is written literally, as stored; a stored
 *   all-day series that gets times here gets the new series' default;
 * - a new series, and a single event that becomes one, gets the default,
 *   never over a chosen UTC.
 */
export function zoneToSave(form: SeriesZoneForm, facts: ZoneFormFacts): string | null {
  if (!facts.repeats) return null;
  if (facts.allDay) {
    return form.allDayTurnedOn || form.stored === undefined ? null : form.stored;
  }
  if (form.chosen !== undefined) {
    return form.chosen === null ? null : (canonicalZone(form.chosen) ?? form.chosen);
  }
  if (form.stored !== undefined && !(form.openedAllDay && form.stored === null)) {
    return form.stored;
  }
  return defaultSeriesZone(facts.deviceZone, facts.calendarZones);
}

/**
 * The recurrence a save writes: the form's rule, the exceptions of the series
 * it edits, and the zone `zoneToSave` names. It replaces `editedRecurrence`,
 * which kept the old zone, so a choice was never written.
 */
export function savedRecurrence(
  rrule: string | null | undefined,
  previous: { exceptions: string[] } | null | undefined,
  zone: string | null,
): { rrule: string; exceptions: string[]; tzid?: string } | null {
  if (!rrule) return null;
  const exceptions = previous?.exceptions ?? [];
  return zone === null ? { rrule, exceptions } : { rrule, exceptions, tzid: zone };
}

/**
 * Why a save of a series whose zone this device cannot use is refused: the
 * fields stand on UTC there, so changed times or a changed rule would be
 * written on the wrong clock ("Eine gespeicherte Zone, die Intl auf diesem
 * Gerät ablehnt"). Choosing another zone is allowed, and so is a save that
 * changes neither. `null` where the save may go ahead; else the zone's
 * stored name, for the message.
 */
export function unusableZoneRefusal(
  view: ZoneFormView,
  form: SeriesZoneForm,
  changed: { times: boolean; rule: boolean },
): string | null {
  if (view.unusable === null || form.chosen !== undefined) return null;
  return changed.times || changed.rule ? view.unusable : null;
}

/** What choosing a zone does. */
export interface ZoneChoiceResult {
  form: SeriesZoneForm;
  /** The fields on the new clock. */
  fields: ClockFields;
  /** For something stored: what the core says about the rule, the start and
   *  the exceptions (stage 9). The editor asks when `asks` is set and applies
   *  the answer with `applyClockChange`; otherwise it applies `asNow`. `null`
   *  for a new event, whose typed times stay and whose rule takes its days
   *  from the date. */
  change: SeriesClockChange | null;
}

/**
 * Choosing `zone` (`null` for UTC) in the form ("Die Zone wechseln").
 *
 * A new event keeps its typed times, which now apply on the new clock; the
 * rule's days come from the date, so the weekday question never comes. For
 * anything stored, the instant stays: the fields show it on the new clock,
 * starting from what the form holds now, and the core answers what becomes
 * of the rule (`rrule`, the form's current rule), the start and the
 * `exceptions`.
 */
export function chooseZone(
  form: SeriesZoneForm,
  fields: ClockFields,
  zone: string | null,
  series: { rrule: string; exceptions: string[] },
): ZoneChoiceResult {
  const next: SeriesZoneForm = { ...form, chosen: zone };
  const clock: FieldClock = { kind: 'series', tzid: zone };
  if (form.editing === 'new') {
    return { form: next, fields: keepingWallTimes(fields, clock), change: null };
  }
  const moved = keepingInstants(fields, clock);
  const start = fieldInstant(fields.start, fields.clock);
  if (start.kind !== 'at') {
    // No instant to keep: nothing to ask the core about the days yet.
    return { form: next, fields: moved, change: null };
  }
  const from = fields.clock.kind === 'series' ? fields.clock.tzid : null;
  const change = seriesClockChange({
    rrule: series.rrule,
    start: start.at,
    exceptions: series.exceptions,
    fromTzid: from,
    toTzid: zone,
  });
  return { form: next, fields: moved, change };
}

/**
 * The fields after an answer of the core moved the start (as the rule says,
 * onto its old day): the start shows the answer's start, and the end keeps
 * the elapsed time it had (102a). Fields of an answer that kept the start
 * come back unchanged. A refused answer changes nothing.
 */
export function applyClockChange(fields: ClockFields, answer: ClockChangeAnswer): ClockFields {
  if (answer.outcome !== 'changed') return fields;
  const before = fieldInstant(fields.start, fields.clock);
  const end = fieldInstant(fields.end, fields.clock);
  if (before.kind !== 'at' || Date.parse(before.at) === Date.parse(answer.start)) return fields;
  const start = fieldAt(answer.start, fields.clock);
  if (end.kind !== 'at') return { ...fields, start };
  const movedEnd = new Date(Date.parse(end.at) + (Date.parse(answer.start) - Date.parse(before.at)));
  return { ...fields, start, end: fieldAt(movedEnd.toISOString(), fields.clock) };
}

/** The form with all-day switched on or off here: a series that becomes
 *  all-day in this session writes no zone ("Ganztägig an"). */
export function withAllDay(form: SeriesZoneForm, allDay: boolean): SeriesZoneForm {
  return { ...form, allDayTurnedOn: allDay && !form.openedAllDay };
}

/**
 * The fields on the clock `view` puts them on, keeping their instants: after
 * repeat is switched off they go back to the device's clock, after it is
 * switched on again to the series' clock, and values that name no instant
 * stay as typed ("Wiederholung aus", "Wiederholung wieder an").
 */
export function fieldsOnView(fields: ClockFields, view: ZoneFormView): ClockFields {
  const same =
    fields.clock.kind === view.clock.kind &&
    (fields.clock.kind === 'device' ||
      (view.clock.kind === 'series' && fields.clock.tzid === view.clock.tzid));
  return same ? fields : keepingInstants(fields, view.clock);
}
