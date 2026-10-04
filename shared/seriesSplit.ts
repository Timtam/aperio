// Splitting a series at one of its occurrences — "this and all following".
//
// The move itself is two writes that must both happen or neither: a NEW series
// takes over from the chosen occurrence carrying the change, and the original
// series is truncated to end just before it. Between those two writes the
// appointment shows twice from the cutoff, and every detail of the arithmetic
// decides whether the two halves line up afterwards:
//
//   - RFC-5545 COUNT counts every slot the RULE generates, INCLUDING the ones
//     an EXDATE suppresses. Counting the visible occurrences before the cutoff
//     therefore undercounts, and the tail is created one occurrence too long —
//     a phantom appointment past the end of the series.
//   - EXDATEs at or after the cutoff belong to the TAIL. Left behind on the
//     truncated head they do nothing, and the tail resurrects an occurrence the
//     user had explicitly deleted (or doubles one they had moved, its
//     suppressing EXDATE gone).
//   - The tail is a CONTINUATION, so it keeps the master's zone verbatim,
//     floating zones included. Stamping only the tail makes the two halves
//     expand an hour apart across a DST boundary.
//   - Attendees have to hear about the truncation as well as the new tail. On
//     notify-flag providers a silent truncate leaves them holding the old
//     occurrences AND an invitation to the new ones.
//   - The tail is created FIRST and the head cut after (decision 136). The
//     other order lost data: a cut that landed and a tail that did not left
//     the series ending at the cutoff, and putting the head back restored only
//     its rule — the changed and deleted occurrences the cut had dropped at the
//     provider stayed dropped. Deleting a series just created is an exact undo.
//   - But only when the cut certainly never reached the provider (decision
//     144). After a network failure it may have landed, and deleting the tail
//     then would leave the series ending at the cutoff after all. So both stay,
//     and the user is told the series may show twice from the cutoff. The new
//     series is there, so the split counts as written (decision 145): it gets
//     what a written tail gets, and nothing offers to write it again.
//
// That reasoning lived inline in both editors, and carrying an edit to a
// group's other copies would have made it four. It lives here now: the
// arithmetic in `planSeriesSplit`, the order and the undo in
// `writeSeriesSplit`. What each caller still owns is the SHAPE of the row it
// creates — a copy keeps its own colour, reminders and calendar, and only the
// caller knows those.
//
// And one case is no split at all (decision 118). When nothing the calendar
// shows lies before the cutoff — the user picked the first occurrence, or every
// earlier one was deleted — there is no head to keep. Cutting it anyway wrote a
// rule that ends before it starts: the views showed nothing, but the reminder
// expansion rejects such a rule and falls back to the series start, so the
// deleted series went on ringing, on every device. "This and all following" is
// then the whole series, and the plan says so (`kind: 'whole'`): a delete
// deletes it, an edit rewrites it in place, keeping its id.
//
// What the calendar shows is the master AND the occurrences the provider keeps
// as rows of their own (decision 125). A master's exceptions alone cannot say
// it: Exchange lists the slot of every occurrence changed in Outlook among
// them, next to the deleted ones, and Google keeps a deleted occurrence as a
// cancelled row instead. So the plan reads the series' rows too
// (`readSeriesRows`), and every caller has to hand them over.
//
// The same rows decide what the NEW series must not show (decision 134): the
// occurrences the calendar shows nothing for (`deletedSlots`). Taken from the
// master's exceptions alone, a split brought back every occurrence Google
// keeps deleted as a cancelled row, and it deleted from both halves every
// occurrence Exchange lists among its exceptions because it was CHANGED.
//
// Those rows are read by the series' id, not by a stretch of dates (decision
// 135): a row counts by the slot it names, its own times can lie anywhere,
// and the series can run for years. The host answers with every row its cache
// holds for the series, cancelled ones included, and with how far that cache
// reaches (decision 139) — Exchange and CalDAV hand over the whole calendar,
// Google only a window around today.

import { localDateKey } from './dateKey';
import { writeNeverLanded } from './eventWriteError';
import { formatLongDay } from './intlNames';
import type { SeriesReach } from './generated/SeriesReach';
import type { RecurringEventLike } from './recurrence';
import {
  expandAll,
  expandEvent,
  isExpandedOccurrence,
  localTimeZone,
  movedSeriesUntil,
  moveSeriesInstant,
  overrideRecurrenceIso,
  overrideSeriesId,
  ruleFromCut,
  seriesDayKey,
  slotMatcher,
  splitRRuleForEdit,
} from './recurrence';
import { shiftSeriesRule, type SeriesShift, type ShiftRefusal } from './seriesShift';
import { tailExceptions } from './tailExceptions';
import type { TailSlot } from './types';

/** The least a master needs for its series to be split. */
export interface SplittableEvent extends RecurringEventLike {
  all_day: boolean;
}

/** The recurrence the tail series is created with. */
export interface TailRecurrence {
  rrule: string;
  exceptions: string[];
  tzid: string | null;
}

/** What cutting this series at this occurrence would do. */
export type SeriesSplitPlan = SeriesCutPlan | WholeSeriesPlan;

