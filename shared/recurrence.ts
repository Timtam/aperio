import { RRule, rrulestr } from 'rrule';

import { compareMachineStrings } from './ordering';
import { expansionClock, seriesClockZone, type SeriesExpansionClock } from './seriesClock';
import { cutBefore, seriesEnd, untilThrough, wallOf, type SeriesEnd } from './wallClock';
import type { FieldClock } from './clockFields';

// Event recurrence expansion, shared by desktop + mobile. Generic over a
// minimal `RecurringEventLike` so it needs neither side's full `CalendarEvent`
// type (those still live per-app for now); any event with id/start/end + the
// `{rrule, exceptions}` recurrence shape works. Desktop re-exports this from
// `src/intl/recurrence.ts`; mobile imports it directly.

/** The minimal event shape the expander needs. Both the desktop and mobile
 *  `CalendarEvent` satisfy it. */
export interface RecurringEventLike {
  id: string;
  /** RFC-3339 (UTC) start/end of the master event. */
  start: string;
  end: string;
  /** An all-day event: its start is LOCAL MIDNIGHT of its day, and its series
   *  repeats on the device's calendar days rather than on UTC (decision 48a).
   *  Absent reads as a timed event, which is what a source without the field
   *  can only be. */
  all_day?: boolean;
  /** Cancelled/tombstone flag. A cancelled RECURRENCE-ID override (its id carries
   *  `::rid::`) is a pure deletion marker — `expandAll` uses it to suppress the
   *  master's occurrence and then drops it, so a deleted occurrence vanishes. */
  cancelled?: boolean;
  recurrence: {
    rrule: string;
    exceptions: string[];
    /** IANA zone of the master DTSTART (e.g. `America/New_York`), when the
     *  source carried one. A zone → expand in it so occurrences keep their
     *  local wall-clock across DST; absent/null, a UTC name, or a name tzdata
     *  does not know → expand in UTC (see `seriesClock.ts`). */
    tzid?: string | null;
  } | null;
}

/** An expanded per-occurrence copy of `E`: same fields, but `start`/`end`
 *  shifted to the occurrence, a unique `id`, and the master id kept as
 *  `series_id` so the edit/delete layer can find the underlying row. */
export type ExpandedOccurrence<E extends RecurringEventLike> = E & {
  series_id: string;
  occurrence_start: string;
};

/**
 * Expand a recurring event into all of its occurrences inside `range`.
 *
 * Returns the original event unchanged when there is no recurrence rule.
 * Otherwise produces one copy per occurrence whose `start`/`end` match the
 * occurrence time and whose `id` is suffixed with the occurrence start (ISO) so
 * list keys stay unique; the master `id` is preserved as `series_id`. `EXDATE`
 * entries are honoured (rrule.js filters them out).
 *
 * Time-zone caveat: `start` is RFC-3339 (UTC). rrule.js works in `Date`
 * instants taken from `dtstart`; the result instants are re-serialised via
 * `toISOString()`.
 */
export function expandEvent<E extends RecurringEventLike>(
  event: E,
  range: { start: Date; end: Date },
): (E | ExpandedOccurrence<E>)[] {
  if (!event.recurrence?.rrule) {
    return [event];
  }

  const dtstart = new Date(event.start);
  const dtend = new Date(event.end);
  const duration = dtend.getTime() - dtstart.getTime();
  // WHICH clock the rule is read on is the core's rule (`expansion_clock`,
  // decision 48a); WHICH zone this device is in is ours to supply, because the
  // core reads no clock. An all-day series therefore steps on the days its
  // reader sees — read on UTC, as it was until now, a named weekday landed a
  // day late east of Greenwich.
  const clock = expansionClock(event.all_day === true, event.recurrence.tzid);
  const byDay = clock === 'device-days';
  const tzid = zoneOfClock(clock, event.recurrence.tzid);
  const facts = endFacts(event);

  let occurrences: Date[];
  try {
    occurrences = tzid
      ? zonedOccurrences(event.recurrence.rrule, dtstart, tzid, range, facts)
      : utcOccurrences(untilOnClock(event.recurrence.rrule, facts, null), dtstart, range);
  } catch (err) {
    // Bad rule string — fall back to showing the master at its stored start so
    // the user can still see and edit it.
    // eslint-disable-next-line no-console
    console.warn('failed to expand RRULE', event.recurrence.rrule, err);
    return [event];
  }
  // The views iterate in the clock's wall time; the end's own instant is the
  // last one an occurrence may start at. They differ around a clock change:
  // a cut at an occurrence the spring change pushes past its gap ends one
  // second before it, a wall time after the occurrence's nominal one, and the
  // cut occurrence stood in the old series as well as in the new. The
  // reminders cap at the same instant.
  const cap = endCap(event.recurrence.rrule, facts, byDay ? tzid : null);
  if (cap !== null) occurrences = occurrences.filter((d) => d.getTime() <= cap);
  if (occurrences.length === 0) {
    return [];
  }

  // An exception cancels an occurrence. A timed series names an instant, and
  // an instant is compared exactly. A series of DAYS names a day, and the
  // instant it was stored as is only one spelling of it: one written before
  // 48a, or by a provider that anchors a date elsewhere, sits hours off the
  // local midnight the occurrence has (decision 95).
  const exceptions = event.recurrence.exceptions
    .map((iso) => new Date(iso).getTime())
    .filter((at) => !Number.isNaN(at));
  const cancelled = byDay
    ? (d: Date) => exceptions.some((at) => namesTheSameDay(at, d.getTime()))
    : namesExactly(exceptions);

  return occurrences
    .filter((d) => !cancelled(d))
    .map<ExpandedOccurrence<E>>((occStart) => {
      const occEnd = new Date(occStart.getTime() + duration);
      return {
        ...event,
        id: `${event.id}@${occStart.toISOString()}`,
        series_id: event.id,
        occurrence_start: occStart.toISOString(),
        start: occStart.toISOString(),
        end: occEnd.toISOString(),
      };
    });
}

/** The recurrence zone, or `null` to expand in UTC: no zone, a UTC name
 *  (`Etc/UTC`, `GMT`, …), or a name tzdata does not know. The core's rule. */
function zoneOrNull(tzid: string | null | undefined): string | null {
  return seriesClockZone(tzid);
}

/** True when this series steps on the DEVICE's calendar days: the core's
 *  answer for an all-day series (48a), whatever zone it carries. */
function readsCalendarDays(event: {
  all_day?: boolean;
  recurrence?: { tzid?: string | null } | null;
}): boolean {
  return expansionClock(event.all_day === true, event.recurrence?.tzid) === 'device-days';
}

/**
 * The zone a rule read on `clock` is expanded in, or `null` for UTC.
 *
 * The core chooses WHICH clock; this fills in the one name the core may not
 * know — the device's own zone, for a series of days. A device that reports
 * UTC, or a name tzdata or `Intl` does not know, reads days on UTC, which is
 * the same reading. Takes the clock rather than asking for it again: every
 * expansion of every series on screen passes through here.
 */
function zoneOfClock(
  clock: SeriesExpansionClock,
  tzid: string | null | undefined,
): string | null {
  switch (clock) {
    case 'device-days':
      return usableZone(localTimeZone());
    case 'zone':
      return usableZone(zoneOrNull(tzid));
    default:
      return null;
  }
}

/** The zone this series' rule is read on, or `null` for UTC. */
function expansionZoneFor(event: {
  all_day?: boolean;
  recurrence?: { tzid?: string | null } | null;
}): string | null {
  const tzid = event.recurrence?.tzid;
  return zoneOfClock(expansionClock(event.all_day === true, tzid), tzid);
}

/**
 * A series' rule as `expandEvent` iterates it — on the clock the core chose, in
 * that clock's wall time — with the zone to turn a wall time back into an
 * instant, `null` for UTC. `null` when the rule cannot be read.
 */
function wallRule(series: {
  start: string;
  all_day?: boolean;
  recurrence: { rrule: string; tzid?: string | null };
}): { rule: RRule; zone: string | null } | null {
  const dtstart = new Date(series.start);
  if (Number.isNaN(dtstart.getTime())) return null;
  const zone = expansionZoneFor(series);
  try {
    if (zone) {
      try {
        return {
          rule: buildRule(
            untilOnClock(series.recurrence.rrule, endFacts(series), zone),
            realToWall(dtstart, zone),
          ),
          zone,
        };
      } catch {
        // A zone `Intl` cannot load reads on UTC, as `zonedOccurrences` does.
      }
    }
    const body = untilOnClock(series.recurrence.rrule, endFacts(series), null);
    return { rule: buildRule(body, dtstart), zone: null };
  } catch {
    return null;
  }
}

/**
 * How many occurrences a series' rule generates from its start, its own bound
 * ending them — counted up to `limit`, past which it answers `limit + 1`. The
 * same occurrences `expandEvent` gives, without an exception, and without
 * turning each into an instant: an editor asks on every change of a date, and
 * thousands of zoned occurrences through `Intl` froze the field. `null` when
 * the rule cannot be read.
 */
