// What choosing a series' zone says — stage 12 of DESIGN-series-time-zone.md,
// "Die Zone wechseln" and "Die Wochentag-Frage (15a)", in the wording of 138a:
// the zone button's entry, the weekday question and what is announced after
// a choice. The same on both editors.
//
// The rule is the core's (`seriesClockChange`, stage 9): whether to ask, the
// two answers, and why one is refused. This file only words them.

import type { ClockFields } from './clockFields';
import { fieldInstant } from './clockFields';
import { seriesDayKey } from './recurrence';
import { describeRecurrence, recurrenceDaysText } from './recurrenceSummary';
import type { ClockChangeAnswer, ClockChangeRefusal, SeriesClockChange } from './seriesClock';
import type { ZoneOffsets } from './types';
import { clockIntlZone, clockRangeText, zoneCityName } from './zoneFormText';
import { listedZoneLabels, zoneChoice, zoneEntryText } from './zoneList';

type Translate = (key: string, vars?: Record<string, unknown>) => string;

const DAY_MS = 86_400_000;

/** The first letter upper case, for a phrase that opens a button. */
function upperFirst(text: string): string {
  return text.length === 0 ? text : text[0].toLocaleUpperCase() + text.slice(1);
}

/**
 * The zone in effect as the button and the announcements name it, its offset
 * at the series' start (`offsets`, from `zoneOffsets`): „Berlin, Europa,
 * UTC+02:00“, „Europe/Kiev (nicht in Aperios Liste)“, „UTC (ohne
 * Sommerzeit)“.
 */
export function zoneChoiceEntry(zone: string | null, offsets: ZoneOffsets, t: Translate): string {
  const placed = zoneChoice(zone, null).stored;
  switch (placed.kind) {
    case 'listed':
      return zoneEntryText(listedZoneLabels()[placed.position], offsets.zones[placed.position].offset, t);
    case 'not_listed':
      return t('dialogs.seriesTimeZone.notListed', { name: placed.name });
    case 'unreadable':
      return t('dialogs.seriesTimeZone.unreadable', { name: placed.name });
    case 'utc':
      return zoneEntryText(null, null, t);
  }
}

/** An instant on the clock of `zone` as the question says it: „Dienstag,
 *  16. Juni, 00:30“. */
function onClock(at: string, zone: string | null, language: string): string {
  const timeZone = clockIntlZone(zone);
  const when = new Date(at);
  const day = new Intl.DateTimeFormat(language, { weekday: 'long', day: 'numeric', month: 'long', timeZone });
  const time = new Intl.DateTimeFormat(language, { hour: '2-digit', minute: '2-digit', timeZone });
  return `${day.format(when)}, ${time.format(when)}`;
}

/** The days a rule repeats on, starting on `day` (`YYYY-MM-DD` on the
 *  rule's own clock): „jeden Montag“; the rule's own days where it has no
 *  words for them. */
function daysOn(rrule: string, day: string, language: string, t: Translate): string {
  const summary = describeRecurrence({ rrule, start: day, last_day: null });
  return recurrenceDaysText(summary, { t, language }) ?? t('dialogs.seriesTimeZone.question.ownDays');
}

/** The days a rule repeats on, starting at `start` on the clock of `zone`. */
function daysOf(rrule: string, start: string, zone: string | null, language: string, t: Translate): string {
  return daysOn(rrule, seriesDayKey(start, zone, false), language, t);
}

/** A sentence whose last word is a day of the month keeps one period: „am
 *  15.“ ends it, not „am 15..“. */
function closed(sentence: string): string {
  return sentence.replace(/\.\.$/, '.');
}

/** Why an answer cannot be carried out, as a clause. */
function refusalText(reason: ClockChangeRefusal, zone: string | null, t: Translate): string {
  return t(`dialogs.moveScope.refusal.${reason}`, { clock: zoneCityName(zone) });
}