interface SeriesPlanCommon {
  /**
   * The recurrence from the cutoff on: the NEW tail series' after a cut, the
   * series' own after a whole-series rewrite.
   */
  tail: TailRecurrence;
  /**
   * How many occurrences the RULE generates before the cutoff.
   *
   * Exposed because it is the number the COUNT arithmetic turns on, and a test
   * that cannot see it can only check the split from the outside.
   */
  occurrencesBefore: number;
  /**
   * The occurrences from the cutoff on that the calendar shows nothing for
   * (`deletedSlots`): what the series written from the cutoff must not show
   * either, wherever an edit moves them (`tailRecurrenceFor`).
   */
  deleted: string[];
  /**
   * The exceptions from the cutoff on that a row of the series stands in for:
   * Exchange lists the slot of an occurrence changed in Outlook among them.
   * Only a series rewritten in place keeps them, as they are spelled; a new
   * series owns no rows.
   */
  standing: string[];
}

/** The series keeps a head: at least one occurrence before the cutoff. */
export interface SeriesCutPlan extends SeriesPlanCommon {
  kind: 'cut';
  /** The rule the ORIGINAL series keeps: everything strictly before the cutoff. */
  headRule: string;
}

/**
 * Nothing the calendar shows lies before the cutoff, so "this and all
 * following" is the whole series. There is no head rule on purpose: every
 * caller has to decide what the whole series means for it, and the compiler
 * says so wherever one does not.
 */
export interface WholeSeriesPlan extends SeriesPlanCommon {
  kind: 'whole';
}

export type { SeriesReach } from './generated/SeriesReach';

/**
 * The rows of one series besides its master, and how far the host's cache
 * reaches: past it, a row the provider keeps may simply not be here.
 */
export interface SeriesRows<E> {
  rows: E[];
  reach: SeriesReach;
}

/**
 * What a host is asked for the rows of a series. `start` and `end` are for a
 * host that predates the series read — a phone whose native library is older
 * than its app — which reads that stretch instead; a current host ignores them.
 */
export interface SeriesRowsRequest {
  calendar_id: string;
  series_id: string;
  start: string;
  end: string;
}

/**
 * A phone host's answer to a series read. A native library older than the
 * series read ignores the series and answers with the events of the request's
 * stretch, as a list; how far those reach, nobody knows.
 */
export function seriesRowsFromHost<E>(answer: SeriesRows<E> | E[]): SeriesRows<E> {
  return Array.isArray(answer) ? { rows: answer, reach: { kind: 'unknown' } } : answer;
}

/** How far a moved occurrence is still looked for by a host that can only read
 *  a stretch of dates: before the series' start, and past the cutoff. */
const MOVED_REACH_MS = 31 * 24 * 60 * 60 * 1000;

/**
 * The rows of this series the provider keeps besides the master — its changed
 * and its cancelled occurrences — that the plan needs to know what is shown
 * before the cutoff (decision 125), and how far they reach.
 *
 * Read by the series' id through the caller's own `getSeriesRows`, whatever
 * their dates. A master without a rule has no such rows and is not read. A
 * read that fails throws: guessing "nothing there" would delete or rewrite a
 * series whose earlier occurrences are on screen.
 */
export async function readSeriesRows<
  E extends RecurringEventLike,
  M extends SplittableEvent & { calendar_id: string },
>(
  master: M,
  cutoffIso: string,
  getSeriesRows: (request: SeriesRowsRequest) => Promise<SeriesRows<E>>,
): Promise<SeriesRows<E>> {
  if (!master.recurrence?.rrule) return { rows: [], reach: { kind: 'complete' } };
  const cutoff = new Date(cutoffIso).getTime();
  const from = new Date(master.start).getTime() - MOVED_REACH_MS;
  if (!Number.isFinite(cutoff) || !Number.isFinite(from)) {
    return { rows: [], reach: { kind: 'unknown' } };
  }
  const answer = await getSeriesRows({
    calendar_id: master.calendar_id,
    series_id: master.id,
    start: new Date(from).toISOString(),
    end: new Date(Math.max(cutoff, from) + MOVED_REACH_MS).toISOString(),
  });
  // An older host answers with every event of the stretch.
  return {
    rows: answer.rows.filter((row) => overrideSeriesId(row) === master.id),
    reach: answer.reach,
  };
}

/**
 * The slots of this series the calendar shows nothing for, from its master and
 * its rows (`readSeriesRows`): the occurrences a series that continues it must
 * not show either.
 *
 * - A master's exception with no live row of the series in its slot: the
 *   provider deleted that occurrence. An exception WITH a live row is no
 *   deletion: Exchange lists the slot of every occurrence changed in Outlook
 *   among the exceptions, and that occurrence is there.
 * - Every cancelled row's slot: Google keeps a deleted occurrence as a
 *   cancelled row and lists no exception for it; CalDAV can do the same with a
 *   `STATUS:CANCELLED` override.
 *
 * Slots match as the views match them (`sameSlot`): exactly for a timed series,
 * by day for a series of days. An exception keeps its own spelling, a row's slot
 * is written as the instant it names; each slot comes once, in order. Rows of
 * other series are ignored.
 */
