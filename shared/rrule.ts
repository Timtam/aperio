/**
 * Pure RRULE parsing / building logic for the recurrence editors — shared by
 * the desktop `RecurrenceSelector` and the mobile event recurrence selector.
 *
 * Speaks the subset of RFC 5545 that 95% of users actually need:
 * FREQ, INTERVAL, COUNT, UNTIL, BYDAY (weekly weekday picker), and —
 * for MONTHLY/YEARLY — a derived "repeat on" picker that offers the
 * absolute (BYMONTHDAY) vs. relative (BYDAY=Nxx, "third Wednesday")
 * shapes computed from the event's start date.
 *
 * Kept free of React/DOM so both editor components stay pure-component modules
 * (Fast Refresh) and so the parse/build round-trip can be unit-tested alone.
 */
import { dayParts } from './dayParts';
import { nthOccurrence, seriesEndOf, untilThroughText, type SeriesEndFacts } from './recurrence';

export type Freq = 'NONE' | 'DAILY' | 'WEEKLY' | 'MONTHLY' | 'YEARLY';

export type EndMode = 'NEVER' | 'COUNT' | 'UNTIL';

/** Which monthly/yearly axis is active: a fixed day-of-month
 *  ("the 15th") or a relative weekday ("the third Wednesday"). */
export type MonthlyMode = 'DAY_OF_MONTH' | 'WEEKDAY';

/**
 * What the end field of a STORED series' rule (a file's included, 136a) reads
 * and writes its day on (DESIGN-series-time-zone.md, stage 12, part 3): the
 * series' start, whether it is all-day, the zone of the clock the form's
 * fields stand on — which the field's label names and the views show the
 * series on — and the device's zone, which only a series of days reads. An end is a day on the series' clock, and only the
 * core says which day and how to spell it (`cal_core::read_series_end`,
 * `cal_core::until_through`). A rule that is no stored series' own is edited
 * without facts: its end stays the typed day, `{D}T235959Z`, until a save
 * spells it on the facts being saved (`settledEnd`, 141a).
 */
export type FieldEndFacts = SeriesEndFacts & { deviceZone: string | null };

/**
 * A typed end date the core gave no spelling for on the series' facts: a
 * start that names no instant, or a library older than the op. The field
 * says so and writes nothing; a `…T235959Z` on those facts could read as
 * another day (D6).
 */
export class UntilNotSpelledError extends Error {
  constructor() {
    super('the repeat end could not be spelled on the series clock');
    this.name = 'UntilNotSpelledError';
  }
}

export interface ParsedRule {
  freq: Freq;
  interval: number;
  byDay: string[];
  /** The rule's WKST, kept as it was written so touching the controls does
   *  not drop it; `''` when the rule names none (RFC 5545: Monday). */
  wkst: string;
  /** Monthly/yearly axis. Only meaningful for MONTHLY/YEARLY. */
  monthlyMode: MonthlyMode;
  /** Day-of-month for DAY_OF_MONTH mode. 0 = derive from start. */
  byMonthDay: number;
  /** Ordinal for WEEKDAY mode: 1..4 or -1 (last). 0 = derive. */
  relOrdinal: number;
  /** Weekday for WEEKDAY mode (RRULE code). '' = derive from start. */
  relWeekday: string;
  /** Month for YEARLY (1-12). 0 = derive from start. */
  byMonth: number;
  endMode: EndMode;
  count: number;
  /** The day the end field shows, `YYYY-MM-DD`; `''` for none. */
  until: string;
  /** The rule's UNTIL as it was written, `''` when it names none. It is
   *  written back unchanged while {@link until} still shows the day it was
   *  parsed with ({@link untilShown}), so touching another control keeps an
   *  end nobody changed, to the second (DESIGN-series-time-zone.md, "Prüfen
   *  und Speichern"): a cut's bound, a date, a provider's own spelling. */
  untilText: string;
  /** The day the field showed when the rule was parsed, `''` for none. */
  untilShown: string;
}

const DEFAULT_RULE: ParsedRule = {
  freq: 'NONE',
  interval: 1,
  byDay: [],
  wkst: '',
  monthlyMode: 'DAY_OF_MONTH',
  byMonthDay: 0,
  relOrdinal: 0,
  relWeekday: '',
  byMonth: 0,
  endMode: 'NEVER',
  count: 10,
  until: '',
  untilText: '',
  untilShown: '',
};

export interface MonthlyOption {
  key: string;
  mode: MonthlyMode;
  /** DAY_OF_MONTH only */
  day: number;
  /** WEEKDAY only */
  ordinal: number;
  weekday: string;
  /** Rendered disabled when the target source can't store this
   *  shape (relative weekday options on a source that doesn't
   *  support relative recurrence). */
  disabled?: boolean;
}