/** One answer button of the question. */
export interface WeekdayAnswerButton {
  answer: 'rule' | 'now';
  label: string;
}

/** The weekday question, worded (138a). */
export interface WeekdayQuestionText {
  /** „Welcher Wochentag soll gelten?“ */
  title: string;
  /** What the start falls on and what the rule says, and why an answer has
   *  no button. Every button has it as its description. */
  message: string;
  /** „Zeitzone nicht ändern“: first, with the first focus; Escape does it. */
  keep: string;
  /** The answers that can be carried out, as the rule says first. */
  answers: WeekdayAnswerButton[];
}

/**
 * The weekday question for a zone change the core asks about (138a):
 *
 * - Title „Welcher Wochentag soll gelten?“;
 * - message „In Berlin fällt der Beginn auf Dienstag, 16. Juni, 00:30; die
 *   Regel sagt jeden Montag.“, and for each answer that cannot be carried out
 *   why („… geht nicht: {Grund}.“), its button left out;
 * - buttons „Zeitzone nicht ändern“, „Jeden Montag, wie die Regel sagt
 *   (Termine rücken einen Tag vor)“ (vor: to an earlier day, zurück: to a
 *   later one; left out where the events do not all move alike) and
 *   „Jeden Dienstag, wo die Termine jetzt liegen“.
 *
 * `start` is the series' start as the form holds it, `rrule` its rule, `zone`
 * the zone chosen (`null` for UTC).
 */
export function weekdayQuestion(args: {
  change: SeriesClockChange;
  start: string;
  rrule: string;
  zone: string | null;
  language: string;
  t: Translate;
}): WeekdayQuestionText {
  const { change, start, rrule, zone, language, t } = args;
  const clock = zoneCityName(zone);
  // The rule's days as it reads them now: a day it takes from its start is
  // the start's day on its own clock, the old one.
  const ruleDays = daysOn(rrule, change.startDayBefore, language, t);
  const lines = [
    closed(
      t('dialogs.seriesTimeZone.question.message', {
        clock,
        start: onClock(start, zone, language),
        days: ruleDays,
      }),
    ),
  ];
  const answers: WeekdayAnswerButton[] = [];
  const asRule = change.asRule;
  if (asRule.outcome === 'changed') {
    // How far the events move. Each falls now on its day on the old clock
    // plus the days the start's day shifts between the clocks; as the rule
    // says, it goes back onto the day the rule names. So every event moves by
    // the start's day on the old clock less its day on the new one, whether
    // the start itself moves (a series that really means its old zone) or
    // stays (one that lost its zone), east or west. Not where the start stays
    // and the rule repeats every few weeks, months or years: its period is
    // then counted from the start's day on the new clock, which may lie in
    // another week, month or year, and the events do not all move alike.
    // The button then says the days alone, rather than a wrong move.
    const interval = Number(/(?:^|;)INTERVAL=(\d+)/i.exec(asRule.rrule)?.[1] ?? '1');
    const startStays = Date.parse(asRule.start) === Date.parse(start);
    const moved =
      interval > 1 && startStays
        ? 0
        : Math.round((Date.parse(change.startDayBefore) - Date.parse(change.startDayAfter)) / DAY_MS);
    const days = upperFirst(daysOf(asRule.rrule, asRule.start, zone, language, t));
    answers.push({
      answer: 'rule',
      label:
        moved === 0
          ? t('dialogs.seriesTimeZone.question.asRule', { days })
          : t(`dialogs.seriesTimeZone.question.${moved < 0 ? 'asRuleEarlier' : 'asRuleLater'}`, {
              days,
              count: Math.abs(moved),
            }),
    });
  } else {
    lines.push(
      t('dialogs.seriesTimeZone.question.refused.rule', {
        days: upperFirst(ruleDays),
        reason: refusalText(asRule.reason, zone, t),
      }),
    );
  }
  const asNow = change.asNow;
  if (asNow.outcome === 'changed') {
    answers.push({
      answer: 'now',
      label: t('dialogs.seriesTimeZone.question.asNow', {
        days: upperFirst(daysOf(asNow.rrule, asNow.start, zone, language, t)),
      }),
    });
  } else {
    lines.push(
      t('dialogs.seriesTimeZone.question.refused.now', { reason: refusalText(asNow.reason, zone, t) }),
    );
  }
  return {
    title: t('dialogs.seriesTimeZone.question.title'),
    message: lines.join(' '),
    keep: t('dialogs.seriesTimeZone.question.keep'),
    answers,
  };
}

