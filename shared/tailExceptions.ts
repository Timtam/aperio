// The deleted occurrences a series keeps when "this and all following" writes
// it from the cut on — this surface's door into `cal_core::tail_exceptions`.
//
// A deleted occurrence stays deleted only while an exception names its
// instant, and an edit from the cut on can move every instant: a new date, a
// switch between all-day and a time of day, a new repeat rule. Which deletion
// goes where — by its place in the series (decision 152) or by its day
// (decision 188), and which is dropped — is a rule and lives in the core. The
// shell expands both series and hands each occurrence over with its day,
// because the device's zone is the shell's to know (`tailRecurrenceFor` in
// `seriesSplit.ts`).

import type { TailExceptions, TailExceptionsQuestion } from './types';

export type { TailExceptions, TailExceptionsQuestion };

/** This surface's door into `cal_core::tail_exceptions`: one question, one answer. */
export interface TailExceptionsRules {
  tailExceptionsJson(inputJson: string): string;
}

let installedRules: TailExceptionsRules | null = null;

/** Bind this surface's door into the core. */
export function installTailExceptionsRules(rules: TailExceptionsRules): void {
  installedRules = rules;
}

/** The new series' exceptions for this question. */
export function tailExceptions(question: TailExceptionsQuestion): TailExceptions {
  if (installedRules === null) {
    // Loud, not a local fallback: a copy of the rule here is the copy the core
    // exists to replace.
    throw new Error(
      'tail exception rules used before installTailExceptionsRules() — the ' +
        'surface must bind its door into cal_core::tail_exceptions at startup',
    );
  }
  return JSON.parse(installedRules.tailExceptionsJson(JSON.stringify(question))) as TailExceptions;
}
