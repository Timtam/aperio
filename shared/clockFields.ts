// Date and time fields on a clock — the form's half of stage 8 of
// DESIGN-series-time-zone.md.
//
// A series' start and end are typed and shown on the clock it repeats on: the
// zone its stored name names, or UTC (`cal_core::series_clock`). A single
// event, "only this one" and a series set to "repeat off" stay on the device's
// clock (the table under "Wo die Auswahl erscheint"). This module holds a pair
// of fields on either clock, and the rules that keep them predictable:
//
// - A field nobody typed in keeps the instant it shows: the stored one, to the
//   second, even in the second copy of a doubled hour, and a save writes
//   exactly that. Moving the fields to another clock keeps it as well, so a
//   switch away and back returns the starting values.
// - A typed time reads once, and takes the earlier instant where the clock
//   shows it twice. On a series' clock it has no instant where the clock skips
//   it: the answer is a gap carrying the parts of the message, not a guess. The
//   device's clock keeps today's reading, where the runtime moves a skipped
//   time forward.
// - A pair with a value that names no instant moves to another clock as typed,
//   both values, and stays so on the way back.
// - Moving the start moves the end by the same elapsed time (decision 102a).
//   The end is then read from an instant, so it never lands in a gap by itself;
//   an end typed into a gap stays there until the person changes it.
//
// The core reads a series' clock (`shared/wallClock.ts`), with the tzdata the
// adapters write with. This runtime's Date reads the device's clock, as
// `eventDateTime.ts` does. An all-day event's dates are the device's days on
// every clock (decision 48a), so they stay with `eventDateTime.ts`.
//
// Pure: nothing here reads an editor's state or the device's zone; the caller
// passes what a function needs. The editors start using it in stages 10 and 12.

import { combine, dateInput, timeInput } from './eventDateTime';
import { zoneUsableHere } from './recurrence';
import { canonicalZone, seriesClockZone } from './seriesClock';
import { instantOf, wallOf } from './wallClock';

/** The clock a pair of fields stands on. A series' clock carries the name the
 *  series stores, or `null`; the core reads that name as a zone or as UTC. */
export type FieldClock = { kind: 'device' } | { kind: 'series'; tzid: string | null };

/** The device's clock. */
export const DEVICE_CLOCK: FieldClock = { kind: 'device' };

/** One date-and-time field. */
export interface ClockField {
  /** `YYYY-MM-DD` on the field's clock. */
  date: string;
  /** `HH:MM` on the field's clock. */
  time: string;
  /** The instant the field shows while nobody types in it: the stored one,
   *  verbatim, or the one a move gave it. `null` once the field was typed in. */
  at: string | null;
  /** Set on values carried from another clock as typed, because the pair named
   *  no instant there ({@link keepingInstants}). They move on as typed, so a
   *  switch away and back returns them. Typing in the field clears it. */
  carried?: true;
}

/** A start and an end on one clock. */
export interface ClockFields {
  clock: FieldClock;
  start: ClockField;
  end: ClockField;
}

/** The instant a field names. */
export type FieldInstant =
  /** An instant: the one the field holds, or the reading of what was typed. */
  | { kind: 'at'; at: string }
  /** The typed time does not exist on the series' clock, which skips it on
   *  that date: the parts of „{Zeit} gibt es am {Datum} in {Uhr} nicht“, and
   *  the offsets in seconds east of UTC before and after the change. */
  | {
      kind: 'gap';
      date: string;
      time: string;
      /** The canonical zone. UTC never skips a time, so it is never `null`
       *  here, but it is typed as the core answers it. */
      clock: string | null;
      offsetBefore: number;
      offsetAfter: number;
    }
  /** The field is not (yet) a date and a time of day. */
  | { kind: 'unreadable' };

/** The keys of the four fields, as the editors name them. */
export type ClockFieldKey = 'startDate' | 'startTime' | 'endDate' | 'endTime';

const UNREADABLE_FIELD: ClockField = { date: '', time: '', at: null };

/** A field showing the instant `at` on `clock`, and holding it. A text that
 *  names no instant gives empty fields that hold nothing. */
export function fieldAt(at: string, clock: FieldClock): ClockField {
  if (clock.kind === 'device') {
    const instant = new Date(at);
    if (Number.isNaN(instant.getTime())) return UNREADABLE_FIELD;
    return { date: dateInput(instant), time: timeInput(instant), at };
  }
  const wall = wallOf(at, clock.tzid);
  if (wall === null || wall.date === undefined || wall.time === undefined) {
    return UNREADABLE_FIELD;
  }
  return { date: wall.date, time: wall.time, at };
}

