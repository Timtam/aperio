// The clock a series repeats on — this surface's door into
// `cal_core::series_clock`.
//
// A series repeats on the wall clock of the zone it stores, or on UTC. Which
// stored names count as a zone is a rule, and it lives in the core: a name
// tzdata knows, in any ASCII case and untrimmed, that is not one of the eighteen
// names tzdata gives UTC. Everything else — no zone, `Etc/UTC`, `GMT`, a Windows
// name, an offset such as `+05:30` — repeats on UTC, in the views and in the
// reminders alike. Whether THIS device's clock library can then use an accepted
// zone is the shell's own check, after the rule (`expansionZone` in
// recurrence.ts).

/** This surface's door into `cal_core::series_clock`. The empty string stands
 *  for "none", in both directions. */
export interface SeriesClockRules {
  /** The stored name when the series repeats on it; `''` when on UTC. */
  seriesClockZone(tzid: string): string;
  /** tzdata's spelling of the zone a name resolves to; `''` for an unknown name. */
  canonicalZone(name: string): string;
}

let installedRules: SeriesClockRules | null = null;

/** Bind this surface's door into the core. */
export function installSeriesClockRules(rules: SeriesClockRules): void {
  installedRules = rules;
}

function rules(): SeriesClockRules {
  if (installedRules === null) {
    // Loud, not a local fallback: a copy of the rule here is the copy the core
    // exists to replace.
    throw new Error(
      'series clock rules used before installSeriesClockRules() — the surface ' +
        'must bind its door into cal_core::series_clock at startup',
    );
  }
  return installedRules;
}

/**
 * The zone a series repeats on, as the series stores it, or `null` when it
 * repeats on UTC: no zone, a UTC name, or a name tzdata does not know.
 */
export function seriesClockZone(tzid: string | null | undefined): string | null {
  // The door's answer is only "zone or not"; the name handed on is this
  // surface's own, so nothing it stored is rewritten on the way through.
  return rules().seriesClockZone(tzid ?? '') === '' ? null : (tzid as string);
}

/**
 * The zone a tzdata name resolves to, in tzdata's spelling — `Asia/Calcutta`
 * is `Asia/Kolkata`, `UTC` is `Etc/UTC` — or `null` for a name tzdata does not
 * know. ASCII case is ignored; nothing is trimmed.
 */
export function canonicalZone(name: string): string | null {
  const canonical = rules().canonicalZone(name);
  return canonical === '' ? null : canonical;
}