export function occurrenceCount(
  series: { start: string; all_day?: boolean; recurrence: { rrule: string; tzid?: string | null } },
  limit: number,
): number | null {
  const built = wallRule(series);
  if (!built) return null;
  const cap = capOf(series, built.zone);
  let count = 0;
  // For a rule that repeats daily or slower, only the last few can start past
  // the end's own instant (an occurrence a spring change pushes past its gap),
  // so only they become instants. A rule that repeats by the hour or minute
  // can have such occurrences before its last ones; it is counted as before
  // (TODO.md).
  const tail: Date[] = [];
  built.rule.all((at) => {
    count += 1;
    if (cap !== null) {
      tail.push(at);
      if (tail.length > 3) tail.shift();
    }
    return count <= limit;
  });
  if (cap !== null && count <= limit) {
    for (let i = tail.length - 1; i >= 0 && realOf(tail[i], built.zone) > cap; i -= 1) {
      count -= 1;
    }
  }
  return count;
}

/**
 * The `nth` occurrence (counted from 1) of a series' rule from its start, as an
 * instant; `null` when it has fewer, or the rule cannot be read. Like
 * {@link occurrenceCount}, only the one asked for is turned into an instant.
 */
export function nthOccurrence(
  series: { start: string; all_day?: boolean; recurrence: { rrule: string; tzid?: string | null } },
  nth: number,
): string | null {
  const built = wallRule(series);
  if (!built || nth < 1) return null;
  let seen = 0;
  let found: Date | null = null;
  built.rule.all((at) => {
    seen += 1;
    if (seen === nth) {
      found = at;
      return false;
    }
    return true;
  });
  if (found === null) return null;
  const at: Date = found;
  const cap = capOf(series, built.zone);
  if (cap !== null && realOf(at, built.zone) > cap) return null;
  return (built.zone ? wallToReal(at, built.zone) : at).toISOString();
}

/** A wall-clock occurrence of `wallRule` as the instant it is, in ms. */
function realOf(wall: Date, zone: string | null): number {
  return (zone ? wallToReal(wall, zone) : wall).getTime();
}

/** The end's own instant for a series `wallRule` read on `zone`, in ms, the
 *  cap `expandEvent` applies; `null` without one. */
function capOf(
  series: { start: string; all_day?: boolean; recurrence: { rrule: string; tzid?: string | null } },
  zone: string | null,
): number | null {
  return endCap(series.recurrence.rrule, endFacts(series), readsCalendarDays(series) ? zone : null);
}

/** {@link expansionZoneFor} for a series named by its two facts. */
function clockZone(allDay: boolean, tzid: string | null | undefined): string | null {
  return zoneOfClock(expansionClock(allDay, tzid), tzid);
}

/**
 * Half a day. Two instants this close name the same DAY of a series of days,
 * and two that far apart never can: an all-day occurrence is local midnight,
 * and the instant another writer stored for that same day — midnight UTC,
 * noon, the local midnight of a zone an hour or two away, or the UTC-stepped
 * instant a build before 48a wrote — sits within hours of it, while the
 * neighbouring occurrence is a whole day away.
 *
 * Compared as DAYS instead, the reading is not symmetric: midnight UTC falls
 * on the day before in every zone west of Greenwich, so an exception would
 * cancel a day the user never deleted — and hiding a day is worse than showing
 * one that should be gone (`expandAll` keeps what it cannot place).
 */
const SAME_DAY_MS = 12 * 60 * 60 * 1000;

/** Whether `stored` and `occurrence` name the same day of a series of days. */
function namesTheSameDay(stored: number, occurrence: number): boolean {
  return Math.abs(stored - occurrence) < SAME_DAY_MS;
}

/**
 * Whether two instants name the same occurrence of `series`: exactly for a
 * timed series, the same day for a series of days (decision 95). It is the
 * reading the views make of a series' exceptions (`expandEvent`) and of the
 * rows that stand in for its occurrences (`expandAll`), and a split reads them
 * the same way (`deletedSlots`), so the two cannot drift apart. A loop over
 * one series takes {@link slotMatcher} instead.
 */
export function sameSlot(
  series: { all_day?: boolean; recurrence?: { tzid?: string | null } | null },
  a: number,
  b: number,
): boolean {
  return slotMatcher(series)(a, b);
}

/**
 * {@link sameSlot} for one series, asked once. Which clock a series is read
 * on is the core's answer (`expansionClock`), a call across the WASM or FFI
 * boundary; a loop over a year of occurrences asked it for every comparison.
 */
export function slotMatcher(series: {
  all_day?: boolean;
  recurrence?: { tzid?: string | null } | null;
}): (a: number, b: number) => boolean {
  return readsCalendarDays(series) ? namesTheSameDay : (a, b) => a === b;
}

/** The timed reading: an instant is an instant, compared exactly. */
function namesExactly(instants: readonly number[]): (d: Date) => boolean {
  const set = new Set(instants);
  return (d) => set.has(d.getTime());
}

function buildRule(rruleBody: string, dtstart: Date): RRule {
  // rrulestr accepts a full RFC-5545 "RRULE:..." block; if the stored string is
  // just the body (FREQ=...;BYDAY=...) prepend the marker.
  const body = rruleBody.trim();
  const text = body.toUpperCase().startsWith('RRULE:') ? body : `RRULE:${body}`;
  return rrulestr(text, { dtstart }) as RRule;
}

/** Occurrences of a zone-less rule: rrule.js iterates the UTC instant directly
 *  (floating times read as UTC) — the historical behaviour, unchanged. */
function utcOccurrences(
  rruleBody: string,
  dtstart: Date,
  range: { start: Date; end: Date },
): Date[] {
  // `inc = true` makes the boundaries inclusive so an event starting exactly on
  // a boundary appears.
  return buildRule(rruleBody, dtstart).between(range.start, range.end, true);
}

/**
 * Occurrences of a zoned rule — DST-correct AND independent of the process's own
 * time zone. rrule.js's built-in `tzid` mode is neither (its output is offset by
 * the host's zone), so we iterate the rule purely in the event's WALL-CLOCK space
 * — rrule.js with no tzid treats the dtstart's UTC fields as the recurrence
 * anchor — then convert each wall-clock occurrence back to a real UTC instant in
 * `tzid` ourselves via `Intl`. Every emitted instant is real UTC, so day
 * bucketing and EXDATE / RECURRENCE-ID matching keep comparing real instants.
 */
function zonedOccurrences(
  rruleBody: string,
  dtstart: Date,
  tzid: string,
  range: { start: Date; end: Date },
  facts: SeriesEndFacts,
): Date[] {
  let dtstartWall: Date;
  try {
    dtstartWall = realToWall(dtstart, tzid); // probes the zone (throws if bad)
  } catch {
    // Unresolvable IANA zone (a typo, a Windows zone name, or a custom VTIMEZONE
    // id `Intl` can't load) — degrade to UTC expansion rather than dropping the
    // series. Worst case is the pre-fix behaviour, never worse.
    return utcOccurrences(untilOnClock(rruleBody, facts, null), dtstart, range);
  }
  // Iterate UNTIL in wall-clock space too, else a bounded series' final cutoff is
  // off by the zone offset.
  const rule = buildRule(untilOnClock(rruleBody, facts, tzid), dtstartWall);
  // Pad the wall-clock window a day each side (any zone offset is < 24h) so no
  // occurrence near a real-range edge is missed; the precise real filter trims.
  const lo = new Date(realToWall(range.start, tzid).getTime() - DAY_MS);
  const hi = new Date(realToWall(range.end, tzid).getTime() + DAY_MS);
  return rule
    .between(lo, hi, true)
    .map((wall) => wallToReal(wall, tzid))
    .filter((real) => real >= range.start && real <= range.end);
}