/** Which monthly shapes the target source can store.
 *
 *  Both default to true — the permissive default the manifest itself uses, so
 *  an adapter only spells out what it RESTRICTS.
 *
 *  Two axes rather than one because two different adapters restrict two
 *  different things, and a single flag made one of them unrepresentable. */
export interface MonthlyOptionSupport {
  /** Relative weekday options ("third Wednesday") — `BYDAY=Nxx`. */
  relative?: boolean;
  /** An explicit day of the month — `BYMONTHDAY`. */
  dayOfMonth?: boolean;
}

/** Compute the 2-3 monthly/yearly options for the day a series starts on
 *  (`YYYY-MM-DD`), its parts read off the day key by the core
 *  (`cal_core::day_parts`), not off a Date on the device's clock: the
 *  form's day, whichever clock its fields stand on. No options for a key
 *  the core refuses.
 *
 *  Unsupported options come back marked `disabled` rather than missing, and it
 *  is the SURFACE that decides what to do with that: the desktop greys them so
 *  the user can see the option exists, the mobile picker drops them because an
 *  untappable row is worse on a touch screen than an absent one.
 *
 *  Both axes are decided here, in one place, because they were decided in two
 *  and each surface got a different half right. The desktop honoured
 *  `disabled` and never looked at `monthly_day_of_month`; mobile filtered on
 *  `monthly_day_of_month` and never looked at `disabled`. So a source that
 *  cannot store "third Wednesday" still offered it on the phone, and a source
 *  that cannot store "the 14th" still offered that on the desktop — in both
 *  cases the editor accepted a repeat shape the provider would drop. */
export function deriveMonthlyOptions(
  startDay: string,
  support: MonthlyOptionSupport = {},
): MonthlyOption[] {
  const parts = dayParts(startDay);
  if (parts === null) return [];
  const relativeAllowed = support.relative ?? true;
  const dayOfMonthAllowed = support.dayOfMonth ?? true;
  const day = parts.dayOfMonth;
  const weekday = parts.weekday;
  const ordinal = parts.nthWeekday;
  const opts: MonthlyOption[] = [
    {
      key: 'dom',
      mode: 'DAY_OF_MONTH',
      day,
      ordinal: 0,
      weekday: '',
      disabled: !dayOfMonthAllowed,
    },
    {
      key: 'nth',
      mode: 'WEEKDAY',
      day: 0,
      ordinal,
      weekday,
      disabled: !relativeAllowed,
    },
  ];
  // Only offer "last <weekday>" when the start lands in the final
  // week — otherwise "fourth" and "last" would usually coincide and
  // the extra option is just noise.
  if (parts.inLastWeek) {
    opts.push({
      key: 'last',
      mode: 'WEEKDAY',
      day: 0,
      ordinal: -1,
      weekday,
      disabled: !relativeAllowed,
    });
  }
  return opts;
}

/** Fill the day-derived defaults for any monthly/yearly field a parsed rule
 *  left unspecified (0 / ''), from the day the series starts on
 *  (`YYYY-MM-DD`), so the controls always show concrete values and
 *  `buildRRule` writes a complete rule. One rule for both editors, which
 *  each kept a copy reading a Date on the device's clock. */
export function resolveAgainstDay(parsed: ParsedRule, startDay: string): ParsedRule {
  if (parsed.freq !== 'MONTHLY' && parsed.freq !== 'YEARLY') return parsed;
  const parts = dayParts(startDay);
  if (parts === null) return parsed;
  return {
    ...parsed,
    byMonthDay: parsed.byMonthDay || parts.dayOfMonth,
    relOrdinal: parsed.relOrdinal || parts.nthWeekday,
    relWeekday: parsed.relWeekday || parts.weekday,
    byMonth: parsed.byMonth || parts.month,
  };
}

/**
 * The repeat controls' reading of a rule. With `facts` (a stored series'
 * rule) the end field shows the last day the end reaches on the series'
 * clock, as the core reads it (`seriesEndOf`); the digits stand in only where
 * the core cannot read the rule. Without facts, the end's first eight digits.
 */