/** Fields showing a stored start and end on `clock`, holding both. */
export function fieldsAt(start: string, end: string, clock: FieldClock): ClockFields {
  return { clock, start: fieldAt(start, clock), end: fieldAt(end, clock) };
}

/** The instant `field` names on `clock`. */
export function fieldInstant(field: ClockField, clock: FieldClock): FieldInstant {
  if (field.at !== null) return { kind: 'at', at: field.at };
  if (clock.kind === 'device') {
    const typed = combine(field.date, field.time, false);
    return typed === null ? { kind: 'unreadable' } : { kind: 'at', at: typed.toISOString() };
  }
  const answer = instantOf(field.date, field.time, clock.tzid);
  const wall = answer?.wall;
  if (answer === null || wall === undefined) return { kind: 'unreadable' };
  if (wall.reading === 'gap') {
    return {
      kind: 'gap',
      date: field.date,
      time: field.time,
      clock: answer.clock,
      offsetBefore: wall.offsetBefore,
      offsetAfter: wall.offsetAfter,
    };
  }
  // Once, or twice: a typed time takes the earlier instant, which `at` is.
  return { kind: 'at', at: wall.at };
}

/** The values alone: what a field is once someone types in it. */
function typedValues(field: ClockField): ClockField {
  return { date: field.date, time: field.time, at: null };
}

/** The part of a held instant below the minute its field shows, in ms: the
 *  seconds of a stored instant, or of a zone's old offset (Paris kept
 *  +00:09:21 until 1911). */
function belowTheMinute(at: string, ms: number, clock: FieldClock): number {
  const millis = ((ms % 1000) + 1000) % 1000;
  if (clock.kind === 'device') return new Date(ms).getSeconds() * 1000 + millis;
  return (wallOf(at, clock.tzid)?.second ?? 0) * 1000 + millis;
}

/**
 * The milliseconds of the minute a field shows, for moving and comparing the
 * fields, or `null` when it is no date and time. This is what
 * `applyDateTimeChange` measures on the device's clock: a held instant counts
 * without the seconds its field does not show, in its own copy of a doubled
 * hour. A time the clock skips counts as it would read with the offset from
 * before the change, as RFC 5545 reads it and as the device's runtime moves
 * it; a save still refuses it.
 */
function shownMs(field: ClockField, clock: FieldClock): number | null {
  const named = fieldInstant(field, clock);
  if (named.kind === 'unreadable') return null;
  if (named.kind === 'gap') {
    const [y, m, d] = named.date.split('-').map(Number);
    const [hh, mm] = named.time.split(':').map(Number);
    return Date.UTC(y, m - 1, d, hh, mm) - named.offsetBefore * 1000;
  }
  const ms = Date.parse(named.at);
  if (Number.isNaN(ms)) return null;
  return field.at === null ? ms : ms - belowTheMinute(field.at, ms, clock);
}

/**
 * The fields with one value changed to `value`, which the person typed or
 * picked. The changed field stops holding an instant. A value equal to the one
 * shown changes nothing, so a picker confirmed without a change keeps a stored
 * instant to the second.
 *
 * - A changed start moves the end by the same elapsed time (decision 102a),
 *   and the end holds the instant it moved to. A start the clock skips moves it
 *   as the offset from before the change reads it, so the duration holds once
 *   the start is corrected. An end that names no instant (typed into a gap, or
 *   half a value) stays as it is, and so does one that would move off the
 *   calendar.
 * - A changed end only resizes the event, but never ends before the start: an
 *   end before it becomes the start.
 *
 * As `applyDateTimeChange` does for the device's clock today.
 */
