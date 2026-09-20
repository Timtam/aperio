// A repeat rule in words — this surface's door into
// `cal_core::recurrence_summary`, and the renderer that puts the answer into
// the reader's language (decision 84a).
//
// The core decides WHAT the sentence says: which parts it names, in which
// order, and which shapes have no sentence at all. It answers with i18n keys
// and values. This file looks the words up and joins the lists, and has no
// rule of its own — a second rule here would be the rule the core exists to
// replace.

import { formatLongDay, monthName, weekdayName } from './intlNames';
import type { Phrase } from './generated/Phrase';
import type { RecurrenceSummary } from './generated/RecurrenceSummary';
import type { RecurrenceSummaryQuestion } from './generated/RecurrenceSummaryQuestion';
import type { RepeatUnit } from './generated/RepeatUnit';

export type { Phrase, RecurrenceSummary, RecurrenceSummaryQuestion };

/** This surface's door into the core: one question, one answer. */
export interface RecurrenceSummaryRules {
  recurrenceSummaryJson(inputJson: string): string;
}

let installedRules: RecurrenceSummaryRules | null = null;

/** Bind this surface's door into the core. */
export function installRecurrenceSummaryRules(rules: RecurrenceSummaryRules): void {
  installedRules = rules;
}

/**
 * The rule in words, as keys and values.
 *
 * `start` is the day the series starts, `YYYY-MM-DD` on its own clock;
 * `lastDay` the day its last occurrence falls on, when the surface worked it
 * out (see `lastOccurrenceDayKey`). A rule with an end and no `lastDay` says
 * only that it ends.
 */
export function describeRecurrence(question: RecurrenceSummaryQuestion): RecurrenceSummary {
  if (installedRules === null) {
    // Loud, not a local fallback: a copy of the rule here is the copy the core
    // exists to replace.
    throw new Error(
      'recurrence summary rules used before installRecurrenceSummaryRules() — the ' +
        'surface must bind its door into cal_core::recurrence_summary at startup',
    );
  }
  return JSON.parse(
    installedRules.recurrenceSummaryJson(JSON.stringify(question)),
  ) as RecurrenceSummary;
}

type Translate = (key: string, values?: Record<string, unknown>) => string;

/** What the renderer needs: the translator and the reader's language. */
export interface SummaryRenderContext {
  t: Translate;
  language: string;
}

const KEY = 'recurrenceSummary';

/** A list as the language writes one: "a, b and c" / „a, b und c". */
function joinNames(names: string[], { t }: SummaryRenderContext): string {
  if (names.length === 0) return '';
  if (names.length === 1) return names[0];
  const separator = t(`${KEY}.list.separator`);
  const rest = names.slice(0, -1).join(separator);
  return t(`${KEY}.list.and`, { rest, last: names[names.length - 1] });
}

function ordinalWord(ordinal: number, { t }: SummaryRenderContext): string {
  return ordinal === -1
    ? t(`${KEY}.ordinal.last`)
    : t(`${KEY}.ordinal.${ordinal}`);
}

/** One part of the sentence, in words. */
function renderPhrase(phrase: Phrase, context: SummaryRenderContext): string {
  const { t, language } = context;
  const values: Record<string, unknown> = {};
  if (phrase.count !== undefined) values.count = phrase.count;
  if (phrase.weekdays !== undefined && phrase.weekdays.length > 0) {
    values.weekdays = joinNames(
      phrase.weekdays.map((day) => weekdayName(language, day)),
      context,
    );
  }
  if (phrase.weekday !== undefined) values.weekday = weekdayName(language, phrase.weekday);
  if (phrase.ordinal !== undefined) values.ordinal = ordinalWord(phrase.ordinal, context);
  if (phrase.month_days !== undefined && phrase.month_days.length > 0) {
    const days = phrase.month_days.map((day) => t(`${KEY}.day.number`, { day }));
    values.days = joinNames(days, context);
    // A date in the year names one day, not a list.
    values.day = days[0];
  }
  if (phrase.month !== undefined) values.month = monthName(language, phrase.month);
  if (phrase.date !== undefined) values.date = formatLongDay(phrase.date, language);
  if (phrase.set !== undefined) {
    values.set = t(
      `${KEY}.set.${phrase.set === 'weekend_day' ? 'weekendDay' : 'weekday'}`,
    );
  }
  return t(phrase.key, values);
}

/** How often a rule that has no sentence repeats, as an adverb. */
function unitAdverb(unit: RepeatUnit | undefined, { t }: SummaryRenderContext): string | null {
  return unit === undefined || unit === null ? null : t(`${KEY}.unitAdverb.${unit}`);
}

/**
 * The summary as one sentence. An event that does not repeat gives the empty
 * string, so a caller can leave its field out; a rule with no sentence says
 * so, naming how often it repeats when the core could read that much.
 */
export function recurrenceSummaryText(
  summary: RecurrenceSummary,
  context: SummaryRenderContext,
): string {
  const { t } = context;
  switch (summary.outcome) {
    case 'none':
      return '';
    case 'described': {
      const values: Record<string, unknown> = {
        every: renderPhrase(summary.every, context),
      };
      if (summary.on) values.on = renderPhrase(summary.on, context);
      if (summary.end) values.end = renderPhrase(summary.end, context);
      return t(summary.key, values);
    }
    case 'undescribed': {
      const unit = unitAdverb(summary.unit ?? undefined, context);
      return unit === null
        ? t(`${KEY}.undescribed.unknown`)
        : t(`${KEY}.undescribed.known`, { unit });
    }
  }
}

/** The rule in words in one call, for a surface that has both at hand. */
export function recurrenceSentence(
  question: RecurrenceSummaryQuestion,
  context: SummaryRenderContext,
): string {
  return recurrenceSummaryText(describeRecurrence(question), context);
}