export function parseRRule(value: string | null, facts?: FieldEndFacts): ParsedRule {
  if (!value) return { ...DEFAULT_RULE };
  const body = value.toUpperCase().startsWith('RRULE:')
    ? value.slice('RRULE:'.length)
    : value;
  const parts = new Map<string, string>();
  body.split(';').forEach((piece) => {
    const [k, v] = piece.split('=');
    if (k && v) parts.set(k.trim().toUpperCase(), v.trim());
  });

  const freqStr = parts.get('FREQ') ?? 'NONE';
  const freq: Freq =
    freqStr === 'DAILY' ||
    freqStr === 'WEEKLY' ||
    freqStr === 'MONTHLY' ||
    freqStr === 'YEARLY'
      ? freqStr
      : 'NONE';

  const interval = Math.max(1, Number(parts.get('INTERVAL') ?? '1') || 1);
  const wkst = (parts.get('WKST') ?? '').toUpperCase();
  const byDayRaw = (parts.get('BYDAY') ?? '')
    .split(',')
    .map((x) => x.trim())
    .filter(Boolean);

  // Monthly/yearly axis. A BYDAY carrying an ordinal prefix (3WE,
  // -1FR) or paired with BYSETPOS is the relative shape; a plain
  // BYMONTHDAY (or nothing) is the absolute shape.
  let monthlyMode: MonthlyMode = 'DAY_OF_MONTH';
  let byMonthDay = 0;
  let relOrdinal = 0;
  let relWeekday = '';
  let byMonth = 0;
  if (freq === 'MONTHLY' || freq === 'YEARLY') {
    byMonth = Number(parts.get('BYMONTH') ?? '0') || 0;
    const bymonthday = parts.get('BYMONTHDAY');
    const bysetpos = Number(parts.get('BYSETPOS') ?? '0') || 0;
    const ordinalTok = byDayRaw.find((tok) => /^[+-]?\d/.test(tok));
    if (ordinalTok) {
      // Single ordinal token, e.g. "3WE" / "-1FR".
      const m = ordinalTok.match(/^([+-]?\d+)([A-Z]{2})$/);
      if (m) {
        monthlyMode = 'WEEKDAY';
        relOrdinal = Number(m[1]);
        relWeekday = m[2];
      }
    } else if (byDayRaw.length > 0 && bysetpos !== 0) {
      // Composite multi-day BYDAY + BYSETPOS (e.g. last weekday).
      // The simple picker can only show single-weekday options, so
      // collapse to the first weekday — editing a composite rule in
      // this UI necessarily simplifies it.
      monthlyMode = 'WEEKDAY';
      relOrdinal = bysetpos;
      relWeekday = byDayRaw[0].replace(/^[+-]?\d+/, '');
    } else if (bymonthday) {
      monthlyMode = 'DAY_OF_MONTH';
      byMonthDay = Number(bymonthday) || 0;
    }
  }

  // BYDAY for the weekly picker only keeps plain (ordinal-free) codes.
  const byDay =
    freq === 'WEEKLY' ? byDayRaw.filter((tok) => /^[A-Z]{2}$/.test(tok)) : [];

  let endMode: EndMode = 'NEVER';
  let count = DEFAULT_RULE.count;
  let until = DEFAULT_RULE.until;
  let untilText = DEFAULT_RULE.untilText;
  let untilShown = DEFAULT_RULE.untilShown;
  if (parts.has('COUNT')) {
    endMode = 'COUNT';
    count = Math.max(1, Number(parts.get('COUNT')) || 1);
  } else if (parts.has('UNTIL')) {
    endMode = 'UNTIL';
    untilText = parts.get('UNTIL') ?? '';
    until =
      (facts === undefined
        ? undefined
        : seriesEndOf(body, facts, facts.allDay ? facts.deviceZone : null)?.endDay) ??
      untilDayOf(untilText);
    untilShown = until;
  }

  return {
    freq,
    interval,
    byDay,
    wkst,
    monthlyMode,
    byMonthDay,
    relOrdinal,
    relWeekday,
    byMonth,
    endMode,
    count,
    until,
    untilText,
    untilShown,
  };
}

/**
 * The rule the repeat controls write. An end nobody changed goes back byte for
 * byte. A typed day is written, without `facts`, as `{D}T235959Z`: the day
 * state of a rule that is no stored series' own, which its save spells
 * (`settledEnd`). With `facts` it is spelled through that day on the series'
 * clock by the core (`untilThroughText`); where the core gives no spelling,
 * {@link UntilNotSpelledError} is thrown and nothing is written (D6).
 */
