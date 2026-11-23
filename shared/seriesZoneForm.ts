// The series clock form model — stage 10 of DESIGN-series-time-zone.md.
//
// What an editor needs to hold a series' zone: where the choice shows and
// which clock the date and time fields stand on (the table under "Wo die
// Auswahl erscheint"), which zone a new series starts with, what choosing
// another zone does to the fields, the rule and the exceptions ("Die Zone
// wechseln"), what switching repeat or all-day does ("Wiederholung,
// Ganztägig, Umfänge"), and which zone a save writes ("Prüfen und
// Speichern"). The fields themselves are `clockFields.ts` (stage 8), the rule
// for a zone change is the core's `series_clock_change` (stage 9,
// `seriesClock.ts`).
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
import { ruleWithoutUntil, ruleWithUntil, untilTextOf, zoneUsableHere } from './recurrence';
import {
  canonicalZone,
  seriesClockChange,
  seriesClockZone,
  type ClockChangeAnswer,
  type ClockChangeRefusal,
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

/** What an editor opened, as the table in "Wo die Auswahl erscheint" asks. */
export interface ZoneEditingQuestion {
  /** Whether the editor opened something stored, not a new event or a file. */
  isEdit: boolean;
  /** Whether it opened one occurrence of a series (expanded, or an override). */
  isOccurrence: boolean;
  /** Whether the event opened as a series. */
  repeats: boolean;
  /** The scope asked before the editor opened; `null` where the form asks it
   *  itself (the scope picker in the form, without the question before it). */
  scope: 'series' | 'this_and_future' | 'occurrence' | null;
  /** Whether an appointment read from a file fills the editor. */
  imported: boolean;
  /** Whether the editor only shows the event: a locked invitation, or an
   *  invitation read from a file. */
  readOnly: boolean;
}

/**
 * The row of the table in "Wo die Auswahl erscheint" an editor stands on, the
 * same for both editors; `null` where the editor only shows the event, whose
 * fields stay on the device's clock.
 *
 * An appointment read from a file counts as a stored one (136a): the file
 * names its instants and its zone, so its fields stand on its clock and a zone
 * change keeps the instants.
 */
export function zoneEditingFor(q: ZoneEditingQuestion): ZoneEditing | null {
  if (q.readOnly) return null;
  if (q.imported || (q.isEdit && !q.isOccurrence)) return q.repeats ? 'series' : 'single';
  if (!q.isEdit) return 'new';
  switch (q.scope) {
    case null:
      return 'scope-in-form';
    case 'series':
      return 'series';
    case 'this_and_future':
      return 'following';
    case 'occurrence':
      return 'occurrence';
  }
}

/** The zones a calendar stores for a series (97a); every zone for a calendar
 *  not known (yet), as the list says nothing against it. */
export function calendarZonesOf(
  calendar: { recurrence_capabilities?: { time_zones?: CalendarZones } | null } | null | undefined,
): CalendarZones {
  return calendar?.recurrence_capabilities?.time_zones ?? 'all';
}

/** Whether `calendarZones` stores `zone`, compared canonically as the core
 *  compares it (`SeriesTimeZones::stores`). UTC is no zone and always fits,
 *  under any of its names; so does a name tzdata does not know, which the
 *  series repeats on as UTC and the adapter judges when it writes. */
function storesZone(calendarZones: CalendarZones, zone: string | null): boolean {
  const named = seriesClockZone(zone);
  if (named === null || calendarZones === 'all') return true;
  if (calendarZones === 'none') return false;
  return calendarZones.includes(canonicalZone(named) ?? named);
}

/** The facts of a form, from what its editor holds. */
export function zoneFacts(args: {
  rrule: string | null | undefined;
  allDay: boolean;
  calendar: Parameters<typeof calendarZonesOf>[0];
  /** `deviceZoneName()`, read by the editor. */
  deviceZone: string | null;
  /** Whether the target calendar is another than the one the event opened in. */
  calendarMoved?: boolean;
}): ZoneFormFacts {
  return {
    repeats: !!args.rrule?.trim(),
    allDay: args.allDay,
    calendarZones: calendarZonesOf(args.calendar),
    deviceZone: args.deviceZone,
    calendarMoved: args.calendarMoved ?? false,
  };
}

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
  /** The device's zone, canonically, where the calendar cannot store it and a
   *  new series therefore starts on UTC (22a): the editor says so after the
   *  end time. `null` otherwise. */
  calendarRefuses: string | null;
  /** The zone in effect, canonically, where it was chosen here or the event
   *  moves to another calendar, and the target calendar cannot store it
   *  (139a): the choice stays, the editor says so after the end time, and a
   *  save is refused until another zone or calendar is chosen. `null`
   *  otherwise; an untouched zone in its own calendar is written as stored. */
  notStorable: string | null;
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
  /** Whether the target calendar is another than the one the event opened in. */
  calendarMoved?: boolean;
}