/**
 * What is said once a zone is chosen ("Die Zone wechseln", "Danach"), once:
 * - a new event keeps its typed times on the new clock: „Zeitzone der Serie:
 *   New York. Die Uhrzeiten bleiben wie getippt und gelten jetzt auf dieser
 *   Uhr: Montag, 14. September, 09:00 bis 10:00.“;
 * - something stored keeps its instant: „… Die Serie beginnt zum selben
 *   Zeitpunkt: Sonntag, 13. September, 18:30 bis 19:30 auf dieser Uhr.“;
 * - an answer that moved the start: „… Die Serie beginnt jetzt am Montag,
 *   14. September, 00:30 auf dieser Uhr.“;
 * - and where the answer rewrote the rule: „Wiederholt sich jeden Dienstag.“
 *
 * `fields` are the fields after the choice, on the new clock; `rewritten`
 * the rule the answer wrote where it is another than before.
 */
export function zoneChoiceAnnouncement(args: {
  kind: 'typed' | 'sameInstant' | 'startMoved';
  zone: string | null;
  fields: ClockFields;
  rewritten: string | null;
  language: string;
  t: Translate;
}): string {
  const { kind, zone, fields, rewritten, language, t } = args;
  const clock = zoneCityName(zone);
  const start = fieldInstant(fields.start, fields.clock);
  const end = fieldInstant(fields.end, fields.clock);
  const both = start.kind === 'at' && end.kind === 'at' ? { start: start.at, end: end.at } : null;
  const times = both && clockRangeText(both.start, both.end, language, clockIntlZone(zone), t);
  const first =
    kind === 'typed'
      ? times === null
        ? t('dialogs.seriesTimeZone.announce.typedNoTimes', { clock })
        : t('dialogs.seriesTimeZone.announce.typed', { clock, times })
      : kind === 'startMoved' && both !== null
        ? t('dialogs.seriesTimeZone.announce.startMoved', { clock, start: onClock(both.start, zone, language) })
        : times === null
          ? t('dialogs.seriesTimeZone.announce.typedNoTimes', { clock })
          : t('dialogs.seriesTimeZone.announce.sameInstant', { clock, times });
  if (rewritten === null || both === null) return first;
  const days = daysOf(rewritten, both.start, zone, language, t);
  return `${first} ${closed(t('dialogs.seriesTimeZone.announce.repeats', { days }))}`;
}

/** „Zeitzone der Serie nicht geändert: Berlin, Europa, UTC+01:00“: the zone
 *  in effect chosen again, „behalten“ or Escape in the question. */
export function zoneUnchangedAnnouncement(entry: string, t: Translate): string {
  return t('dialogs.seriesTimeZone.announce.unchanged', { entry });
}

/** „Zeitzone der Serie nicht geändert: {Grund}.“: no answer can be carried
 *  out (20a), and the zone stays. */
export function zoneRefusedAnnouncement(reason: ClockChangeRefusal, zone: string | null, t: Translate): string {
  return t('dialogs.seriesTimeZone.announce.refused', { reason: refusalText(reason, zone, t) });
}

/** Whether an answer moved the series' start. */
export function answerMovesStart(answer: ClockChangeAnswer, start: string): boolean {
  return answer.outcome === 'changed' && Date.parse(answer.start) !== Date.parse(start);
}