export function deletedSlots(
  master: SplittableEvent,
  rows: readonly RecurringEventLike[],
): string[] {
  const own = rows.filter((row) => overrideSeriesId(row) === master.id);
  const slotOf = (row: RecurringEventLike) => Date.parse(overrideRecurrenceIso(row) ?? '');
  const live = own
    .filter((row) => !row.cancelled)
    .map(slotOf)
    .filter((at) => Number.isFinite(at));
  const same = slotMatcher(master);
  const found: { iso: string; at: number }[] = [];
  const add = (iso: string, at: number) => {
    if (!found.some((slot) => same(slot.at, at))) found.push({ iso, at });
  };
  for (const iso of master.recurrence?.exceptions ?? []) {
    const at = Date.parse(iso);
    if (!Number.isFinite(at)) continue;
    if (live.some((slot) => same(slot, at))) continue;
    add(iso, at);
  }
  for (const row of own) {
    if (!row.cancelled) continue;
    const at = slotOf(row);
    if (Number.isFinite(at)) add(new Date(at).toISOString(), at);
  }
  // Instants, compared as numbers: two spellings of one instant are one.
  return found.sort((a, b) => a.at - b.at).map((slot) => slot.iso);
}

/**
 * The arithmetic of a split, decided before anything is written.
 *
 * `rows` are the series' own rows besides the master (`readSeriesRows`); rows
 * of other series are ignored. They decide, with the master, whether anything
 * is shown before the cutoff.
 *
 * `null` when there is nothing to split: the master carries no rule at all, or
 * the cutoff is not a moment. A caller that gets `null` for an event it
 * believed was a series must NOT fall through to editing the whole series —
 * that moves every occurrence, which is the outcome the scope question exists
 * to prevent.
 */
export function planSeriesSplit<E extends SplittableEvent>(
  master: E,
  cutoffIso: string,
  rows: readonly RecurringEventLike[],
): SeriesSplitPlan | null {
  const recurrence = master.recurrence;
  if (!recurrence?.rrule) return null;
  const cutoff = new Date(cutoffIso);
  if (!Number.isFinite(cutoff.getTime())) return null;

  // Occurrences the RULE generates strictly before the cutoff. Two subtleties:
  // (1) `expandEvent`'s range includes the end instant and the cutoff IS an
  // occurrence, so the range ends a millisecond earlier to exclude it;
  // (2) COUNT counts exdated slots too, so the expansion runs with the
  // exceptions cleared — otherwise an EXDATE before the cutoff undercounts and
  // the tail keeps a COUNT one too large.
  const occurrencesBefore = expandEvent(
    { ...master, recurrence: { ...recurrence, exceptions: [] } },
    { start: new Date(master.start), end: new Date(cutoff.getTime() - 1) },
  ).length;

  const { oldRule, newRule } = splitRRuleForEdit(
    recurrence.rrule,
    cutoff,
    occurrencesBefore,
    { allDay: master.all_day },
  );
  // The EXDATEs from the cutoff on move to the tail with the occurrences they
  // suppress. Which side the cutoff's OWN slot falls on depends on the plan,
  // below. It can carry an exception only when the provider keeps that
  // occurrence as a row of its own — Exchange lists such slots among the
  // exceptions — because a deleted occurrence cannot have been opened.
  const tailWith = (keep: (at: number) => boolean): TailRecurrence => ({
    rrule: newRule,
    exceptions: (recurrence.exceptions ?? []).filter((x) =>
      keep(new Date(x).getTime()),
    ),
    tzid: recurrence.tzid ?? null,
  });

  // What the calendar shows of the HEAD — unlike the count above, which is
  // the rule's and feeds COUNT. The master's occurrences before the cutoff,
  // less its exceptions and the slots its own rows stand in for (the reading
  // the views make, `expandAll`), plus those rows that are not cancelled and
  // stand in for a slot before the cutoff: the ones a cut keeps. Nothing
  // there means no head: the cutoff is the first occurrence, or the series
  // starts off its own pattern, or every earlier occurrence was deleted. A
  // rule that cannot be read comes back as the master itself, so it counts as
  // a head and is cut as before.
  //
  // A row counts by its SLOT, not by where it was moved: a cut keeps the rows
  // of the slots before it and drops the others, wherever they are shown. A
  // series of days names its slot as a day, and another writer may have
  // spelled that day hours off its local midnight (decision 95), so a slot
  // counts as before only when it is at least half a day before the cutoff.
  const slotMargin = master.all_day ? 12 * 60 * 60 * 1000 : 0;
  const slotBefore = (row: RecurringEventLike) => {
    const at = Date.parse(overrideRecurrenceIso(row) ?? '');
    return Number.isFinite(at) && at < cutoff.getTime() - slotMargin;
  };
  const ownRows = rows.filter((row) => overrideSeriesId(row) === master.id);
  const headShown = expandAll([master as RecurringEventLike, ...ownRows], {
    start: new Date(master.start),
    end: new Date(cutoff.getTime() - 1),
  }).filter(
    (shown) => shown === master || isExpandedOccurrence(shown) || slotBefore(shown),
  ).length;
  const at = cutoff.getTime();
  if (headShown === 0) {
    // Written whole, in place: an exception on the cutoff's own slot stays, so
    // the occurrence the provider keeps there goes on standing in for it
    // (decision 122). Of the exceptions from the cutoff on, those a row stands
    // in for stay as they are; the others are deletions, and so is every
    // cancelled row's slot.
    const tail = tailWith((x) => x >= at - slotMargin);
    const same = slotMatcher(master);
    const deleted = deletedSlots(master, ownRows).filter((x) => Date.parse(x) >= at - slotMargin);
    const deletedAt = deleted.map((x) => Date.parse(x));
    return {
      kind: 'whole',
      tail,
      occurrencesBefore,
      deleted,
      standing: tail.exceptions.filter(
        (x) => !deletedAt.some((d) => same(d, Date.parse(x))),
      ),
    };
  }
  // A NEW series from the cutoff: the slot it starts on is the occurrence
  // being edited, and the cut drops the provider's row for it. An exception
  // there would hide the new series' first occurrence.
  //
  // It owns no rows of its own, so every occurrence the calendar shows
  // nothing for is an exception of it (`deletedSlots`) — a cancelled row's
  // slot included, a changed occurrence's slot not: the new series shows that
  // occurrence at its pattern time, with the new series' content. The old
  // series keeps its rows up to the cut. After it, a timed series loses them
  // with the truncate (Exchange drops them itself); an all-day series on
  // Google or CalDAV keeps them, because the adapters skip that cleanup for
  // days, so a changed all-day occurrence after the cut shows twice — the
  // head's row and the new series' occurrence. That was so before this rule
  // too, and it goes with the truncate, not here: keeping the exception for
  // it would take the changed occurrences of Exchange out of both halves.
  const after = (x: number) => (master.all_day ? x >= at + slotMargin : x > at);
  const deleted = deletedSlots(master, ownRows).filter((x) => after(Date.parse(x)));
  return {
    kind: 'cut',
    headRule: oldRule,
    tail: {
      rrule: newRule,
      exceptions: deleted,
      tzid: recurrence.tzid ?? null,
    },
    occurrencesBefore,
    deleted,
    standing: [],
  };
}