/**
 * A zone as the form writes it (117a): tzdata's spelling, or the name as given
 * where this device can use that and not tzdata's (old zone data reports
 * `Europe/Kiev` and refuses `Europe/Kyiv`). A name tzdata does not know stays
 * as it is. The default and a chosen zone both go through it, so picking the
 * device's own zone from the list (a group's pick from the device, its row
 * "Dieses Gerät") writes what the device can use.
 */
export function writtenZone(name: string): string {
  const canonical = canonicalZone(name);
  if (canonical === null) return name;
  return zoneUsableHere(canonical) || !zoneUsableHere(name) ? canonical : name;
}

/** The device's zone as a new series would get it, before the calendar has
 *  a say ([`writtenZone`]); `null` for a device on UTC or on a name tzdata does
 *  not know (26a). */
function deviceSeriesZone(deviceZone: string | null): string | null {
  const zone = seriesClockZone(deviceZone);
  if (zone === null || canonicalZone(zone) === null) return null;
  return writtenZone(zone);
}

/**
 * The zone a new series starts with: the device's, written canonically
 * (decision 117a) where this device can use that spelling, or
 * UTC where the device reports UTC or a name tzdata does not know (26a) or
 * the calendar cannot store the device's zone (22a). `null` for UTC, and in a
 * calendar that stores no zones.
 */