// `Intl.DateTimeFormat` construction is comparatively costly and we call it once
// per occurrence; memoise one formatter per zone. Constructing it throws
// `RangeError` for a zone Intl can't resolve, which is how `zonedOccurrences`
// detects a bad zone.
const zoneFormatters = new Map<string, Intl.DateTimeFormat>();
function zoneFormatter(tzid: string): Intl.DateTimeFormat {
  let f = zoneFormatters.get(tzid);
  if (!f) {
    f = new Intl.DateTimeFormat('en-US', {
      timeZone: tzid,
      hourCycle: 'h23',
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
    zoneFormatters.set(tzid, f);
  }
  return f;
}

/**
 * Offset in ms such that `wall-clock = instant + offset` for `tzid` at `instant`
 * (e.g. −4h for America/New_York in summer). Computed via `Intl` with an
 * explicit `timeZone`, so it never depends on the process's own zone.
 */
function zoneOffsetMs(instant: Date, tzid: string): number {
  const parts = zoneFormatter(tzid).formatToParts(instant);
  const get = (type: string): number =>
    Number(parts.find((p) => p.type === type)?.value);
  const asIfUtc = Date.UTC(
    get('year'),
    get('month') - 1,
    get('day'),
    get('hour') % 24, // some engines report midnight as 24 under h23
    get('minute'),
    get('second'),
  );
  return asIfUtc - instant.getTime();
}

/** Real UTC instant → a Date whose UTC fields hold its wall-clock in `tzid`. */
function realToWall(instant: Date, tzid: string): Date {
  return new Date(instant.getTime() + zoneOffsetMs(instant, tzid));
}

const DAY_MS = 86_400_000;

/** Inverse of {@link realToWall}: a wall-clock-as-UTC Date → the real instant in
 *  `tzid`, resolving DST edges deterministically. A spring-forward GAP time (no
 *  such reading on the local clock) rounds FORWARD to the first valid instant; a
 *  fall-back AMBIGUOUS time (two readings) takes the FIRST (earlier) instant. */
function wallToReal(wall: Date, tzid: string): Date {
  const t = wall.getTime();
  // Bracket any transition near the wall time: DST changes once per ~6 months
  // and at most once within a day, so the offsets a day before/after pin it.
  const offBefore = zoneOffsetMs(new Date(t - DAY_MS), tzid);
  const offAfter = zoneOffsetMs(new Date(t + DAY_MS), tzid);
  if (offBefore === offAfter) {
    return new Date(t - offBefore); // no transition in range → unambiguous
  }
  const candBefore = t - offBefore;
  const candAfter = t - offAfter;
  // A candidate is real iff its actual offset matches the side it came from.
  const beforeValid = zoneOffsetMs(new Date(candBefore), tzid) === offBefore;
  const afterValid = zoneOffsetMs(new Date(candAfter), tzid) === offAfter;
  if (beforeValid && afterValid) {
    return new Date(Math.min(candBefore, candAfter)); // overlap → first reading
  }
  if (beforeValid) return new Date(candBefore);
  if (afterValid) return new Date(candAfter);
  return new Date(Math.max(candBefore, candAfter)); // gap → round forward
}

/** What the core needs to read a series' end: its start, whether it is
 *  all-day, and the zone name it stores. */
export interface SeriesEndFacts {
  start: string;
  allDay: boolean;
  tzid: string | null;
}

/** The facts a stored series' end is read on. */
export function endFacts(series: {
  start: string;
  all_day?: boolean;
  recurrence?: { tzid?: string | null } | null;
}): SeriesEndFacts {
  return {
    start: series.start,
    allDay: series.all_day === true,
    tzid: series.recurrence?.tzid ?? null,
  };
}

/**
 * The core's reading of a series' end, asked once per series and clock: the
 * views expand every series on screen on every render, and the answer only
 * changes with the rule, the start, the zone or the device's zone. Kept small,
 * since an edited rule asks with every keystroke.
 */
const endReadings = new Map<string, SeriesEnd | null>();
function readEnd(rruleBody: string, facts: SeriesEndFacts, deviceZone: string | null): SeriesEnd | null {
  const key = JSON.stringify([rruleBody, facts.start, facts.allDay, facts.tzid, deviceZone]);
  const known = endReadings.get(key);
  if (known !== undefined) return known;
  const answer = seriesEnd(rruleBody, facts.start, facts.allDay, facts.tzid, deviceZone);
  const reading = answer?.end ?? null;
  if (endReadings.size >= 2000) endReadings.clear();
  endReadings.set(key, reading);
  return reading;
}

/** The last instant a series' end lets an occurrence start at, in ms: the
 *  core's `at` (`cal_core::read_series_end`). `null` without one. */
function endCap(rruleBody: string, facts: SeriesEndFacts, deviceZone: string | null): number | null {
  if (!/UNTIL=/i.test(rruleBody)) return null;
  const at = readEnd(rruleBody, facts, deviceZone)?.at;
  if (at === null || at === undefined) return null;
  const ms = Date.parse(at);
  return Number.isNaN(ms) ? null : ms;
}

/** `YYYY-MM-DDTHH:MM:SS`, with or without `Z`, as an RRULE's UTC date-time. */
function ruleDateTime(text: string): string {
  return `${text.replace(/[-:]/g, '').slice(0, 15)}Z`;
}

/**
 * How a series' end reads on the clock it repeats on: the core's answer
 * (`cal_core::read_series_end`), asked once per rule and facts. `deviceZone`
 * is read only for a series of days. `null` where the rule has no `UNTIL` or
 * cannot be read.
 */
export function seriesEndOf(
  rrule: string,
  facts: SeriesEndFacts,
  deviceZone: string | null,
): SeriesEnd | null {
  return readEnd(rrule.trim().replace(/^RRULE:/i, ''), facts, deviceZone);
}

/** The calendar a series is stored in, as far as how it stores an end goes. */
export interface EndsByDateCalendar {
  recurrence_capabilities?: { ends_by_date?: boolean } | null;
}

/**
 * Whether `calendar` stores a series' end as a date, counted whole on the
 * series' clock (manifest `recurrence.ends_by_date`; DESIGN-series-time-zone.md,
 * 140a). Shared code reads the capability only through this, never by the
 * adapter's name. A calendar not known reads `false`: the 105a reading.
 */
export function endsByDateOf(calendar: EndsByDateCalendar | null | undefined): boolean {
  return calendar?.recurrence_capabilities?.ends_by_date === true;
}

/** A rule's UNTIL as written, or `null`. */
export function untilTextOf(rrule: string): string | null {
  return /(?:^|[;:])\s*UNTIL=([^;]*)/i.exec(rrule)?.[1]?.trim() ?? null;
}

/** The rule with its UNTIL written as `value`; a leading `RRULE:` stays. */
function replacedUntil(rrule: string, value: string): string {
  return rrule.replace(/((?:^|[;:])\s*UNTIL=)[^;]*/i, `$1${value}`);
}

/** The rule without its UNTIL: it repeats on for as long as it is read. A
 *  leading `RRULE:` stays. */
export function ruleWithoutUntil(rrule: string): string {
  const [, prefix = '', body = ''] = /^(RRULE:)?(.*)$/is.exec(rrule.trim()) ?? [];
  return (
    prefix +
    body
      .split(';')
      .filter((part) => part.trim() !== '' && !/^\s*UNTIL=/i.test(part))
      .join(';')
  );
}

/** The rule ending by the UNTIL `value`: its own replaced, or added last
 *  where it names none. A leading `RRULE:` stays. */
export function ruleWithUntil(rrule: string, value: string): string {
  return untilTextOf(rrule) === null ? `${rrule};UNTIL=${value}` : replacedUntil(rrule, value);
}

/** `YYYY-MM-DD` from the first eight digits of an UNTIL, or `null`. */
function untilDigitsDay(until: string): string | null {
  const m = /^(\d{4})(\d{2})(\d{2})/.exec(until);
  return m ? `${m[1]}-${m[2]}-${m[3]}` : null;
}

/**
 * The UNTIL, as the rule stores it, that ends a series through `day`
 * (`YYYY-MM-DD`) on its clock (`cal_core::until_through`): `DT235959Z` where
 * that reads through the day on these facts, else the day's last instant on
 * the series' clock. `null` where the core gives no answer: facts that name
 * no instant, or a door that fails (a library older than the op).
 */
export function untilThroughText(day: string, rrule: string, facts: SeriesEndFacts): string | null {
  let at: string | undefined;
  try {
    at = untilThrough(day, rrule, facts.start, facts.allDay, facts.tzid)?.at;
  } catch {
    return null;
  }
  return at === undefined ? null : ruleDateTime(at);
}

/**
 * The UNTIL, as the rule stores it, that ends a series just before the
 * occurrence at `next` (`cal_core::cut_before`, 106a): one second before it,
 * two where one would spell `…T235959Z`, except in a rule that repeats by the
 * second. A cut Aperio writes so never needs the 105a guard (142a). Throws
 * where the core gives no answer: a cut has no other spelling.
 */
export function cutBeforeText(next: string, rrule: string): string {
  const at = cutBefore(next, rrule)?.at;
  if (at === undefined) throw new Error('cutBefore: no answer');
  return ruleDateTime(at);
}

/**
 * A stored rule ready to be counted, moved or carried to another calendar,
 * settled on the facts of the calendar it is read from
 * (DESIGN-series-time-zone.md, 140a; D1, D4).
 *
 * Only an end the core reads as an instant through the 105a guard alone
 * (`SeriesEnd.guarded`) changes. Where its own calendar stores an end as a
 * date (`endsByDate`), it ends through its own date and is written so
 * (`untilThroughText`); everywhere else it is the cut 105a reads it as, now
 * spelled `…T235958Z` (`cutBeforeText`), which every reader reads alike. Any
 * other rule comes back as it was. A leading `RRULE:` stays.
 */
export function carriedEnd(rrule: string, facts: SeriesEndFacts, endsByDate: boolean): string {
  const end = seriesEndOf(rrule, facts, null);
  if (end?.guarded !== true) return rrule;
  const until = untilTextOf(rrule);
  const day = until === null ? null : untilDigitsDay(until);
  if (until === null || day === null) return rrule;
  if (endsByDate) {
    const text = untilThroughText(day, rrule, facts);
    return text === null ? rrule : replacedUntil(rrule, text);
  }
  const at = untilInstantMs(until);
  if (!Number.isFinite(at)) return rrule;
  return replacedUntil(rrule, cutBeforeText(new Date(at + 1000).toISOString(), rrule));
}

/**
 * Whether `rrule` repeats at most once a day, the only rules whose end is a
 * day (DESIGN-series-time-zone.md, part 3 of stage 12): not hourly or faster,
 * and at most one value in each of BYHOUR, BYMINUTE and BYSECOND. The end of
 * any other rule is a moment, and the day state (`{D}T235959Z`) would stretch
 * it to the whole day: such a rule keeps its end as written.
 */
export function endIsADay(rrule: string): boolean {
  const freq = /(?:^|[;:])\s*FREQ=([A-Z]+)/i.exec(rrule)?.[1]?.toUpperCase();
  if (freq === 'HOURLY' || freq === 'MINUTELY' || freq === 'SECONDLY') return false;
  return ['BYHOUR', 'BYMINUTE', 'BYSECOND'].every((part) => {
    const value = new RegExp(`(?:^|[;:])\\s*${part}=([^;]*)`, 'i').exec(rrule)?.[1];
    return value === undefined || !value.includes(',');
  });
}

/**
 * The rule a new series is saved with (DESIGN-series-time-zone.md, 141a, D3).
 * A rule that is not a stored series' own keeps its end as the day the user
 * typed, `{D}T235959Z`, while the form is open; on save it is spelled through
 * that day on the facts being saved (`untilThroughText`). For rules in that
 * day state only: a stored series' end is read and written on its own facts
 * (`parseRRule`, `buildRRule`). All-day facts, no such end, or no answer from
 * the core leave the rule as it is.
 */
export function settledEnd(rrule: string | null, facts: SeriesEndFacts): string | null {
  if (rrule === null || facts.allDay || !endIsADay(rrule)) return rrule;
  const until = untilTextOf(rrule);
  if (until === null || !/^\d{8}T235959Z$/i.test(until)) return rrule;
  const day = untilDigitsDay(until);
  if (day === null) return rrule;
  const text = untilThroughText(day, rrule, facts);
  return text === null ? rrule : replacedUntil(rrule, text);
}

/**
 * A stored rule as a new form holds it (a prefill from another event): its
 * end settled on its source (`carriedEnd`), then turned into the day it
 * reaches on the source's clock, in day state, `{D}T235959Z`, which the save
 * spells on the new event's facts (`settledEnd`). All-day rules, rules
 * without an UNTIL, rules with a COUNT and rules whose end is a moment, not a
 * day (`endIsADay`), stay as they are; an end the core gives no day for stays
 * as carried.
 */
export function asTypedEnd(
  rrule: string | null,
  facts: SeriesEndFacts,
  endsByDate: boolean,
): string | null {
  if (rrule === null || facts.allDay) return rrule;
  // A rule that repeats more than once a day keeps a moment as its end; one
  // the 105a guard alone reads as a cut is settled on its source first, so
  // the text keeps its meaning when the start or the zone changes later.
  if (!endIsADay(rrule)) return carriedEnd(rrule, facts, endsByDate);
  if (untilTextOf(rrule) === null || /(?:^|[;:])\s*COUNT=/i.test(rrule)) return rrule;
  const carried = carriedEnd(rrule, facts, endsByDate);
  const day = seriesEndOf(carried, facts, null)?.endDay;
  return day ? replacedUntil(carried, `${day.replace(/-/g, '')}T235959Z`) : carried;
}

/**
 * The rule an editor holds for a series read from a file (136a; DESIGN-
 * series-time-zone.md, stage 12, part 3): its end read ONCE as a day on the
 * file's own facts — its start, its kind and its zone — when the editor is
 * filled (`asTypedEnd`). A file stores no end as a date, so an end the 105a
 * guard alone reads as a cut counts as the day before it. From then on the end
 * is the typed day, `{D}T235959Z`, as a new series' (141a, D3), and the save
 * spells it on the facts it saves (`settledEnd`): a time or zone changed in
 * the editor keeps the last day.
 */
export function importedRule(file: {
  start: string;
  all_day?: boolean;
  recurrence?: { rrule?: string | null; tzid?: string | null } | null;
}): string | null {
  return asTypedEnd(file.recurrence?.rrule ?? null, endFacts(file), false);
}

/**
 * The rule with its UNTIL as the bound the views iterate it against, on the
 * clock they expand the series on: `zoneInUse`, or UTC for `null`.
 *
 * How an end reads is the core's rule (`cal_core::read_series_end`,
 * DESIGN-series-time-zone.md, decisions 101b and 105a to 116a), read on the
 * clock the series repeats on: its zone, UTC, or for a series of days the
 * device's days, by its digits (201).
 * - Through a date (`…T235959Z` on a zone's clock that no cut wrote, a date)
 *   and a wall time (a date-time without `Z`, a series of days' digits) go
 *   in as that wall time: the views iterate in the clock's wall time. "Until
 *   3 May" on a Berlin series at 00:30 no longer reaches 4 May 00:30, and one
 *   in New York at 20:00 keeps its 3 May.
 * - An instant goes in as its wall time on this device's reading of the zone,
 *   the reading every occurrence gets, as before.
 * - Where this device's `Intl` cannot use a zone the core reads, the series
 *   expands on UTC, and the bound goes in as the instant the core names.
 * An end the core cannot read stays as written, for rrule.js to judge. The
 * reminders read the same answer (host-core `expand_occurrences`); both are
 * pinned by `shared/contracts/eventOccurrences.json`.
 */
function untilOnClock(rruleBody: string, facts: SeriesEndFacts, zoneInUse: string | null): string {
  if (!/UNTIL=/i.test(rruleBody)) return rruleBody;
  const clock = expansionClock(facts.allDay, facts.tzid);
  // A series of days reads its digits on the days the views expand it on.
  const end = readEnd(rruleBody, facts, clock === 'device-days' ? zoneInUse : null);
  if (end === null) return rruleBody;
  let bound: string | null;
  if (end.reading === 'instant' && end.at !== null) {
    bound =
      zoneInUse === null ? ruleDateTime(end.at) : ruleDateTime(realToWall(new Date(end.at), zoneInUse).toISOString());
  } else if (clock === 'zone' && zoneInUse === null) {
    bound = end.at === null ? null : ruleDateTime(end.at);
  } else {
    bound = ruleDateTime(end.wall);
  }
  if (bound === null) return rruleBody;
  return rruleBody.replace(/UNTIL=[^;]*/i, `UNTIL=${bound}`);
}

/** A zone the core accepted, if THIS device's `Intl` can also use it; `null`
 *  otherwise, which expands in UTC rather than dropping the series. The
 *  formatter it probes with is the memoised one every occurrence then uses. */
function usableZone(zone: string | null): string | null {
  if (!zone) return null;
  try {
    zoneFormatter(zone);
    return zone;
  } catch {
    return null;
  }
}

/** Whether this device's `Intl` can use `zone`: the check the views make
 *  before they expand a series on it. A series on a zone that fails it expands
 *  on UTC, so a form shows its fields on UTC too (`seriesFieldClock`). */
export function zoneUsableHere(zone: string): boolean {
  return usableZone(zone) !== null;
}

/** An instant on the clock a series recurs in, as a Date whose UTC fields hold
 *  that clock's reading. */
function seriesWall(
  instant: Date,
  tzid: string | null | undefined,
  allDay: boolean,
): Date {
  const zone = clockZone(allDay, tzid);
  return zone ? realToWall(instant, zone) : instant;
}

/**
 * The `YYYY-MM-DD` day `iso` falls on in the clock a series recurs in: its
 * zone, the DEVICE's days for an all-day series (48a), or UTC (see
 * {@link expandEvent}). A rule's weekdays and days of the month are read
 * against this day, which for a zoned series can differ from the day the
 * device shows.
 *
 * `allDay` is the series' own flag, and it decides: an all-day series carries
 * no zone, so without it every all-day day would be read on UTC — the reading
 * that put a named weekday a day late east of Greenwich.
 */
export function seriesDayKey(
  iso: string,
  tzid: string | null | undefined,
  allDay: boolean,
): string {
  return seriesWall(new Date(iso), tzid, allDay).toISOString().slice(0, 10);
}

/**
 * `iso` moved by `days` whole days on the clock a series recurs in, and placed
 * at `timeOf`'s time of day on that clock when given. This is how each instant
 * of a series (its start, its exceptions) moves when the whole series moves, so
 * the exceptions still meet the occurrences they cancel.
 */
export function moveSeriesInstant(
  iso: string,
  tzid: string | null | undefined,
  allDay: boolean,
  days: number,
  timeOf?: string,
): string {
  const zone = clockZone(allDay, tzid);
  let moved = seriesWall(new Date(iso), tzid, allDay).getTime() + days * DAY_MS;
  if (timeOf !== undefined) {
    const timeOfDay = (ms: number) => ((ms % DAY_MS) + DAY_MS) % DAY_MS;
    moved +=
      timeOfDay(seriesWall(new Date(timeOf), tzid, allDay).getTime()) - timeOfDay(moved);
  }
  return (zone ? wallToReal(new Date(moved), zone) : new Date(moved)).toISOString();
}

/**
 * A rule's UTC `UNTIL`, moved on the series' clock the way its start moved from
 * `from` to `to`, written `YYYYMMDDTHHMMSSZ` as the rule stores it; `undefined`
 * when the rule has no UTC date-time `UNTIL`.
 *
 * A timed series' bound is an instant. Moved by whole UTC days it slides an
 * hour against the occurrences across a clock change, and left in place while
 * the time of day changes, the last occurrence drops past it or a cut one
 * comes back. A series of days reads its bound by its digits, on its day clock
 * (decision 201), so the digits move as its days do: moved as an instant
 * across a clock change, they landed an hour into the next day, and the
 * series gained one.
 *
 * The end it gives needs no guard to be read (142a): an end through a day is
 * spelled by the core (`untilThroughText`), a moved cut never spells
 * `…T235959Z` (`cutBeforeText`, D5). An end that reads as a cut only through
 * the 105a guard is settled by the caller first, on the facts of the calendar
 * it is read from (`carriedEnd`, D4); moved as it stands, it would be moved
 * as a cut on a calendar where it ends through its own date.
 */
export function movedSeriesUntil(
  rrule: string,
  tzid: string | null | undefined,
  allDay: boolean,
  from: string,
  to: string,
): string | undefined {
  const match = /(?:^|;)\s*UNTIL=(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z\s*(?:;|$)/i.exec(rrule);
  if (!match) return undefined;
  // `…T235959Z` that reads as "through that date" on the series' clock
  // (101b) is a date, not an instant: the core moves it by whole days and
  // keeps its spelling, so the series keeps ending on the day it moved to.
  // Moved as an instant across a clock change, it would no longer read so.
  // Only where the moved series' time of day would make that spelling read
  // as a cut (105a: New York at 19:00 in winter) is the day's end written as
  // its instant on the series' clock instead, which reads the same day.
  if (!allDay) {
    const end = readEnd(rrule, { start: from, allDay, tzid: tzid ?? null }, null);
    if (end?.reading === 'throughDay') {
      const days = Math.round(
        (Date.parse(seriesDayKey(to, tzid, allDay)) - Date.parse(seriesDayKey(from, tzid, allDay))) /
          DAY_MS,
      );
      const day = new Date(Date.parse(end.wall.slice(0, 10)) + days * DAY_MS).toISOString().slice(0, 10);
      const text = untilThroughText(day, rrule, { start: to, allDay, tzid: tzid ?? null });
      if (text === null || text === `${day.replace(/-/g, '')}T235959Z`) return undefined;
      return text;
    }
  }
  const [, y, mo, d, h, mi, s] = match.map(Number);
  const until = Date.UTC(y, mo - 1, d, h, mi, s);
  if (Number.isNaN(until)) return undefined;
  const zone = clockZone(allDay, tzid);
  const wall = (ms: number) => (zone ? realToWall(new Date(ms), zone) : new Date(ms)).getTime();
  const shift = wall(Date.parse(to)) - wall(Date.parse(from));
  const byDigits = expansionClock(allDay, tzid) === 'device-days';
  let untilWall = byDigits ? until : wall(until);
  if (!byDigits) {
    // An end that does not reach its own day (a cut) moves as "before that
    // day's first occurrence". Its wall time alone can sit after the
    // occurrence's nominal time: a cut at an occurrence the spring change
    // pushes past its gap ends at 03:29:59 for an occurrence at 02:30, and
    // moved onto a day without the change that bound let 02:30 back in.
    const end = readEnd(rrule, { start: from, allDay, tzid: tzid ?? null }, null);
    const untilDay = new Date(untilWall).toISOString().slice(0, 10);
    if (end !== null && end.endDay < untilDay) {
      const timeOfDay = ((wall(Date.parse(from)) % DAY_MS) + DAY_MS) % DAY_MS;
      untilWall = Math.min(untilWall, Date.parse(`${untilDay}T00:00:00Z`) + timeOfDay - 1000);
    }
  }
  const movedWall = untilWall + shift;
  const moved = zone && !byDigits ? wallToReal(new Date(movedWall), zone) : new Date(movedWall);
  // On a zone's clock a moved instant never spells the end of a UTC day
  // (106a, D5): the 105a guard would have to read it, and only reads it right
  // where it is a cut. On UTC that spelling is the end of the day it names,
  // read alike with or without the guard, and the digits of a series of days
  // are its date: both stay as moved.
  if (expansionClock(allDay, tzid) !== 'zone') {
    return moved.toISOString().replace(/[-:]/g, '').replace(/\.\d{3}/, '');
  }
  return cutBeforeText(new Date(moved.getTime() + 1000).toISOString(), rrule);
}

/**
 * The host's current IANA time zone as the runtime spells it (e.g.
 * `America/New_York`, or `Asia/Calcutta` from V8), or `null` when it reports
 * none, a UTC name, or a name tzdata does not know (`+00:00`, `Etc/Unknown`):
 * the same rule a stored zone is read by, so a series stamped here repeats
 * where the views and the reminders will read it.
 */
export function localTimeZone(): string | null {
  let tz: string | undefined;
  // Only the runtime's read is guarded. The rule below throws when its door is
  // not installed, and swallowing that would quietly stop every stamp.
  try {
    // The formatter is memoised, its ANSWER is not: building one costs real
    // time and every all-day series on screen asks (48a), while a device that
    // moves must still be able to say so.
    deviceFormatter ??= new Intl.DateTimeFormat();
    tz = deviceFormatter.resolvedOptions().timeZone;
  } catch {
    return null;
  }
  return seriesClockZone(tz);
}

/** The formatter {@link localTimeZone} reads the device's zone from. */
let deviceFormatter: Intl.DateTimeFormat | null = null;

/**
 * The device's zone name exactly as the runtime reports it (`Asia/Calcutta`,
 * `Europe/Kiev`, `UTC`, `+00:00`), or `null` when it reports none. The zone
 * list reads it so (decision 118a): the core resolves the name itself, and
 * a pick of the device's own zone is written in this spelling where the
 * device cannot use tzdata's (117a). {@link localTimeZone} is the same name
 * read as a series clock.
 */
export function deviceZoneName(): string | null {
  try {
    deviceFormatter ??= new Intl.DateTimeFormat();
    return deviceFormatter.resolvedOptions().timeZone || null;
  } catch {
    return null;
  }
}

/** The listed zones this device's clock library refuses, read once. */
let refusedListed: readonly string[] | null = null;

/**
 * The names this device's clock library refuses, for the zone list's groups
 * (`unusable`, 127a): every listed zone it refuses, read once per session,
 * and the series' stored name in its own spelling when it refuses that (a
 * group keeps the stored zone as it is spelled, 120a). `listed` is the zone
 * list's names by position.
 */
export function unusableZones(listed: readonly string[], stored: string | null): string[] {
  refusedListed ??= Object.freeze(listed.filter((zone) => !zoneUsableHere(zone)));
  const refused = [...refusedListed];
  if (stored !== null && stored !== '' && !zoneUsableHere(stored) && !refused.includes(stored)) {
    refused.push(stored);
  }
  return refused;
}

/**
 * Stamp the host's local zone onto a freshly-created TIMED recurring rule so it
 * expands DST-correctly — a series created here at 19:00 keeps 19:00 across DST,
 * the same guarantee a zoned CalDAV series gets. Leaves all-day rules (they use
 * the date-based path), already-zoned rules, and non-recurring events untouched.
 * Called when a series is created, and by {@link editedRecurrence} when an
 * existing event becomes a series in the editor.
 */
export function withCreatedRecurrenceZone<
  R extends { rrule: string; exceptions: string[]; tzid?: string | null },
>(recurrence: R | null, allDay: boolean): R | null {
  if (!recurrence || allDay || recurrence.tzid) {
    return recurrence;
  }
  const tz = localTimeZone();
  return tz ? { ...recurrence, tzid: tz } : recurrence;
}

/**
 * The recurrence an editor saves: the rule from the form, with the exceptions
 * and the zone of the series it edits.
 *
 * The editors hold only the rule text, and rebuilding `{rrule, exceptions}`
 * from it dropped the zone. Every writer stores what it is handed — the local
 * store left `rrule_tzid` empty, and CalDAV, Google, Graph and EWS wrote the
 * start in UTC — so a series edited as a whole slid an hour at the next clock
 * change, in the views, in its reminders and in every other client.
 *
 * A series that has a zone keeps it. An event that had no rule and gets one
 * here is a NEW series whose rule was just built from the local form, so it
 * gets the device's zone, as a created series does
 * ({@link withCreatedRecurrenceZone}); it is saved through the update path,
 * where nothing else stamps one. A series that already recurs without a zone
 * stays as it is: a lost zone and a series meant to run in UTC look the same
 * here (Google's `Etc/UTC` and a CalDAV `Z` start both arrive without one), and
 * stamping the second would move its occurrences — to another weekday for a
 * late-evening rule.
 */
export function editedRecurrence(
  rrule: string | null | undefined,
  previous: { exceptions: string[]; tzid?: string | null } | null | undefined,
  allDay: boolean,
): { rrule: string; exceptions: string[]; tzid?: string | null } | null {
  if (!rrule) {
    return null;
  }
  if (!previous) {
    return withCreatedRecurrenceZone({ rrule, exceptions: [] }, allDay);
  }
  const { exceptions } = previous;
  return previous.tzid ? { rrule, exceptions, tzid: previous.tzid } : { rrule, exceptions };
}

/** The local calendar day of an instant, counted in days. */
function localDayNumber(iso: string): number {
  const d = new Date(iso);
  return Math.round(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()) / DAY_MS);
}

/**
 * The start and end a whole series gets from an edit made on the fields of one
 * of its occurrences.
 *
 * A whole-series edit opens the series itself, with its own start. A row of a
 * series can still reach an editor whose fields were filled from that
 * occurrence (the scope control inside the editor), and writing those fields as
 * they are moved the series start to the occurrence: the earlier occurrences
 * disappeared. So the edit is read as a change. The days the date moved move
 * the series start by as many days, a new time of day becomes the series' time
 * of day, and the length is the edited one; both are counted on the series'
 * clock (`tzid`, or UTC without one). Untouched fields leave the series exactly
 * where it was.
 *
 * Whether the time changed, and how many days an untouched time moved, are
 * read on `fieldClock`, the clock the form's fields stand on: the device's
 * for the scope picker inside the form, the series' own where the editor puts
 * the fields there (stage 12 of DESIGN-series-time-zone.md). A time typed on
 * that clock is a new time even where the device reads it as before.
 */
export function seriesTimesFromOccurrenceEdit(
  series: { start: string; end: string },
  tzid: string | null | undefined,
  occurrence: { start: string; end: string },
  edited: { start: string; end: string },
  allDay: boolean,
  fieldClock: FieldClock = { kind: 'device' },
): { start: string; end: string } {
  const same = (a: string, b: string) => Date.parse(a) === Date.parse(b);
  if (same(edited.start, occurrence.start) && same(edited.end, occurrence.end)) {
    return { start: series.start, end: series.end };
  }
  if (allDay) {
    // An all-day series is a run of local days.
    const start = new Date(series.start);
    start.setDate(start.getDate() + localDayNumber(edited.start) - localDayNumber(occurrence.start));
    start.setHours(0, 0, 0, 0);
    const end = new Date(start);
    end.setDate(end.getDate() + localDayNumber(edited.end) - localDayNumber(edited.start));
    return { start: start.toISOString(), end: end.toISOString() };
  }
  // The time changed when its reading on the fields' clock did. Read on
  // another clock, an untouched time on a date past a clock change there would
  // read as a new time for every occurrence, and a typed time that clock reads
  // as before would be lost.
  const onFields = (iso: string): { day: number; minutes: number } => {
    if (fieldClock.kind === 'device') {
      const d = new Date(iso);
      return { day: localDayNumber(iso), minutes: d.getHours() * 60 + d.getMinutes() };
    }
    const wall = wallOf(iso, fieldClock.tzid);
    const [hours, minutes] = (wall?.time ?? '00:00').split(':').map(Number);
    return {
      day: Math.round(Date.parse(wall?.date ?? iso.slice(0, 10)) / DAY_MS),
      minutes: hours * 60 + minutes,
    };
  };
  const editedOn = onFields(edited.start);
  const occurrenceOn = onFields(occurrence.start);
  const timeChanged = editedOn.minutes !== occurrenceOn.minutes;
  // An untouched time moves by the dates the user changed, counted on the
  // fields' calendar: on another clock a date moved past a clock change near
  // midnight would count a day too many. A new time counts on the series'
  // clock, where it may land on another day.
  // Past the all-day branch above, this series has a time of day.
  const dayOf = (iso: string) => Date.parse(seriesDayKey(iso, tzid, false)) / DAY_MS;
  const days = timeChanged
    ? Math.round(dayOf(edited.start) - dayOf(occurrence.start))
    : editedOn.day - occurrenceOn.day;
  const start = moveSeriesInstant(
    series.start,
    tzid,
    false,
    days,
    timeChanged ? edited.start : undefined,
  );
  const end = new Date(Date.parse(start) + Date.parse(edited.end) - Date.parse(edited.start));
  return { start, end: end.toISOString() };
}

/**
 * The exceptions of a series, moved to its new time of day when an edit of the
 * whole series changed it.
 *
 * An exception names the instant of the occurrence it cancels. When the series
 * moves to another time the old instants cancel nothing, and the cancelled
 * occurrences came back at the new time. Each exception takes the new time, on
 * the day the occurrence it cancels moves to. All-day series are left alone.
 */
export function exceptionsAtSeriesTime<
  R extends { rrule: string; exceptions: string[]; tzid?: string | null },
>(
  recurrence: R | null,
  previousStart: string,
  start: string,
  allDay: boolean,
): R | null {
  if (!recurrence || allDay || recurrence.exceptions.length === 0) {
    return recurrence;
  }
  const { tzid } = recurrence;
  if (
    moveSeriesInstant(previousStart, tzid, false, 0, start) ===
    moveSeriesInstant(previousStart, tzid, false, 0)
  ) {
    return recurrence;
  }
  // A new time can land on another day on the series' clock while the device
  // shows the same date. When the rule's day follows the start, its occurrences
  // move to that day, so the exceptions do too; a date the user changed moves
  // only the series start. A rule that names its days or months (BYDAY,
  // BYMONTHDAY, …) keeps them, because the editors write it back as it is, so
  // its exceptions stay on their day and still meet its occurrences.
  const dayOf = (iso: string) => Date.parse(seriesDayKey(iso, tzid, false)) / DAY_MS;
  const daysFollowStart = !/(?:^|[;:])\s*BY(?:DAY|MONTHDAY|YEARDAY|WEEKNO|SETPOS|MONTH)\s*=/i.test(
    recurrence.rrule,
  );
  const days = daysFollowStart
    ? Math.round(dayOf(start) - dayOf(previousStart)) -
      (localDayNumber(start) - localDayNumber(previousStart))
    : 0;
  return {
    ...recurrence,
    exceptions: recurrence.exceptions.map((iso) =>
      moveSeriesInstant(iso, tzid, false, days, start),
    ),
  };
}

/**
 * Marker in an override instance's id, separating the recurring series'
 * `{href}|{uid}` from the RECURRENCE-ID instant it replaces (e.g.
 * `…|uid::rid::2026-06-14T13:00:00Z`). Mirrors `RECURRENCE_ID_MARKER` in the
 * CalDAV adapter's iCalendar mapping (`crates/ical-mapping/src/mapping.rs`),
 * which mints these ids — keep the two in sync.
 */
const RECURRENCE_ID_MARKER = '::rid::';

/**
 * The original occurrence instant (ISO) a RECURRENCE-ID override replaces, or
 * `null` for a master / plain event. Read off the id the CalDAV adapter minted.
 */
export function overrideRecurrenceIso<E extends RecurringEventLike>(
  event: E,
): string | null {
  const i = event.id.indexOf(RECURRENCE_ID_MARKER);
  return i < 0 ? null : event.id.slice(i + RECURRENCE_ID_MARKER.length);
}

/**
 * The id that ADDRESSES one occurrence of `seriesId` as an exception — the
 * `{series}::rid::{slot}` shape every adapter that can hold one understands
 * (`cal_core::split_override_id`, decision 79b).
 *
 * This is a REQUEST: the surfaces mint it to say which occurrence they mean,
 * and the adapter answers with the id of its own making, in its own spelling.
 * The slot is written to the second, as the adapters write it, because an id
 * is read by people too — the instant itself is what is compared, never the
 * text.
 */
export function overrideIdFor(seriesId: string, occurrenceIso: string): string {
  const at = new Date(occurrenceIso);
  const slot = Number.isNaN(at.getTime())
    ? occurrenceIso
    : at.toISOString().replace(/\.\d{3}Z$/, 'Z');
  return `${seriesId}${RECURRENCE_ID_MARKER}${slot}`;
}

/**
 * The series (master) id a RECURRENCE-ID override belongs to, or `null` for a
 * master / plain event.
 */
export function overrideSeriesId<E extends RecurringEventLike>(
  event: E,
): string | null {
  const i = event.id.indexOf(RECURRENCE_ID_MARKER);
  return i < 0 ? null : event.id.slice(0, i);
}

/**
 * Walk events through {@link expandEvent}, flatten, and sort chronologically.
 * The result is `E[]` (occurrences are assignment-compatible with `E`); callers
 * that need the underlying series read `series_id` via {@link seriesIdOf}.
 *
 * RECURRENCE-ID overrides (a recurring series' modified single instances) arrive
 * as separate non-recurring events whose id carries both the master id and the
 * occurrence they replace. We drop the master's own copy of each overridden
 * occurrence so the override stands in for it (at its possibly-moved time) —
 * otherwise the day shows the instance twice, or (before the adapter fix that
 * stopped them colliding in the cache) not at all.
 */
export function expandAll<E extends RecurringEventLike>(
  events: E[],
  range: { start: Date; end: Date },
): E[] {
  // Per series, the instants of the occurrences an override supersedes. A
  // timed series matches its slot exactly, as it always did; a series of DAYS
  // matches the occurrence the slot NAMES (within half a day, 48a and decision
  // 95), because the instant a slot was minted at is only one spelling of that
  // day and a provider — or a build before 48a — may have written another.
  let overridden: Map<string, number[]> | null = null;
  for (const ev of events) {
    const iso = overrideRecurrenceIso(ev);
    const seriesId = overrideSeriesId(ev);
    if (iso == null || seriesId == null) continue;
    const at = new Date(iso).getTime();
    if (Number.isNaN(at)) continue;
    (overridden ??= new Map());
    const slots = overridden.get(seriesId);
    if (slots) slots.push(at);
    else overridden.set(seriesId, [at]);
  }

  const out = events.flatMap((ev) => {
    // A CANCELLED RECURRENCE-ID override is a deletion tombstone: it exists only to
    // suppress the master's occurrence (already recorded in `overridden` above) and
    // carries no content of its own, so never emit it as a visible event — the
    // deleted occurrence must VANISH, not linger as an empty cancelled row. This is
    // independent of the show-cancelled-events toggle, which governs whole cancelled
    // events (those keep a normal id, no `::rid::`). A MODIFIED (non-cancelled)
    // override still renders at its moved time.
    if (ev.cancelled && overrideRecurrenceIso(ev) != null) return [];
    const occs = expandEvent(ev, range);
    const replaced = ev.recurrence?.rrule ? overridden?.get(ev.id) : undefined;
    if (!replaced || replaced.length === 0) return occs;
    // Drop the master occurrences an override stands in for (matched on the
    // original occurrence instant). Keep anything we can't place — never hide.
    const same = slotMatcher(ev);
    return occs.filter((o) => {
      const iso = occurrenceIsoOf(o);
      if (iso == null) return true;
      const at = new Date(iso).getTime();
      return !replaced.some((slot) => same(slot, at));
    });
  });
  // `start` is an ISO instant — a machine string, not text.
  out.sort((a, b) => compareMachineStrings(a.start, b.start));
  return out;
}

/** Type guard: a synthetic occurrence vs a regular/master event. */
export function isExpandedOccurrence<E extends RecurringEventLike>(
  event: E,
): event is ExpandedOccurrence<E> {
  return (
    'series_id' in event &&
    typeof (event as ExpandedOccurrence<E>).series_id === 'string'
  );
}

/**
 * Underlying series id for an event row: the master's `series_id` for an
 * expanded occurrence, else `event.id`. Keying off `series_id` (not
 * `id.split('@')[0]`) is the canonical fix — Aperio CalDAV UIDs themselves
 * contain `@aperio`, so the split shortcut dropped half the master UID.
 */
export function seriesIdOf<E extends RecurringEventLike>(event: E): string {
  if (isExpandedOccurrence(event)) return event.series_id;
  // An OVERRIDE — a real row the provider sent for one modified occurrence —
  // is just as much part of a series, and its id carries the master's in front
  // of the marker. This used to return the override's own id, so "the whole
  // series" acted on the single row the user had opened and left the series
  // untouched.
  return overrideSeriesId(event) ?? event.id;
}

/**
 * Whether a row is ONE occurrence of a recurring series — expanded by us from
 * a master, or sent by the provider as a modified occurrence.
 *
 * The distinction that matters to a user is "does acting on this need to ask
 * about scope", and both shapes answer yes. Only one of them satisfies
 * {@link isExpandedOccurrence}, which is a type guard for the synthetic shape
 * and stays narrow: an override has no `occurrence_start` field, so widening
 * the guard would let code read one that is not there.
 *
 * Every surface that opens a scope prompt, or that must not silently act on a
 * whole series, asks THIS.
 */
export function isSeriesOccurrence<E extends RecurringEventLike>(
  event: E,
): boolean {
  return isExpandedOccurrence(event) || overrideRecurrenceIso(event) != null;
}

/**
 * Whether a row is a provider override: a real row the provider sent for one
 * modified occurrence (its id names the series and the slot it replaces), not
 * an occurrence we expanded from a master.
 *
 * Editing "just this one" on such a row updates the row itself, by its own id.
 * There is nothing to carve out of the series — it already skips that slot —
 * and carving would delete the provider's exception and leave a detached copy.
 * Desktop and phone both ask this, so the two editors decide the same way.
 */
export function isProviderOverride<E extends RecurringEventLike>(
  event: E,
): boolean {
  return !isExpandedOccurrence(event) && overrideRecurrenceIso(event) != null;
}

/**
 * Occurrence-start ISO for an expanded occurrence, else `null` for a master.
 * Drives "delete only this occurrence" (append onto the master's EXDATE).
 */
export function occurrenceIsoOf<E extends RecurringEventLike>(
  event: E,
): string | null {
  if (isExpandedOccurrence(event)) return event.occurrence_start;
  // An override's instant is the RRULE slot it REPLACES, which is what an
  // EXDATE has to name — not where the user moved it to. Returning `null` here
  // is what made "delete just this one" bail out silently on an edited
  // occurrence: the caller reads the instant first and returns when it is
  // absent.
  return overrideRecurrenceIso(event);
}

/** UTC "basic" RFC-5545 timestamp (`YYYYMMDDTHHMMSSZ`) — the form an RRULE
 *  `UNTIL` takes when the series has a zoned/timed DTSTART. */
function formatRRuleUntilUtc(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return (
    `${d.getUTCFullYear()}${p(d.getUTCMonth() + 1)}${p(d.getUTCDate())}` +
    `T${p(d.getUTCHours())}${p(d.getUTCMinutes())}${p(d.getUTCSeconds())}Z`
  );
}

/** Date-only RFC-5545 value (`YYYYMMDD`) — the form an RRULE `UNTIL` MUST take
 *  when the series has a DATE-valued (all-day) DTSTART. Per RFC-5545 §3.3.10 the
 *  UNTIL value type must match DTSTART's, so a datetime UNTIL on an all-day
 *  series is malformed and strict providers (iCloud CalDAV) may reject the write
 *  or silently drop the RRULE. */
function formatRRuleUntilDate(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  // The LOCAL day, not the UTC one. An all-day event is stored as local
  // midnight expressed as an instant, so the moment one second before an
  // all-day cutoff is 23:59:59 of the previous LOCAL day — and west of
  // Greenwich that instant still falls on the cutoff's UTC day. Read in UTC it
  // therefore named the cutoff day itself, and a date-only UNTIL is INCLUSIVE:
  // the occurrence the user split away stayed in the truncated series, so it
  // existed twice, once in each half. East of Greenwich the two readings agree,
  // which is why it went unseen.
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}`;
}

/** Parse an RRULE `UNTIL` value (`YYYYMMDD` or `YYYYMMDDTHHMMSSZ`) to an epoch
 *  instant in ms for chronological comparison. A date-only value is inclusive of
 *  the whole day, so it maps to that day's last instant — this is what lets a
 *  date-only and a datetime bound compare correctly (a naive lexicographic
 *  compare mis-ranks same-date values because the date-only string is a prefix of
 *  the datetime form). Returns NaN for an unparseable value. */
function untilInstantMs(until: string): number {
  const m = /^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})Z?)?$/.exec(
    until.trim(),
  );
  if (!m) return Number.NaN;
  const [, y, mo, d, hh, mm, ss] = m;
  if (hh == null) {
    return Date.UTC(+y, +mo - 1, +d, 23, 59, 59, 999);
  }
  return Date.UTC(+y, +mo - 1, +d, +hh, +mm, +ss);
}

/**
 * Truncate a recurrence RULE so it keeps only occurrences STRICTLY BEFORE
 * `cutoff` — the "this and all following occurrences" split point.
 *
 * Sets `UNTIL` to one second before `cutoff` (RFC-5545 `UNTIL` is inclusive) and
 * drops any `COUNT` (a rule can't carry both). An existing earlier `UNTIL` wins
 * (we never EXTEND a series). `cutoff` is the target occurrence's UTC instant, so
 * the comparison is zone-independent: the occurrence at `cutoff` and everything
 * after it fall away, everything before stays.
 *
 * `opts.allDay` picks the emitted UNTIL value type: an all-day (DATE-valued)
 * series gets a date-only `YYYYMMDD` UNTIL, a timed series the datetime
 * `YYYYMMDDTHHMMSSZ` form — the value type MUST match DTSTART's or strict
 * providers drop the rule.
 *
 * Returns the rule body without a leading `RRULE:` (matching how the app stores
 * recurrence rules).
 */
export function truncateRRuleBefore(
  rrule: string,
  cutoff: Date,
  opts: { allDay?: boolean; tzid?: string | null; start?: string } = {},
): string {
  const lastKept = new Date(cutoff.getTime() - 1000);
  const body = rrule.trim().replace(/^RRULE:/i, '');
  const kept: string[] = [];
  let existingUntil: string | null = null;
  for (const part of body.split(';')) {
    if (!part) continue;
    const eq = part.indexOf('=');
    const key = part.slice(0, eq).toUpperCase();
    if (key === 'COUNT') continue;
    if (key === 'UNTIL') {
      existingUntil = part.slice(eq + 1);
      continue;
    }
    kept.push(part);
  }
  // Keep whichever ends the series sooner. With the series' start at hand the
  // existing end reads as the views and the reminders read it
  // (`cal_core::read_series_end`, 101b): `…T235959Z` on a zone's clock is the
  // end of that day there, so a New York series at 20:00 cut on its last day
  // ends before it, where the end of the UTC day let it stand twice. An end
  // that wins goes back as it was written (an all-day series' as its date, the
  // value type its start has); re-spelled through an instant, a date-only end
  // became a UTC end of day and an all-day one moved to the next day east of
  // Greenwich. Without the start, the end is compared on its UTC reading, as
  // before.
  let boundMs = lastKept.getTime();
  let keptEnd: string | null = null;
  if (existingUntil) {
    const facts = { start: opts.start ?? '', allDay: opts.allDay === true, tzid: opts.tzid ?? null };
    const read =
      opts.start === undefined
        ? null
        : readEnd(body, facts, facts.allDay ? usableZone(localTimeZone()) : null);
    const ex = read?.at != null ? Date.parse(read.at) : untilInstantMs(existingUntil);
    if (Number.isFinite(ex) && ex < boundMs) {
      boundMs = ex;
      if (read !== null) keptEnd = existingUntil;
    }
  }
  if (keptEnd !== null) {
    kept.push(`UNTIL=${opts.allDay ? keptEnd.slice(0, 8) : keptEnd}`);
    return kept.join(';');
  }
  const bound = new Date(boundMs);
  let finalUntil = opts.allDay
    ? formatRRuleUntilDate(bound)
    : formatRRuleUntilUtc(bound);
  // A cut never spells the end of a UTC day. `…T235959Z` reads as "through
  // that date" (DESIGN-series-time-zone.md, decision 101b), and one second
  // before an occurrence at 00:00 UTC (New York at 19:00, in summer at
  // 20:00) is exactly that spelling. The core spells the cut (106a,
  // `cal_core::cut_before`): two seconds before it then, which keeps the same
  // occurrences, except in a rule that repeats by the second.
  if (!opts.allDay && boundMs === lastKept.getTime()) {
    finalUntil = cutBeforeText(cutoff.toISOString(), body);
  }
  kept.push(`UNTIL=${finalUntil}`);
  return kept.join(';');
}

/**
 * Split a recurrence rule for "edit this and all following occurrences": the
 * ORIGINAL series is truncated to end just before `cutoff`, and a NEW series
 * takes over from `cutoff` (created with the edited fields by the caller).
 *
 * Returns `{ oldRule, newRule }` (rule bodies, no `RRULE:` prefix). The new
 * series reuses the same pattern; only a COUNT-bounded series needs adjusting —
 * its remaining count is `COUNT - occurrencesBeforeCutoff` (clamped to >= 1). An
 * UNTIL/absolute-ended or open-ended series carries its end unchanged: the same
 * UNTIL still bounds the new series from its later start, and an open series
 * stays open.
 */
export function splitRRuleForEdit(
  rrule: string,
  cutoff: Date,
  occurrencesBeforeCutoff: number,
  opts: { allDay?: boolean; tzid?: string | null; start?: string } = {},
): { oldRule: string; newRule: string } {
  const oldRule = truncateRRuleBefore(rrule, cutoff, opts);
  return { oldRule, newRule: ruleFromCut(rrule, occurrencesBeforeCutoff) };
}

/**
 * A rule read from a cut on: its COUNT less the occurrences before the cut
 * (clamped to at least one), every other part as it is. No prefix.
 *
 * A COUNT counts from the series' first occurrence — which is what the editor
 * shows in its repeat field for any occurrence of the series. So the rule a
 * user set there for "this and all following" (decision 121) is read the same
 * way as an untouched one: "ends after 10 times" on the fifth occurrence
 * leaves six from here, not ten more.
 */
export function ruleFromCut(rrule: string, occurrencesBeforeCutoff: number): string {
  const body = rrule.trim().replace(/^RRULE:/i, '');
  const parts = body.split(';').filter(Boolean);
  const countIdx = parts.findIndex((p) => p.toUpperCase().startsWith('COUNT='));
  if (countIdx === -1) {
    return body;
  }
  const count = Number(parts[countIdx].slice('COUNT='.length));
  const remaining = Number.isFinite(count)
    ? Math.max(1, count - Math.max(0, occurrencesBeforeCutoff))
    : count;
  return parts.map((p, i) => (i === countIdx ? `COUNT=${remaining}` : p)).join(';');
}

/**
 * The occurrence of `master` at `slotIso`, shaped as the views expand it.
 *
 * For a slot the master's own expansion does not produce — one the provider
 * keeps as a row of its own, which the master skips — so an editor can open
 * "this and all following" on the series at that point, with the series'
 * fields, as it does for any other occurrence (decision 126).
 */
export function occurrenceOfSeries<E extends RecurringEventLike>(
  master: E,
  slotIso: string,
): ExpandedOccurrence<E> {
  // A series of days names its slot as a DAY, and another writer may have
  // spelled that day hours off this device's local midnight (decision 95): it
  // opens on the series' own occurrence that day names, as the views show it.
  // With none that close, the slot stays as written — nothing is guessed.
  let at = new Date(slotIso).getTime();
  if (master.all_day === true && master.recurrence?.rrule && Number.isFinite(at)) {
    const named = expandEvent(
      { ...master, recurrence: { ...master.recurrence, exceptions: [] } },
      { start: new Date(at - SAME_DAY_MS), end: new Date(at + SAME_DAY_MS) },
    ).find(
      (occ) =>
        isExpandedOccurrence(occ) &&
        namesTheSameDay(at, new Date(occ.occurrence_start).getTime()),
    );
    if (named && isExpandedOccurrence(named)) {
      at = new Date(named.occurrence_start).getTime();
    }
  }
  const slot = new Date(at);
  // A length that cannot be read is none: the occurrence still opens, where
  // an invalid instant would throw inside the load that opens it.
  const length = new Date(master.end).getTime() - new Date(master.start).getTime();
  const duration = Number.isFinite(length) ? Math.max(0, length) : 0;
  return {
    ...master,
    id: `${master.id}@${slot.toISOString()}`,
    series_id: master.id,
    occurrence_start: slot.toISOString(),
    start: slot.toISOString(),
    end: new Date(slot.getTime() + duration).toISOString(),
  };
}

/**
 * Whether `master` still has an occurrence at `slotIso`, as the views expand
 * it: not deleted (an exception), not cut off (UNTIL, COUNT), not moved away
 * by a changed rule. A reminder can outlive its occurrence — the scan that
 * planned it ran before the delete, or the click waited behind an editor —
 * and an occurrence opened that is no longer there would come back when
 * saved (TODO A5). Compared as {@link sameSlot} compares, so a series of days
 * by its day. A master without a rule has no occurrences: `false`.
 */
export function seriesHasOccurrence<E extends RecurringEventLike>(
  master: E,
  slotIso: string,
): boolean {
  const at = new Date(slotIso).getTime();
  if (!Number.isFinite(at) || !master.recurrence?.rrule) return false;
  const same = slotMatcher(master);
  return expandEvent(master, {
    start: new Date(at - 2 * SAME_DAY_MS),
    end: new Date(at + 2 * SAME_DAY_MS),
  }).some(
    (occ) => isExpandedOccurrence(occ) && same(at, new Date(occ.occurrence_start).getTime()),
  );
}

/**
 * The day the last occurrence of a bounded series falls on, `YYYY-MM-DD` on
 * the series' own clock, or `null`.
 *
 * `UNTIL` is a BOUND, not an occurrence, and three providers write it three
 * ways: Aperio and Exchange as the end of the day in UTC, Apple as the local
 * end of day expressed in UTC, and a truncation as one second before the cut.
 * Reading the digits would name a day the series does not meet, one day out
 * for an invitation from west of Greenwich. So the day is asked of the same
 * expander that decides which occurrences the user sees on screen (decision
 * 85a), and the repeat sentence then agrees with the calendar.
 *
 * `null` for a rule without `UNTIL`, for a `COUNT` rule (whose sentence says
 * how often instead), for a sub-daily rule (whose bound would iterate by the
 * minute) and when nothing falls before the bound.
 */
export function lastOccurrenceDayKey(event: RecurringEventLike): string | null {
  const body = event.recurrence?.rrule?.trim();
  if (!body) return null;
  const upper = body.toUpperCase();
  if (!upper.includes('UNTIL=') || upper.includes('COUNT=')) return null;
  if (/FREQ=(SECONDLY|MINUTELY|HOURLY)/.test(upper)) return null;
  // The rule exactly as the views iterate it (`wallRule`): on the clock they
  // expand this series on, an all-day series' days included (48a), in that
  // clock's wall-clock time, with its bound on that clock — a timed series'
  // instant shifted into it, a series of days' digits as written (201). Built
  // from the stored bound instead, a date ended a series of days a day early
  // where its days begin after this device's midnight, and an evening
  // occurrence on a timed bound's own day fell out by the zone's offset, while
  // the calendar showed both. Outside the `try`, as for `occurrenceCount`: an
  // unreadable rule comes back `null`, and a missing series-clock door throws.
  const built = wallRule({ ...event, recurrence: { ...event.recurrence, rrule: body } });
  if (!built) return null;
  const { rule, zone } = built;
  const facts = endFacts(event);
  const cap = endCap(body, facts, readsCalendarDays(event) ? zone : null);
  try {
    const until = rule.options.until;
    if (!until) return null;
    let last = rule.before(until, true);
    // Capped at the end's own instant, as `expandEvent` caps (a cut at an
    // occurrence pushed past a spring gap).
    while (last && cap !== null && (zone ? wallToReal(last, zone) : last).getTime() > cap) {
      last = rule.before(last, false);
    }
    if (!last) return null;
    // Both readings land on the same digits: a zoned rule answers in wall
    // clock, which already IS the series' clock, and a rule without a zone
    // answers in UTC, which is the clock `expandEvent` reads it on.
    return last.toISOString().slice(0, 10);
  } catch {
    return null;
  }
}