/** An occurrence with its day on its series' clock and on the device's. */
function slotOf(at: string, tzid: string | null | undefined, allDay: boolean): TailSlot {
  return {
    at,
    day: seriesDayKey(at, tzid, allDay),
    device_day: seriesDayKey(at, null, true),
  };
}

const HALF_DAY_MS = 12 * 60 * 60 * 1000;
const DAY_MS = 24 * 60 * 60 * 1000;
/** Past any series a calendar sensibly holds, as `firstOccurrenceFrom` reaches. */
const LONGEST_REACH_MS = 15_000 * DAY_MS;

/**
 * The rule of "this and all following" cannot move with its new date: it names
 * days a move by whole days would change the meaning of — the second Sunday,
 * the 30th, a rule for some months only (decision 193). Thrown before anything
 * is written; the surface says why, using `reason`, and the user chooses the
 * repeat themselves or changes only the one occurrence.
 */
export class TailShiftRefusedError extends Error {
  constructor(readonly reason: ShiftRefusal) {
    super(`the repeat rule cannot move with the new date or time: ${reason}`);
    this.name = 'TailShiftRefusedError';
  }
}

/** A rule of "this and all following", and where its start went. */
export interface TailRuleMove {
  /** The rule as it stands. */
  rrule: string;
  /** The clock the series repeats on: its zone, and whether it is all-day. */
  series: { tzid?: string | null; all_day: boolean };
  /**
   * The occurrence the rule names first: the cut, or the start the user set
   * the rule at.
   */
  from: string;
  /**
   * The start the edit is read against: the occurrence the editor was filled
   * from, or `from` itself.
   */
  opened: string;
  /** The start and the kind being written. */
  tail: { start: string; all_day: boolean };
}

/**
 * The zone of the series written from the cut on: kept while it stays timed or
 * stays all-day (a continuation expands as its other half does), none once it
 * becomes all-day, the device's once it gets a time of day, as a created
 * series does (DESIGN-series-time-zone.md, "Ganztägig aus").
 */
function tailZone(
  zone: string | null | undefined,
  allDay: boolean,
  tailAllDay: boolean,
): string | null {
  if (tailAllDay === allDay) return zone ?? null;
  return tailAllDay ? null : localTimeZone();
}

/** A rule's UNTIL as written, when it ends by one. */
function untilOf(rrule: string): string | null {
  return /(?:^|[;:])\s*UNTIL=([^;]*)/i.exec(rrule)?.[1]?.trim() ?? null;
}

/** The rule with its UNTIL written as `value`. */
function withUntil(rrule: string, value: string): string {
  return rrule.replace(/((?:^|[;:])\s*UNTIL=)[^;]*/i, `$1${value}`);
}

/** The rule without its UNTIL: it repeats on, for as long as it is read. */
function withoutUntil(rrule: string): string {
  const [, prefix = '', body = ''] = /^(RRULE:)?(.*)$/is.exec(rrule.trim()) ?? [];
  return (
    prefix +
    body
      .split(';')
      .filter((part) => part.trim() !== '' && !/^\s*UNTIL=/i.test(part))
      .join(';')
  );
}

/**
 * Far enough past a rule's UNTIL to see every occurrence it keeps, read
 * loosely: a date or a floating time as UTC, two days on.
 */
function pastUntil(value: string): number {
  const m = /^(\d{4})(\d{2})(\d{2})/.exec(value);
  return m ? Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3])) + 2 * DAY_MS : NaN;
}

/** The occurrences a rule generates from `start` up to `until`, no exception. */
function occurrencesFrom(
  start: string,
  allDay: boolean,
  rrule: string,
  tzid: string | null,
  until: number,
): string[] {
  const series: SplittableEvent = {
    id: 'moved',
    start,
    end: start,
    all_day: allDay,
    recurrence: { rrule, exceptions: [], tzid },
  };
  const from = new Date(Date.parse(start) - (allDay ? HALF_DAY_MS : 0));
  return expandEvent(series, { start: from, end: new Date(until) }).map((occ) => occ.start);
}