export function defaultSeriesZone(deviceZone: string | null, calendarZones: CalendarZones): string | null {
  if (calendarZones === 'none') return null;
  const zone = deviceSeriesZone(deviceZone);
  if (zone === null) return null;
  if (calendarZones !== 'all' && !calendarZones.includes(canonicalZone(zone) ?? zone)) return null;
  return zone;
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

/** Whether the series takes the new series' default rather than a stored
 *  zone: nothing stored, or an all-day series that gets times here, which
 *  gets the device's zone whatever zone it carried (46a, 48a, "Ganztägig
 *  aus"), as `tailZone` writes it on a split. */
function takesTheDefault(form: SeriesZoneForm): boolean {
  return form.stored === undefined || form.openedAllDay;
}

/** The zone the series repeats on as the form stands, before a save writes
 *  it: the one chosen, else the stored one, else the new series' default. */
function zoneInEffect(form: SeriesZoneForm, facts: ZoneFormFacts): string | null {
  if (form.chosen !== undefined) return form.chosen;
  if (!takesTheDefault(form)) return form.stored ?? null;
  return defaultSeriesZone(facts.deviceZone, facts.calendarZones);
}

/** What a form says of a series' zone where its fields stand on the device's
 *  clock: nothing. Also the view of an editor that only shows the event (a
 *  locked invitation, an invitation read from a file; `zoneEditingFor` gives
 *  `null` there). */
export const DEVICE_ZONE_VIEW: ZoneFormView = Object.freeze({
  choice: 'none',
  clock: DEVICE_CLOCK,
  zone: null,
  unusable: null,
  calendarRefuses: null,
  notStorable: null,
});

/** The zone a stored event opened with, as `SeriesZoneForm.stored` takes it:
 *  its series' zone as stored, `null` for a series without one, `undefined`
 *  for an event that is no series. */
export function storedZoneOf(
  recurrence: { tzid?: string | null } | null | undefined,
): string | null | undefined {
  return recurrence ? (recurrence.tzid ?? null) : undefined;
}

/**
 * How the fields move when the target calendar changes (question 8 of stage
 * 12, 139a): to the device's clock (a calendar that stores no zones) the
 * instant stays; a new series on its untouched default keeps the typed times,
 * now on the clock that calendar gives it (22a); everything else keeps its
 * instants, and a chosen zone stays chosen.
 */
export function calendarMoveMode(form: SeriesZoneForm, next: ZoneFormView): 'instants' | 'values' {
  if (next.clock.kind === 'device') return 'instants';
  return form.editing === 'new' && form.chosen === undefined ? 'values' : 'instants';
}

/**
 * How the fields move when a title suggestion fills the form ("Titelvorschläge
 * füllen die Regel, lassen die Zone aber stehen") to the view `after`:
 * - where it changes all-day, as that switch moves them: the dates stay;
 * - where the form did not repeat before, as a repeat switched on does: the
 *   instant stays (a calendar the suggestion brings changes while it does not
 *   repeat, on the device's clock, where the instant stays as well);
 * - where the form already repeated, as the calendar it brings moves them
 *   (`calendarMoveMode`), the same as choosing that calendar by hand, also
 *   from a calendar that stores no zones.
 */
export function prefillMoveMode(
  form: SeriesZoneForm,
  after: ZoneFormView,
  change: { allDay: boolean; repeatedBefore: boolean },
): 'instants' | 'values' {
  if (change.allDay) return 'values';
  if (!change.repeatedBefore) return 'instants';
  return calendarMoveMode(form, after);
}

/** What the form shows of the zone, and the clock its fields stand on. */
export function zoneFormView(form: SeriesZoneForm, facts: ZoneFormFacts): ZoneFormView {
  const placed = placement(form, facts);
  if (placed === 'device') return DEVICE_ZONE_VIEW;
  const zone = zoneInEffect(form, facts);
  const { clock, unusable } = seriesFieldClock(zone);
  const device = deviceSeriesZone(facts.deviceZone);
  const calendarRefuses =
    form.chosen === undefined && takesTheDefault(form) && zone === null && device !== null
      ? (canonicalZone(device) ?? device)
      : null;
  const chosenOrMoved = form.chosen !== undefined || facts.calendarMoved === true;
  const notStorable =
    zone !== null && chosenOrMoved && !storesZone(facts.calendarZones, zone)
      ? (canonicalZone(zone) ?? zone)
      : null;
  return { choice: placed, clock, zone, unusable, calendarRefuses, notStorable };
}

/**
 * The zone a save writes for the series, `null` for none ("Prüfen und
 * Speichern"):
 * - a form that does not repeat writes none;
 * - an all-day series writes none where all-day was switched on here, and its
 *   stored value, literally, where it opened all-day (46a);
 * - a chosen zone is written canonically, or in the device's spelling where
 *   the device cannot use tzdata's ([`writtenZone`]); a chosen UTC as none;
 * - an untouched stored zone is written literally, as stored;
 * - a new series, a single event that becomes one, and an all-day series that
 *   gets times here get the default, never over a chosen UTC.
 */
export function zoneToSave(form: SeriesZoneForm, facts: ZoneFormFacts): string | null {
  if (!facts.repeats) return null;
  if (facts.allDay) {
    return form.allDayTurnedOn || form.stored === undefined ? null : form.stored;
  }
  if (form.chosen !== undefined) {
    return form.chosen === null ? null : writtenZone(form.chosen);
  }
  return zoneInEffect(form, facts);
}

/**
 * The recurrence a save writes: the form's rule, the exceptions it holds, and
 * the zone `zoneToSave` names. It replaces `editedRecurrence`, which kept the
 * old zone, so a choice was never written. `exceptions` are the series'
 * stored ones, or the ones a zone change moved (`ClockChangeApplied`).
 */
export function savedRecurrence(
  rrule: string | null | undefined,
  exceptions: string[] | null | undefined,
  zone: string | null,
): { rrule: string; exceptions: string[]; tzid?: string } | null {
  if (!rrule) return null;
  const kept = exceptions ?? [];
  return zone === null ? { rrule, exceptions: kept } : { rrule, exceptions: kept, tzid: zone };
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
  /** The form with the zone chosen; the form as it was where the change is
   *  refused. */
  form: SeriesZoneForm;
  /** The fields on the new clock; as they were where the change is refused. */
  fields: ClockFields;
  /** For something stored: what the core says about the rule, the start and
   *  the exceptions (stage 9). The editor asks when `asks` is set, and
   *  otherwise applies `asNow`, with `applyClockChange`. `null` for a new
   *  event (its typed times stay and its rule takes its days from the date),
   *  and for fields that name no instant (they move as typed). */
  change: SeriesClockChange | null;
  /** Why the change itself is refused: no answer can be written (20a), and
   *  the zone stays. `null` otherwise. */
  refused: ClockChangeRefusal | null;
}

/**
 * Choosing `zone` (`null` for UTC) in the form ("Die Zone wechseln").
 *
 * A new event keeps its typed times, which now apply on the new clock; the
 * rule's days come from the date, so the weekday question never comes. For
 * anything stored, the instant stays: the fields show it on the new clock,
 * starting from what the form holds now, and the core answers what becomes
 * of the rule (`series.rrule`, the form's current rule), the start and
 * `series.exceptions` (the ones the form holds). The core reads the series
 * on the zone it repeats on, which is the stored one even where this device
 * shows the fields on UTC. Fields that name no instant move as typed, and
 * the core is not asked. Where the core can write no answer, the change is
 * refused and nothing moves. `series.endsByDate` is whether the series'
 * stored calendar stores its end as a date (`endsByDateOf`): an end the 105a
 * guard alone reads as a cut then counts through its own date (140a, D1).
 *
 * A rule that is no stored series' own (a single event that becomes a series
 * here) holds its end as the day the user typed, `{D}T235959Z`, until it is
 * saved (141a, D3). That day is no end on any clock yet, so the core is asked
 * about the rule without it, and every answer gets the typed day back as it
 * stood: the field and the save keep the typed day, on the new clock.
 */
export function chooseZone(
  form: SeriesZoneForm,
  facts: ZoneFormFacts,
  fields: ClockFields,
  zone: string | null,
  series: { rrule: string; exceptions: string[]; endsByDate: boolean },
): ZoneChoiceResult {
  const next: SeriesZoneForm = { ...form, chosen: zone };
  const clock: FieldClock = { kind: 'series', tzid: zone };
  if (form.editing === 'new') {
    return { form: next, fields: keepingWallTimes(fields, clock), change: null, refused: null };
  }
  const moved = keepingInstants(fields, clock);
  const start = fieldInstant(fields.start, fields.clock);
  if (start.kind !== 'at' || moved.start.carried) {
    return { form: next, fields: moved, change: null, refused: null };
  }
  const typedEnd = form.editing === 'single' ? untilTextOf(series.rrule) : null;
  const asked = seriesClockChange({
    rrule: typedEnd === null ? series.rrule : ruleWithoutUntil(series.rrule),
    start: start.at,
    exceptions: series.exceptions,
    fromTzid: zoneInEffect(form, facts),
    toTzid: zone,
    endsByDate: series.endsByDate,
  });
  const change = asked === null || typedEnd === null ? asked : withTypedEnd(asked, typedEnd);
  if (change !== null) {
    const answers = change.asks ? [change.asNow, change.asRule] : [change.asNow];
    const written = answers.find((answer) => answer.outcome === 'changed');
    if (written === undefined) {
      const first = answers[0];
      const reason = first.outcome === 'refused' ? first.reason : 'unreadable';
      return { form, fields, change, refused: reason };
    }
  }
  return { form: next, fields: moved, change, refused: null };
}

/** The core's answers with the typed end, `until` as it stood, written back
 *  into each rule it wrote. */
function withTypedEnd(change: SeriesClockChange, until: string): SeriesClockChange {
  const typed = (answer: ClockChangeAnswer): ClockChangeAnswer =>
    answer.outcome === 'changed' ? { ...answer, rrule: ruleWithUntil(answer.rrule, until) } : answer;
  return { ...change, asNow: typed(change.asNow), asRule: typed(change.asRule) };
}

/** The series after an answer of the core: the fields, and the rule and the
 *  exceptions the editor keeps in its state and saves (`savedRecurrence`). */
export interface ClockChangeApplied {
  fields: ClockFields;
  rrule: string;
  exceptions: string[];
}

/**
 * The series after an answer of the core: its rule and its exceptions as the
 * answer writes them, and the fields with the answer's start, the end keeping
 * the elapsed time it had (102a). Fields of an answer that kept the start
 * stay as they are. `null` for a refused answer, which changes nothing.
 */
export function applyClockChange(fields: ClockFields, answer: ClockChangeAnswer): ClockChangeApplied | null {
  if (answer.outcome !== 'changed') return null;
  const { rrule, exceptions } = answer;
  const before = fieldInstant(fields.start, fields.clock);
  const end = fieldInstant(fields.end, fields.clock);
  if (before.kind !== 'at' || Date.parse(before.at) === Date.parse(answer.start)) {
    return { fields, rrule, exceptions };
  }
  const start = fieldAt(answer.start, fields.clock);
  if (end.kind !== 'at') return { fields: { ...fields, start }, rrule, exceptions };
  const movedEnd = new Date(Date.parse(end.at) + (Date.parse(answer.start) - Date.parse(before.at)));
  return {
    fields: { ...fields, start, end: fieldAt(movedEnd.toISOString(), fields.clock) },
    rrule,
    exceptions,
  };
}

/** The form with all-day switched on or off here: a series that becomes
 *  all-day in this session writes no zone ("Ganztägig an"). */
export function withAllDay(form: SeriesZoneForm, allDay: boolean): SeriesZoneForm {
  return { ...form, allDayTurnedOn: allDay && !form.openedAllDay };
}

/**
 * The fields on the clock `view` puts them on.
 * - `'instants'`, after repeat is switched off or on: the instants stay, and
 *   the fields show them on the new clock ("Wiederholung aus", "Wiederholung
 *   wieder an"); values that name no instant stay as typed.
 * - `'values'`, after all-day is switched on or off: the dates stay ("die
 *   Daten bleiben"), and the times the form kept come back as typed on the
 *   clock it now stands on ("Ganztägig aus").
 */
export function fieldsOnView(
  fields: ClockFields,
  view: ZoneFormView,
  keep: 'instants' | 'values',
): ClockFields {
  const same =
    fields.clock.kind === view.clock.kind &&
    (fields.clock.kind === 'device' ||
      (view.clock.kind === 'series' && fields.clock.tzid === view.clock.tzid));
  if (same) return fields;
  return keep === 'instants' ? keepingInstants(fields, view.clock) : keepingWallTimes(fields, view.clock);
}

/**
 * The series as a zone answer leaves it (question 12 of stage 12): the
 * answer's start, the end keeping the length the series had, its rule and
 * exceptions, and the new zone. A whole-series save measures the edit against
 * this, not the stored series, so the answer is not applied twice: a start the
 * answer moved to the rule's weekday is the start the edit begins from, and a
 * date moved after the answer counts on the new clock from there.
 */
export function zoneAppliedSeries<
  E extends {
    start: string;
    end: string;
    recurrence?: { rrule: string; exceptions: string[]; tzid?: string | null } | null;
  },
>(series: E, answer: { start: string; rrule: string; exceptions: string[] }, zone: string | null): E {
  const length = Date.parse(series.end) - Date.parse(series.start);
  const end = new Date(Date.parse(answer.start) + length).toISOString();
  const recurrence = { ...(series.recurrence ?? {}), rrule: answer.rrule, exceptions: answer.exceptions };
  return {
    ...series,
    start: answer.start,
    end,
    recurrence: zone === null ? { ...recurrence, tzid: null } : { ...recurrence, tzid: zone },
  };
}

/**
 * Why choosing a zone for a stored series waits, or `null` where it may go
 * ahead. The core is asked against the series as stored or as the last zone
 * answer left it (`baselineStart`), and a whole-series save measures the edit
 * against the answered series (`zoneAppliedSeries`), so a change made after
 * the choice moves the rule and the deleted occurrences as any edit does.
 * Made before it, part of it would be lost in between, so the editor says so
 * instead of asking (predictable rather than half right):
 * - `'start'`: the start was changed in this session, or the series opened
 *   all-day and now has times ("Speichere zuerst den geänderten Beginn");
 * - `'times'`: the start or the end names no instant (a time the clock skips,
 *   half a value), so there is nothing to ask the core about.
 * `baselineStart` is `null` for anything else (a new event, a single event,
 * an import), which never waits.
 */
export function zoneChoiceWait(
  form: SeriesZoneForm,
  fields: ClockFields,
  baselineStart: string | null,
): 'start' | 'times' | null {
  if (form.editing !== 'series' || baselineStart === null) return null;
  if (form.openedAllDay) return 'start';
  const start = fieldInstant(fields.start, fields.clock);
  if (start.kind !== 'at' || fieldInstant(fields.end, fields.clock).kind !== 'at') return 'times';
  return Date.parse(start.at) === Date.parse(baselineStart) ? null : 'start';
}

/**
 * The zone a zone answer was computed on, as a save writes it: the chosen
 * zone (`writtenZone`, `null` for UTC), or the stored one where nothing was
 * chosen. The series a whole-series save measures against carries it
 * (`zoneAppliedSeries`), whatever the save then writes: an all-day series
 * switched on afterwards writes no zone, but its deletions are read where the
 * answer put them.
 */
export function answeredZone(form: SeriesZoneForm): string | null {
  if (form.chosen === undefined) return form.stored ?? null;
  return form.chosen === null ? null : writtenZone(form.chosen);
}