export function buildRRule(p: ParsedRule, facts?: FieldEndFacts): string | null {
  if (p.freq === 'NONE') return null;
  const out: string[] = [`FREQ=${p.freq}`];
  if (p.interval > 1) out.push(`INTERVAL=${p.interval}`);

  if (p.freq === 'WEEKLY' && p.byDay.length > 0) {
    out.push(`BYDAY=${p.byDay.join(',')}`);
  }
  // Kept as the rule had it: dropping it would move a biweekly series whose
  // week does not start on Monday (DESIGN-series-time-zone.md, WKST).
  if (p.wkst) out.push(`WKST=${p.wkst}`);

  if (p.freq === 'MONTHLY' || p.freq === 'YEARLY') {
    if (p.freq === 'YEARLY' && p.byMonth) {
      out.push(`BYMONTH=${p.byMonth}`);
    }
    if (p.monthlyMode === 'WEEKDAY' && p.relWeekday) {
      // RRULE's compact ordinal-prefixed form: "3WE", "-1FR".
      out.push(`BYDAY=${p.relOrdinal}${p.relWeekday}`);
    } else if (p.byMonthDay) {
      out.push(`BYMONTHDAY=${p.byMonthDay}`);
    }
  }

  if (p.endMode === 'COUNT') {
    out.push(`COUNT=${p.count}`);
  } else if (p.endMode === 'UNTIL') {
    const day = /^(\d{4})-(\d{2})-(\d{2})$/.exec(p.until);
    if (p.untilText !== '' && (p.until === p.untilShown || !day)) {
      // Nobody changed the end, or what stands in the field is no whole date
      // yet: the end goes back as it came, rather than none at all.
      out.push(`UNTIL=${p.untilText}`);
    } else if (day && facts === undefined) {
      // The typed day, `{D}T235959Z`, which the save spells on the facts it
      // saves (`settledEnd`). Only a whole date is written: a part of one
      // would be a bound no reader can parse.
      out.push(`UNTIL=${day[1]}${day[2]}${day[3]}T235959Z`);
    } else if (day && facts !== undefined) {
      // Through that day on the series' clock, as the core spells it on the
      // rule as it now stands.
      const text = untilThroughText(p.until, out.join(';'), facts);
      if (text === null) throw new UntilNotSpelledError();
      out.push(`UNTIL=${text}`);
    }
  }
  return out.join(';');
}

/** The day an UNTIL as written shows in the end field: its first eight
 *  digits, `YYYY-MM-DD`, or `''` where it starts with none. */
function untilDayOf(untilText: string): string {
  const m = /^(\d{4})(\d{2})(\d{2})/.exec(untilText);
  return m ? `${m[1]}-${m[2]}-${m[3]}` : '';
}

/**
 * The day a newly chosen "until a date" end starts on: the day of the
 * tenth occurrence from `startDay`, where the count's default, "after 10
 * occurrences", ends the same rule. So switching from one to the other
 * keeps the series, and the field never starts empty, which wrote no end
 * at all and sent the control back to "never". `startDay` itself where
 * the rule has fewer occurrences or cannot be read.
 *
 * Counted on UTC from noon of the start day, which keeps the days the
 * rule takes from the start day (`cal_core::day_parts`).
 */
export function defaultUntilDay(rule: ParsedRule, startDay: string): string {
  const counted = buildRRule({ ...rule, endMode: 'COUNT', count: DEFAULT_RULE.count });
  if (counted === null || dayParts(startDay) === null) return startDay;
  const tenth = nthOccurrence(
    { start: `${startDay}T12:00:00Z`, recurrence: { rrule: counted, tzid: null } },
    DEFAULT_RULE.count,
  );
  return tenth === null ? startDay : tenth.slice(0, 10);
}

/** The rule with another end. "Until a date" starts on a date
 *  ({@link defaultUntilDay}): without one no end is written, and the control
 *  fell back to "never" the moment it was chosen. */
export function withEndMode(rule: ParsedRule, endMode: EndMode, startDay: string): ParsedRule {
  return endMode === 'UNTIL' && rule.until === ''
    ? { ...rule, endMode, until: defaultUntilDay(rule, startDay) }
    : { ...rule, endMode };
}

/**
 * The rule the repeat controls would rebuild from what they show, or `null`
 * when they show nothing.
 *
 * The pickers parse a stored rule into their own options and write those
 * options back. For a rule they cannot hold — a lowercase one, `HOURLY`, the
 * last day of the month, a `BYSETPOS` composite — what they show is a
 * DIFFERENT rule than the one stored, and touching any control saves that
 * different rule. A surface can compare this with the stored rule and say
 * what really recurs (decision 87b).
 */
export function rebuiltByPicker(rrule: string | null): string | null {
  return buildRRule(parseRRule(rrule));
}

/** Whether the repeat controls show something other than the stored rule. */
export function pickerMisreadsRule(rrule: string | null | undefined): boolean {
  const stored = rrule?.trim();
  if (!stored) return false;
  const body = stored.replace(/^RRULE:/i, '');
  const rebuilt = rebuiltByPicker(body);
  if (rebuilt === null) return true;
  // Compared as sets of parts, with the defaults RFC 5545 §3.3.10 gives a
  // rule that leaves them out: Apple, Exchange and Google spell `INTERVAL=1`
  // and `WKST=MO` out, the pickers write nothing, and both rules recur on the
  // same days. Counting that as a misreading would put a read-only line above
  // the controls of every ordinary weekly appointment.
  const parts = (rule: string) =>
    rule
      .split(';')
      .map((part) => part.trim().toUpperCase())
      .filter((part) => part !== '' && part !== 'INTERVAL=1' && part !== 'WKST=MO')
      .sort()
      .join(';');
  return parts(body) !== parts(rebuilt);
}