/**
 * The moved rule with the bound that keeps the series as long as it was: the
 * new series ends on the occurrence in the place of the old one's last
 * (decision 194).
 *
 * `candidates` are the moved rule with its bound moved two ways: by whole
 * days, which keeps its spelling — the end of a day the repeat field writes, a
 * provider's date — and by the days and the new time of day. The first that
 * keeps as many occurrences as the old series had from `from` is it. Neither
 * does when the occurrences move by other steps than the start: a monthly rule
 * that begins anew on another day of the month moves each by another number of
 * days, and a date bound ends a series that got a time of day at its midnight.
 * Then the bound is the new series' own occurrence in that place, written the
 * way its kind needs: a date for an all-day series, a UTC time otherwise. All
 * that only when the new series starts on its own rule — places line up then;
 * otherwise the bound moves by the days and the new time of day.
 */
function keepingLength(
  rrule: string,
  candidates: readonly string[],
  series: { tzid: string | null; all_day: boolean },
  from: string,
  tail: { start: string; all_day: boolean; tzid: string | null },
): string {
  const last = candidates[candidates.length - 1];
  const old = untilOf(rrule);
  if (old == null || untilOf(candidates[0]) == null || /(?:^|[;:])\s*COUNT=/i.test(rrule)) {
    return last;
  }
  const before = occurrencesFrom(from, series.all_day, rrule, series.tzid, pastUntil(old));
  if (before.length === 0) return last;
  const same = slotMatcher({ all_day: tail.all_day, recurrence: { tzid: tail.tzid } });
  const reach = (candidate: string) =>
    pastUntil(untilOf(candidate) ?? '') + Math.abs(Date.parse(tail.start) - Date.parse(from));
  const counted = candidates.map((candidate) =>
    occurrencesFrom(tail.start, tail.all_day, candidate, tail.tzid, reach(candidate)),
  );
  const first = counted[0][0];
  if (first === undefined || !same(Date.parse(first), Date.parse(tail.start))) return last;
  const keeping = candidates.find((_, i) => counted[i].length === before.length);
  if (keeping !== undefined) return keeping;
  // The new series without its bound, as far as the old one's occurrences.
  const open = withoutUntil(candidates[0]);
  let span = reach(candidates[0]) - Date.parse(tail.start) + 2 * DAY_MS;
  let found: string[] = [];
  for (;;) {
    found = occurrencesFrom(tail.start, tail.all_day, open, tail.tzid, Date.parse(tail.start) + span);
    if (found.length >= before.length || span >= LONGEST_REACH_MS) break;
    span = Math.min(span * 4, LONGEST_REACH_MS);
  }
  const end = found[before.length - 1];
  if (end === undefined) return last;
  return withUntil(
    candidates[0],
    tail.all_day
      ? seriesDayKey(end, null, true).replace(/-/g, '')
      : new Date(end).toISOString().replace(/[-:]/g, '').replace(/\.\d{3}/, ''),
  );
}

/**
 * The rule "this and all following" writes when its start moved (decisions
 * 189, 192-194), through the core's `begin_series_anew`.
 *
 * - Its days move only when this edit moved the date, from `opened` to the new
 *   start (192). They move from `from`, so the rule names the new start:
 *   "every Monday" from a Monday the user put on Thursday is "every Thursday".
 *   Days are counted on the series' clock, where its rule is read — or on the
 *   device's days when the series switches between all-day and a time of day,
 *   as the core counts them then: an all-day start is the device's midnight,
 *   which another clock may put on the day before.
 * - A rule that names its days moves them, or is refused (193): the answer is
 *   `refused`, with the reason the surface says. One that names its times is
 *   refused for a new time of day as well.
 * - Its UNTIL moves so that the new series ends on the occurrence in the place
 *   of the old one's last (194, `keepingLength`).
 *
 * Nothing moved gives the rule back as it was written.
 */
export function movedTailRule({ rrule, series, from, opened, tail }: TailRuleMove): SeriesShift {
  const tzid = series.tzid ?? null;
  const allDay = series.all_day;
  const sameKind = tail.all_day === allDay;
  const dayNumber = (iso: string) =>
    Date.parse(sameKind ? seriesDayKey(iso, tzid, allDay) : seriesDayKey(iso, null, true)) /
    DAY_MS;
  const days =
    dayNumber(tail.start) === dayNumber(opened)
      ? 0
      : Math.round(dayNumber(tail.start) - dayNumber(from));
  // A time of day only while the series stays timed; the switch to or from
  // all-day is the zone's (`tailZone`).
  const timed = !allDay && !tail.all_day;
  const timeChanges =
    timed &&
    moveSeriesInstant(opened, tzid, false, 0, tail.start) !==
      moveSeriesInstant(opened, tzid, false, 0);
  const byDays = moveSeriesInstant(from, tzid, allDay, days);
  const byTime = moveSeriesInstant(from, tzid, allDay, days, timed ? tail.start : undefined);
  if (sameKind && Date.parse(byTime) === Date.parse(from)) {
    return { outcome: 'shifted', rrule };
  }
  let moved = rrule;
  if (days !== 0 || timeChanges) {
    const answer = shiftSeriesRule(
      rrule,
      seriesDayKey(from, tzid, allDay),
      days,
      timeChanges,
      movedSeriesUntil(rrule, tzid, allDay, from, byDays),
      { beginsAnew: true },
    );
    if (answer.outcome === 'refused') return answer;
    moved = answer.rrule;
  }
  // The bound moved by the new time of day too, where that is another one.
  const timedBound = movedSeriesUntil(rrule, tzid, allDay, from, byTime);
  const candidates =
    timedBound !== undefined && Date.parse(byTime) !== Date.parse(byDays)
      ? [moved, withUntil(moved, timedBound)]
      : [moved];
  return {
    outcome: 'shifted',
    rrule: keepingLength(rrule, candidates, { tzid, all_day: allDay }, from, {
      ...tail,
      tzid: tailZone(tzid, allDay, tail.all_day),
    }),
  };
}

