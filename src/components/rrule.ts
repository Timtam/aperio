/**
 * Pure RRULE parsing / building logic for the recurrence editors.
 *
 * Speaks the subset of RFC 5545 that 95% of users actually need:
 * FREQ, INTERVAL, COUNT, UNTIL, BYDAY (weekly weekday picker), and —
 * for MONTHLY/YEARLY — a derived "repeat on" picker that offers the
 * absolute (BYMONTHDAY) vs. relative (BYDAY=Nxx, "third Wednesday")
 * shapes computed from the event's start date.
 *
 * Kept free of React so the editor component (`RecurrenceSelector`)
 * stays a pure-component module (Fast Refresh) and so the parse/build
 * round-trip can be unit-tested in isolation.
 */
export type Freq = 'NONE' | 'DAILY' | 'WEEKLY' | 'MONTHLY' | 'YEARLY';

export type EndMode = 'NEVER' | 'COUNT' | 'UNTIL';

/** Which monthly/yearly axis is active: a fixed day-of-month
 *  ("the 15th") or a relative weekday ("the third Wednesday"). */
export type MonthlyMode = 'DAY_OF_MONTH' | 'WEEKDAY';

export interface ParsedRule {
  freq: Freq;
  interval: number;
  byDay: string[];
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
  /** YYYY-MM-DD */
  until: string;
}

const DEFAULT_RULE: ParsedRule = {
  freq: 'NONE',
  interval: 1,
  byDay: [],
  monthlyMode: 'DAY_OF_MONTH',
  byMonthDay: 0,
  relOrdinal: 0,
  relWeekday: '',
  byMonth: 0,
  endMode: 'NEVER',
  count: 10,
  until: '',
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

/** JS `Date.getDay()` (0=Sun) → RRULE weekday code. */
export const JS_DAY_TO_RRULE = ['SU', 'MO', 'TU', 'WE', 'TH', 'FR', 'SA'];

/** How many days in the month containing `d`. */
function daysInMonth(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth() + 1, 0).getDate();
}

/** 1-based ordinal of `d`'s weekday within its month (1st..5th). */
export function nthWeekdayOfMonth(d: Date): number {
  return Math.floor((d.getDate() - 1) / 7) + 1;
}

/** True when `d` sits in the final 7 days of its month — the window
 *  where offering a "last <weekday>" option is meaningful (and more
 *  robust than "fourth/fifth", which some months don't have). */
function isInLastWeek(d: Date): boolean {
  return d.getDate() > daysInMonth(d) - 7;
}

/** Compute the 2-3 monthly/yearly options for a given start date.
 *  `relativeAllowed` (default true) marks the relative weekday
 *  options disabled when the target source can't store them — they
 *  stay visible (greyed) so the user can see the option exists. */
export function deriveMonthlyOptions(
  start: Date,
  relativeAllowed = true,
): MonthlyOption[] {
  const day = start.getDate();
  const weekday = JS_DAY_TO_RRULE[start.getDay()];
  const ordinal = nthWeekdayOfMonth(start);
  const opts: MonthlyOption[] = [
    { key: 'dom', mode: 'DAY_OF_MONTH', day, ordinal: 0, weekday: '' },
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
  if (isInLastWeek(start)) {
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

export function parseRRule(value: string | null): ParsedRule {
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
  if (parts.has('COUNT')) {
    endMode = 'COUNT';
    count = Math.max(1, Number(parts.get('COUNT')) || 1);
  } else if (parts.has('UNTIL')) {
    endMode = 'UNTIL';
    const raw = parts.get('UNTIL') ?? '';
    // Accept both "20260530" and "20260530T235959Z" forms.
    const m = raw.match(/^(\d{4})(\d{2})(\d{2})/);
    if (m) {
      until = `${m[1]}-${m[2]}-${m[3]}`;
    }
  }

  return {
    freq,
    interval,
    byDay,
    monthlyMode,
    byMonthDay,
    relOrdinal,
    relWeekday,
    byMonth,
    endMode,
    count,
    until,
  };
}

export function buildRRule(p: ParsedRule): string | null {
  if (p.freq === 'NONE') return null;
  const out: string[] = [`FREQ=${p.freq}`];
  if (p.interval > 1) out.push(`INTERVAL=${p.interval}`);

  if (p.freq === 'WEEKLY' && p.byDay.length > 0) {
    out.push(`BYDAY=${p.byDay.join(',')}`);
  }

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
  } else if (p.endMode === 'UNTIL' && p.until) {
    // RFC 5545 wants a basic-format UTC timestamp. End-of-day so the
    // last day is included whatever the user's timezone is.
    const [y, m, d] = p.until.split('-');
    if (y && m && d) {
      out.push(`UNTIL=${y}${m}${d}T235959Z`);
    }
  }
  return out.join(';');
}