export function changeClockField(
  fields: ClockFields,
  key: ClockFieldKey,
  value: string,
): ClockFields {
  const side = key === 'startDate' || key === 'startTime' ? 'start' : 'end';
  const part = key === 'startDate' || key === 'endDate' ? 'date' : 'time';
  if (fields[side][part] === value) return fields;
  const typed: ClockField = { ...typedValues(fields[side]), [part]: value };
  const { clock } = fields;

  if (side === 'start') {
    const next: ClockFields = { ...fields, start: typed };
    if (fieldInstant(fields.end, clock).kind !== 'at') return next;
    const before = shownMs(fields.start, clock);
    const after = shownMs(typed, clock);
    const end = shownMs(fields.end, clock);
    if (before === null || after === null || end === null) return next;
    const moved = new Date(end + (after - before));
    if (Number.isNaN(moved.getTime())) return next;
    const movedEnd = fieldAt(moved.toISOString(), clock);
    if (movedEnd.at !== null) next.end = movedEnd;
    return next;
  }

  const start = shownMs(fields.start, clock);
  const end = shownMs(typed, clock);
  if (start !== null && end !== null && end < start) {
    return { ...fields, end: { ...fields.start } };
  }
  return { ...fields, end: typed };
}

/**
 * The fields moved to `clock`, keeping the instants they name. This is how
 * anything stored changes clocks (a whole series, an event that becomes a
 * series here, "repeat off"): the fields show the same moment on the new clock
 * and hold it, so moving back shows exactly what was there before.
 *
 * Where either field names no instant (a time the old clock skips, or half a
 * value), both keep their values as typed, carried: the pair has no moment to
 * keep, and moving only one of them would put the end on another day than the
 * start. Carried values move on as typed, so switching back returns them, gap
 * and all ("Wiederholung aus" in DESIGN-series-time-zone.md keeps such values
 * the same way).
 */
export function keepingInstants(fields: ClockFields, clock: FieldClock): ClockFields {
  const carry = (field: ClockField): ClockField => ({ ...typedValues(field), carried: true });
  const start = fieldInstant(fields.start, fields.clock);
  const end = fieldInstant(fields.end, fields.clock);
  if (start.kind !== 'at' || end.kind !== 'at') {
    return { clock, start: carry(fields.start), end: carry(fields.end) };
  }
  const move = (field: ClockField, at: string): ClockField =>
    field.carried ? field : fieldAt(at, clock);
  return { clock, start: move(fields.start, start.at), end: move(fields.end, end.at) };
}

/**
 * The fields moved to `clock`, keeping their values: what was typed for a new
 * event now applies on the new clock. Nothing is held any more, so every value
 * is read again on the new clock, and one it skips is a gap at once.
 */
export function keepingWallTimes(fields: ClockFields, clock: FieldClock): ClockFields {
  return { clock, start: typedValues(fields.start), end: typedValues(fields.end) };
}

/**
 * Whether `clock` reads every instant as the device does, so the fields need
 * no clock in their names and no hint after them. `deviceZone` is the zone as
 * the runtime reports it (`Intl.DateTimeFormat().resolvedOptions().timeZone`),
 * `null` when it reports none.
 *
 * Compared by canonical name, because V8 reports old names such as
 * `Asia/Calcutta`. A series on UTC matches a device on one of tzdata's names
 * for UTC. A device reporting a name tzdata does not know matches no series.
 */
export function sameClockAsDevice(clock: FieldClock, deviceZone: string | null): boolean {
  if (clock.kind === 'device') return true;
  if (deviceZone === null) return false;
  const device = canonicalZone(deviceZone);
  if (device === null) return false;
  const zone = seriesClockZone(clock.tzid);
  if (zone === null) return seriesClockZone(deviceZone) === null;
  return canonicalZone(zone) === device;
}

/** The clock a timed series' fields stand on here, and the zone it stores when
 *  this device cannot use it. */
export interface SeriesFieldClock {
  clock: FieldClock;
  /** The stored name of a zone the core reads but this device's `Intl`
   *  refuses (old Android zone data). The views then show the series on UTC,
   *  so its fields stand on UTC too, and changed times cannot be saved.
   *  `null` when the device can use the zone, or the series is on UTC. */
  unusable: string | null;
}

/**
 * The clock the fields of a timed series storing `tzid` stand on, on this
 * device: the series' own clock, or UTC where this device cannot use its zone
 * ("Eine gespeicherte Zone, die Intl auf diesem Gerät ablehnt"). It says so
 * rather than falling back without a word. An all-day series stands on the
 * device's days (decision 48a) and does not ask.
 */
export function seriesFieldClock(tzid: string | null | undefined): SeriesFieldClock {
  const zone = seriesClockZone(tzid);
  if (zone !== null && !zoneUsableHere(zone)) {
    return { clock: { kind: 'series', tzid: null }, unusable: zone };
  }
  return { clock: { kind: 'series', tzid: tzid ?? null }, unusable: null };
}