/**
 * The recurrence of the series written from the cutoff on: the new series of
 * a split, or the series itself when "this and all following" rewrites it
 * whole (`kind: 'whole'`).
 *
 * - The rule: the series' own from the cutoff on, or the one the user set in
 *   the repeat field (`rule.form`, when `rule.touched` or it differs from
 *   `rule.opened`, what the field was filled with), its COUNT counted from the
 *   series' first occurrence as the field showed it (decision 121). A cleared
 *   field gives no rule, and `null` comes back: the edit writes a single event.
 *   The series' own rule moves with a new date and time read against
 *   `opened` (`movedTailRule`, decisions 189, 192-194), and one that cannot
 *   throws {@link TailShiftRefusedError}.
 * - The zone: kept as it is while the series stays timed or stays all-day (a
 *   continuation expands as its other half does); none once it becomes all-day;
 *   the device's once it gets a time of day, as a created series does
 *   (DESIGN-series-time-zone.md, "Ganztägig aus").
 * - The exceptions: the deleted occurrences (`plan.deleted`) go along with the
 *   edit — by their place when every occurrence moves with the date, the rule
 *   moved along included, by their day under a new rule or a rule that names
 *   its days and did not move — and the ones a row
 *   stands in for stay as spelled (`plan.standing`). Which goes where is the
 *   core's rule (`cal_core::tail_exceptions`); this expands both series for it,
 *   with each occurrence's day on its series' clock and on the device's,
 *   because the device's zone is this surface's to know.
 *
 * `tail` is the start and the kind of the row being written; `opened` the
 * start the edit is read against — the occurrence an editor was filled from,
 * the copy's own cutoff for a carry; `rule` only an editor passes — a carry
 * never changes the rule. Throws when the door into the core is missing (a
 * phone library older than it), before anything is written.
 */
export function tailRecurrenceFor(input: {
  master: SplittableEvent;
  cutoffIso: string;
  plan: SeriesSplitPlan;
  tail: { start: string; all_day: boolean };
  opened?: string;
  rule?: { form: string | null; opened: string | null; touched?: boolean };
}): TailRecurrence | null {
  const { master, cutoffIso, plan, tail, opened, rule } = input;
  const ruleChanged =
    rule !== undefined && (rule.touched === true || rule.form !== rule.opened);
  let rrule = ruleChanged
    ? rule.form && ruleFromCut(rule.form, plan.occurrencesBefore)
    : plan.tail.rrule;
  if (!rrule) return null;
  const tzid = tailZone(plan.tail.tzid, master.all_day, tail.all_day);
  if (!ruleChanged && opened !== undefined) {
    const moved = movedTailRule({
      rrule,
      series: { tzid: master.recurrence?.tzid, all_day: master.all_day },
      from: cutoffIso,
      opened,
      tail,
    });
    if (moved.outcome === 'refused') {
      throw new TailShiftRefusedError(moved.reason);
    }
    rrule = moved.rrule;
  }

  const cut = Date.parse(cutoffIso);
  const deletedAt = plan.deleted.map((x) => Date.parse(x)).filter((x) => Number.isFinite(x));
  let oldSlots: TailSlot[] = [];
  let tailSlots: TailSlot[] = [];
  if (deletedAt.length > 0 && Number.isFinite(cut) && master.recurrence) {
    // The old series' slots from the cutoff to its last deleted one, as its
    // rule generates them: the exceptions cleared, so a deletion has its place.
    // A series of days names a slot within half a day of it (decision 95).
    const last = Math.max(...deletedAt);
    const oldMargin = master.all_day ? HALF_DAY_MS : 0;
    oldSlots = expandEvent(
      { ...master, recurrence: { ...master.recurrence, exceptions: [] } },
      { start: new Date(cut - oldMargin), end: new Date(last + oldMargin) },
    ).map((occ) => slotOf(occ.start, master.recurrence?.tzid, master.all_day));
    // The new series' occurrences, as many as the old slots and at least past
    // the last deleted day — the date may have moved either way. Widening, as
    // `firstOccurrenceFrom` does, until there are enough or the rule ends.
    const start = Date.parse(tail.start);
    const from = new Date(start - (tail.all_day ? HALF_DAY_MS : 0));
    const series: SplittableEvent = {
      id: 'tail',
      start: tail.start,
      end: tail.start,
      all_day: tail.all_day,
      recurrence: { rrule, exceptions: [], tzid },
    };
    let span = Math.max(last, start) + Math.abs(start - cut) + 2 * DAY_MS - from.getTime();
    let found = -1;
    for (;;) {
      const expanded = expandEvent(series, { start: from, end: new Date(from.getTime() + span) });
      tailSlots = expanded.map((occ) => slotOf(occ.start, tzid, tail.all_day));
      if (
        tailSlots.length >= oldSlots.length ||
        tailSlots.length === found ||
        span >= LONGEST_REACH_MS
      ) {
        break;
      }
      found = tailSlots.length;
      span = Math.min(span * 4, LONGEST_REACH_MS);
    }
  }
  const answer = tailExceptions({
    old_slots: oldSlots,
    old_all_day: master.all_day,
    old_tzid: master.recurrence?.tzid ?? null,
    old_rule: master.recurrence?.rrule ?? '',
    tail: tailSlots,
    tail_rule: rrule,
    tail_start: tail.start,
    tail_all_day: tail.all_day,
    tail_tzid: tzid ?? null,
    deleted: plan.deleted,
    standing: plan.standing,
  });
  return { rrule, exceptions: answer.exceptions, tzid: tzid ?? null };
}

/**
 * The series as it stands from the cutoff on, for a plan with no head.
 *
 * It starts at the cutoff and repeats by the plan's `tail`: the same pattern,
 * a COUNT less what the rule generated before, the exceptions from the cutoff
 * on. For the usual case — the cutoff IS the series' first occurrence — that is
 * the master exactly. For a series that starts off its own pattern, or whose
 * earlier occurrences were all deleted, it is the same appointment anchored
 * where it is first seen, which is what a whole-series edit then reads its
 * change against.
 */
export function seriesFromCut<E extends SplittableEvent & { end: string }>(
  master: E,
  plan: WholeSeriesPlan,
  cutoffIso: string,
): E {
  const cutoff = new Date(cutoffIso).getTime();
  const start = new Date(master.start).getTime();
  const sameStart = cutoff === start;
  const duration = new Date(master.end).getTime() - start;
  return {
    ...master,
    start: sameStart ? master.start : new Date(cutoff).toISOString(),
    end: sameStart ? master.end : new Date(cutoff + duration).toISOString(),
    recurrence: {
      ...master.recurrence,
      rrule: plan.tail.rrule,
      exceptions: plan.tail.exceptions,
      tzid: plan.tail.tzid,
    },
  };
}

/** What "delete this and all following" did. */
export type SeriesDeleteOutcome =
  /** The series ends just before the cutoff; the earlier occurrences stay. */
  | 'truncated'
  /** Nothing came before the cutoff: the whole series was deleted. */
  | 'deleted';

/**
 * The sentence for a "delete this and all following", by what it did.
 *
 * One table for every place that deletes from an occurrence — the editor, the
 * four views, the phone — so none of them says "the earlier ones stay" about a
 * series that had none and is gone.
 */
export function thisAndFutureDeletedKey(
  outcome: SeriesDeleteOutcome,
  notified: boolean,
): string {
  if (outcome === 'deleted') {
    return notified
      ? 'dialogs.event.thisAndFutureCancelledWhole'
      : 'dialogs.event.thisAndFutureDeletedWhole';
  }
  return notified
    ? 'dialogs.event.thisAndFutureCancelled'
    : 'dialogs.event.thisAndFutureDeleted';
}

/**
 * The first occurrence of this series that is not yet OVER at `fromIso`.
 *
 * The anchor of a split is the occurrence the user picked, and on that copy it
 * IS an occurrence. On another copy of the same appointment it need not be:
 * copies are separate series and may be patterned differently, so the honest
 * anchor for "and all following" over there is that copy's own next occurrence.
 *
 * "Not yet over" rather than "starting at or after": copies of one appointment
 * often start a little apart — the work copy at 09:00 and the private one at
 * 08:45 for the walk over — and by START alone the copy that is already
 * running at the cutoff counted as past. It was then cut a whole period late:
 * today's appointment stayed as it was and next week's carried the change.
 *
 * `null` when the series has none left — the copy ends before the cutoff, and
 * there is nothing to carry to it. Reported to the user rather than treated as
 * success, because a copy that silently kept its old shape is exactly the
 * contradiction a group exists to prevent.
 */
export function firstOccurrenceFrom<E extends SplittableEvent>(
  master: E,
  fromIso: string,
  rows: readonly RecurringEventLike[],
): string | null {
  const from = new Date(fromIso);
  if (!Number.isFinite(from.getTime())) return null;
  const duration = Math.max(
    0,
    new Date(master.end).getTime() - new Date(master.start).getTime(),
  );
  if (!master.recurrence?.rrule) {
    // A single event is its own only occurrence — and only if it is not over.
    return new Date(master.end).getTime() > from.getTime() ? master.start : null;
  }
  // Widening rather than one fixed window. A weekly series answers in the first
  // step; a three-yearly one would have fallen outside any horizon short enough
  // to keep the common case cheap, and "no occurrence" is not a harmless answer
  // here — it tells the carry this copy has nothing left, and the copy is
  // reported as one it could not do. The last step reaches forty years out,
  // past any series a calendar sensibly holds.
  //
  // The range starts one duration EARLIER than the cutoff because `expandEvent`
  // selects by start: an occurrence already running at the cutoff begins before
  // it, and it is the one being split at, not the next one.
  //
  // Which occurrences there ARE is what the calendar shows (decision 141): an
  // occurrence changed in Outlook is there although Exchange lists its slot
  // among the exceptions, and one Google deleted is gone although no
  // exception names it. So the series is read with its deleted slots
  // (`deletedSlots`), and an occurrence counts at its SLOT, not where it was
  // moved, as the plan cuts.
  const shown = {
    ...master,
    recurrence: { ...master.recurrence, exceptions: deletedSlots(master, rows) },
  };
  const DAY_MS = 24 * 60 * 60 * 1000;
  const searchFrom = new Date(from.getTime() - duration);
  for (const days of [400, 1_200, 4_000, 15_000]) {
    const horizon = new Date(from.getTime() + days * DAY_MS);
    const found = expandEvent(shown, { start: searchFrom, end: horizon }).find(
      (occ) => new Date(occ.start).getTime() + duration > from.getTime(),
    );
    if (found) return found.start;
  }
  return null;
}

/**
 * The three writes a split needs, in the caller's own terms.
 *
 * Deliberately narrow: the shaping of the rows is the caller's business (a copy
 * keeps its colour, its reminders and its calendar, and only the caller knows
 * what the user just typed), while the ORDER, the notify flag and the recovery
 * are the same everywhere and belong to `writeSeriesSplit`.
 */
export interface SeriesSplitIo<Created> {
  /** Create the tail series with this recurrence, keeping the zone verbatim. */
  createTail(recurrence: TailRecurrence): Promise<Created>;
  /**
   * Write the master back ending just before the cutoff.
   *
   * Must pass the notify flag on, and must ask the adapter to drop provider-side
   * overrides in the dropped tail — a cross-client single-occurrence change
   * synced in as its own RECURRENCE-ID row otherwise survives the truncation and
   * ghosts against the new series.
   */
  truncate(headRule: string): Promise<unknown>;
  /**
   * Delete the tail `createTail` just made.
   *
   * Called only when the truncate certainly changed nothing. Must carry the
   * SAME notify flag as `createTail`: attendees invited to the new series have
   * to hear that it is gone again.
   */
  removeTail(created: Created): Promise<unknown>;
}

/**
 * The day a split cuts at, written out in `language`: the day the user sees
 * the occurrence on, for a sentence that has to name it after the dialog is
 * gone.
 */
export function cutoffDay(cutoffIso: string, language: string): string {
  const at = new Date(cutoffIso);
  return Number.isNaN(at.getTime())
    ? cutoffIso
    : formatLongDay(localDateKey(at), language);
}

/**
 * A split that was written: the new series, and whether the old one certainly
 * ends before the cutoff.
 *
 * `unsure` when cutting the old one short failed in a way that may have
 * reached the provider (decision 144). The new series stands either way, so
 * the caller treats it as written — its private reminders, its colour, its
 * group — and says that the series may show twice from the cutoff, with what
 * went wrong (`failure`), instead of that it changed (decision 145).
 */
export type SeriesSplitWritten<Created> =
  | { tail: Created; headCut: 'done' }
  | { tail: Created; headCut: 'unsure'; failure: unknown };

/** Marks a thrown error after which the series may show twice. */
const MAYBE_SHOWN_TWICE = Symbol.for('aperio.seriesSplit.maybeShownTwice');

/**
 * Whether this failure may have left the series showing TWICE from the cutoff.
 *
 * The ordinary failure of a split changes nothing: the tail could not be
 * created, or the truncate was refused and the tail deleted again, and the
 * caller reports an error over an untouched calendar. But when the tail could
 * not be deleted after all, it may stand next to the old series, which runs
 * through the cutoff. That is a different thing to tell the user, and
 * reporting it as "not changed" would be the opposite of true.
 */
export function seriesMaybeShownTwice(err: unknown): boolean {
  return (
    typeof err === 'object' &&
    err !== null &&
    (err as Record<symbol, unknown>)[MAYBE_SHOWN_TWICE] === true
  );
}

/** The failure as thrown, marked: an object carries the mark itself. */
function markedShownTwice(err: unknown): unknown {
  const carrier =
    typeof err === 'object' && err !== null ? err : new Error(String(err));
  (carrier as Record<symbol, unknown>)[MAYBE_SHOWN_TWICE] = true;
  return carrier;
}

/**
 * Create the tail, then truncate — and delete the tail if the truncate was
 * refused.
 *
 * The failure path is the reason this is one function. Deleting the series
 * just created is an exact undo, but only when the truncate certainly changed
 * nothing (`writeNeverLanded`): after a failure that may have reached the
 * provider, deleting the tail would leave a series that ends at the cutoff,
 * every appointment from there on gone. So then both stay (decision 144), and
 * the split is returned as written, with the old series' cut `unsure`
 * (decision 145): thrown, it would invite the caller to write it again, and
 * the new series would stand twice.
 *
 * A refused truncate throws its ORIGINAL failure — it is what the caller is
 * waiting for, and a second message about a failed repair would bury it. When
 * the tail could not be deleted either, that is marked on it, so the caller
 * can say the series may show twice (`seriesMaybeShownTwice`).
 */
export async function writeSeriesSplit<Created>(
  io: SeriesSplitIo<Created>,
  plan: SeriesCutPlan,
): Promise<SeriesSplitWritten<Created>> {
  // The type already refuses a plan with no head; this refuses one that got
  // past it. Truncating such a series writes a rule that ends before it
  // starts — the deleted series that goes on ringing (decision 118).
  if ((plan as SeriesSplitPlan).kind !== 'cut') {
    throw new Error(
      'A series with nothing before the cutoff is not split: it is written whole.',
    );
  }
  const tail = await io.createTail(plan.tail);
  try {
    await io.truncate(plan.headRule);
  } catch (err) {
    if (!writeNeverLanded(err)) return { tail, headCut: 'unsure', failure: err };
    try {
      await io.removeTail(tail);
    } catch {
      throw markedShownTwice(err);
    }
    throw err;
  }
  return { tail, headCut: 'done' };
}
