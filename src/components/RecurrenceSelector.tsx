import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import type { RecurrenceCapabilities, RecurrenceFreq } from '../api/types';

/**
 * Recurrence editor for the event dialog.
 *
 * Speaks the subset of RFC 5545 that 95% of users actually need:
 * FREQ, INTERVAL, COUNT, UNTIL, BYDAY (weekly weekday picker), and —
 * for MONTHLY/YEARLY — a derived "repeat on" picker that offers the
 * absolute (BYMONTHDAY) vs. relative (BYDAY=Nxx, "third Wednesday")
 * shapes computed from the event's start date.
 *
 * The relative shapes are NEVER authored from independent ordinal +
 * weekday pickers; instead the start date dictates the 2-3 sensible
 * options (the same UX Google / Apple / Outlook use), so the surface
 * stays small and keyboard-accessible.
 *
 * Communicates with the parent purely through a `value: string | null`
 * (the RRULE body, without the "RRULE:" prefix) and an `onChange`
 * callback. Storing the body matches what `cal-core::EventRecurrence`
 * already keeps in `rrule`. The `start` date is needed to derive the
 * monthly/yearly options and to fill defaults for legacy rules that
 * carry only `FREQ=MONTHLY` with no day specifier.
 */
export type Freq = 'NONE' | 'DAILY' | 'WEEKLY' | 'MONTHLY' | 'YEARLY';

export type EndMode = 'NEVER' | 'COUNT' | 'UNTIL';

/** Which monthly/yearly axis is active: a fixed day-of-month
 *  ("the 15th") or a relative weekday ("the third Wednesday"). */
export type MonthlyMode = 'DAY_OF_MONTH' | 'WEEKDAY';

export interface RecurrenceSelectorProps {
  value: string | null;
  onChange: (rrule: string | null) => void;
  /** Event start date — drives the derived monthly/yearly options.
   *  Optional so non-event callers / older tests still work; falls
   *  back to "today" when omitted (only matters for the derived
   *  defaults, never for an explicit rule). */
  start?: Date;
  /** Recurrence shapes the target calendar's adapter can store.
   *  Unsupported options are rendered disabled with a hint rather
   *  than hidden, so the user can see the option exists but isn't
   *  available for this source (e.g. EWS has no yearly interval).
   *  Absent → full RFC-5545 support (the local store + most
   *  adapters). */
  capabilities?: RecurrenceCapabilities;
}

/** Permissive fallback when no `capabilities` prop is supplied —
 *  every axis fully supported. Mirrors
 *  `plugin_core::RecurrenceCapabilities::default`. */
const FULL_CAPS: RecurrenceCapabilities = {
  frequencies: ['daily', 'weekly', 'monthly', 'yearly'],
  interval_frequencies: ['daily', 'weekly', 'monthly', 'yearly'],
  relative_monthly: true,
  relative_yearly: true,
  weekly_byday: true,
  count: true,
  until: true,
};

/** Lowercase the selector's `Freq` to the wire `RecurrenceFreq`.
 *  `NONE` has no capability axis and returns null. */
function freqKey(freq: Freq): RecurrenceFreq | null {
  return freq === 'NONE' ? null : (freq.toLowerCase() as RecurrenceFreq);
}

function freqSupported(freq: Freq, caps: RecurrenceCapabilities): boolean {
  const k = freqKey(freq);
  return k === null || caps.frequencies.includes(k);
}

function intervalSupported(freq: Freq, caps: RecurrenceCapabilities): boolean {
  const k = freqKey(freq);
  return k === null || caps.interval_frequencies.includes(k);
}

const WEEKDAYS: { iso: number; rrule: string; key: string }[] = [
  { iso: 1, rrule: 'MO', key: 'mon' },
  { iso: 2, rrule: 'TU', key: 'tue' },
  { iso: 3, rrule: 'WE', key: 'wed' },
  { iso: 4, rrule: 'TH', key: 'thu' },
  { iso: 5, rrule: 'FR', key: 'fri' },
  { iso: 6, rrule: 'SA', key: 'sat' },
  { iso: 7, rrule: 'SU', key: 'sun' },
];

/** JS `Date.getDay()` (0=Sun) → RRULE weekday code. */
const JS_DAY_TO_RRULE = ['SU', 'MO', 'TU', 'WE', 'TH', 'FR', 'SA'];

export function RecurrenceSelector({
  value,
  onChange,
  start,
  capabilities,
}: RecurrenceSelectorProps) {
  const { t, i18n } = useTranslation();
  const caps = capabilities ?? FULL_CAPS;
  // Stabilise the fallback Date so the memos below don't re-run every
  // render when `start` is omitted (a fresh `new Date()` each render
  // would otherwise bust their dependency arrays). Keyed on the start
  // day so a real date change still flows through.
  const startKey = start ? start.toDateString() : '';
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const startDate = useMemo(() => start ?? new Date(), [startKey]);
  const parsed = useMemo(() => parseRRule(value), [value]);
  // Resolve start-derived defaults so the monthly/yearly controls
  // always have concrete values to show even for a legacy
  // `FREQ=MONTHLY` rule that carried no day specifier. Then clamp
  // the interval to 1 when the target source can't store one for
  // this frequency (EWS yearly) — keeps the emitted RRULE honest
  // and the disabled input showing the value that'll actually
  // round-trip.
  const rule = useMemo(() => {
    const resolved = resolveAgainstStart(parsed, startDate);
    if (
      resolved.freq !== 'NONE' &&
      !intervalSupported(resolved.freq, caps) &&
      resolved.interval !== 1
    ) {
      return { ...resolved, interval: 1 };
    }
    return resolved;
  }, [parsed, startDate, caps]);

  const update = (next: ParsedRule) => onChange(buildRRule(next));

  const isMonthlyish = rule.freq === 'MONTHLY' || rule.freq === 'YEARLY';
  // Relative ("third Wednesday") options are gated per-frequency:
  // a source may store relative-monthly but not relative-yearly.
  const relativeAllowed =
    rule.freq === 'YEARLY' ? caps.relative_yearly : caps.relative_monthly;
  const monthlyOptions = useMemo(
    () =>
      isMonthlyish ? deriveMonthlyOptions(startDate, relativeAllowed) : [],
    [isMonthlyish, startDate, relativeAllowed],
  );
  const selectedOptionKey = monthlyOptionKey(rule);
  const intervalEnabled = intervalSupported(rule.freq, caps);

  // Locale-aware weekday / month names for the option labels — reuse
  // the browser's Intl rather than carrying a full weekday/month
  // dictionary in the i18n files (the app already leans on Intl for
  // date formatting).
  const weekdayName = (rruleDay: string) =>
    intlWeekdayName(i18n.language, rruleDay);
  const monthName = intlMonthName(i18n.language, startDate);

  const optionLabel = (opt: MonthlyOption): string => {
    const yearly = rule.freq === 'YEARLY';
    if (opt.mode === 'DAY_OF_MONTH') {
      return yearly
        ? t('dialogs.event.recurrence.by.yearlyOnDay', {
            day: opt.day,
            month: monthName,
          })
        : t('dialogs.event.recurrence.by.monthlyOnDay', { day: opt.day });
    }
    const wd = weekdayName(opt.weekday);
    if (opt.ordinal === -1) {
      return yearly
        ? t('dialogs.event.recurrence.by.yearlyOnLastWeekday', {
            weekday: wd,
            month: monthName,
          })
        : t('dialogs.event.recurrence.by.monthlyOnLastWeekday', {
            weekday: wd,
          });
    }
    const position = t(`dialogs.event.recurrence.ordinal.${opt.ordinal}`);
    return yearly
      ? t('dialogs.event.recurrence.by.yearlyOnWeekday', {
          position,
          weekday: wd,
          month: monthName,
        })
      : t('dialogs.event.recurrence.by.monthlyOnWeekday', {
          position,
          weekday: wd,
        });
  };

  return (
    <div className="recurrence">
      <label className="form__field">
        <span className="form__label">
          {t('dialogs.event.recurrence.label')}
        </span>
        <select
          value={rule.freq}
          onChange={(e) => {
            const freq = e.target.value as Freq;
            update({
              ...rule,
              freq,
              // Reset BYDAY when switching away from weekly.
              byDay: freq === 'WEEKLY' ? rule.byDay : [],
            });
          }}
        >
          <option value="NONE">{t('dialogs.event.recurrence.none')}</option>
          <option value="DAILY" disabled={!freqSupported('DAILY', caps)}>
            {t('dialogs.event.recurrence.daily')}
          </option>
          <option value="WEEKLY" disabled={!freqSupported('WEEKLY', caps)}>
            {t('dialogs.event.recurrence.weekly')}
          </option>
          <option value="MONTHLY" disabled={!freqSupported('MONTHLY', caps)}>
            {t('dialogs.event.recurrence.monthly')}
          </option>
          <option value="YEARLY" disabled={!freqSupported('YEARLY', caps)}>
            {t('dialogs.event.recurrence.yearly')}
          </option>
        </select>
      </label>

      {rule.freq !== 'NONE' && (
        <>
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.event.recurrence.intervalLabel', {
                unit: t(`dialogs.event.recurrence.unit.${rule.freq}`),
              })}
            </span>
            <input
              type="number"
              min={1}
              max={365}
              value={rule.interval}
              disabled={!intervalEnabled}
              onChange={(e) =>
                update({
                  ...rule,
                  interval: Math.max(1, Number(e.target.value) || 1),
                })
              }
            />
            {!intervalEnabled && (
              <span className="form__hint recurrence__unsupported">
                {t('dialogs.event.recurrence.unsupportedHint')}
              </span>
            )}
          </label>

          {rule.freq === 'WEEKLY' && (
            <fieldset className="form__field recurrence__weekdays">
              <legend className="form__label">
                {t('dialogs.event.recurrence.weekdays')}
              </legend>
              <div className="recurrence__weekdays-row">
                {WEEKDAYS.map((d) => {
                  const checked = rule.byDay.includes(d.rrule);
                  return (
                    <label key={d.rrule} className="recurrence__weekday">
                      <input
                        type="checkbox"
                        checked={checked}
                        disabled={!caps.weekly_byday}
                        onChange={(e) => {
                          const next = e.target.checked
                            ? [...rule.byDay, d.rrule]
                            : rule.byDay.filter((x) => x !== d.rrule);
                          update({ ...rule, byDay: next });
                        }}
                      />
                      <span>{t(`dialogs.event.recurrence.short.${d.key}`)}</span>
                    </label>
                  );
                })}
              </div>
              {!caps.weekly_byday && (
                <span className="form__hint recurrence__unsupported">
                  {t('dialogs.event.recurrence.unsupportedHint')}
                </span>
              )}
            </fieldset>
          )}

          {isMonthlyish && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.event.recurrence.by.label')}
              </span>
              <select
                value={selectedOptionKey}
                onChange={(e) => {
                  const opt = monthlyOptions.find(
                    (o) => o.key === e.target.value,
                  );
                  if (!opt) return;
                  update(applyMonthlyOption(rule, opt));
                }}
              >
                {monthlyOptions.map((opt) => (
                  <option key={opt.key} value={opt.key} disabled={opt.disabled}>
                    {optionLabel(opt)}
                  </option>
                ))}
              </select>
              {!relativeAllowed && (
                <span className="form__hint recurrence__unsupported">
                  {t('dialogs.event.recurrence.relativeUnsupportedHint')}
                </span>
              )}
            </label>
          )}

          <label className="form__field">
            <span className="form__label">
              {t('dialogs.event.recurrence.endLabel')}
            </span>
            <select
              value={rule.endMode}
              onChange={(e) =>
                update({ ...rule, endMode: e.target.value as EndMode })
              }
            >
              <option value="NEVER">
                {t('dialogs.event.recurrence.end.never')}
              </option>
              <option value="COUNT" disabled={!caps.count}>
                {t('dialogs.event.recurrence.end.count')}
              </option>
              <option value="UNTIL" disabled={!caps.until}>
                {t('dialogs.event.recurrence.end.until')}
              </option>
            </select>
          </label>

          {rule.endMode === 'COUNT' && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.event.recurrence.countLabel')}
              </span>
              <input
                type="number"
                min={1}
                max={9999}
                value={rule.count}
                onChange={(e) =>
                  update({
                    ...rule,
                    count: Math.max(1, Number(e.target.value) || 1),
                  })
                }
              />
            </label>
          )}

          {rule.endMode === 'UNTIL' && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.event.recurrence.untilLabel')}
              </span>
              <input
                type="date"
                value={rule.until}
                onChange={(e) => update({ ...rule, until: e.target.value })}
              />
            </label>
          )}
        </>
      )}
    </div>
  );
}

interface ParsedRule {
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

interface MonthlyOption {
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

/** How many days in the month containing `d`. */
function daysInMonth(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth() + 1, 0).getDate();
}

/** 1-based ordinal of `d`'s weekday within its month (1st..5th). */
function nthWeekdayOfMonth(d: Date): number {
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

/** Which option key matches the current rule. */
function monthlyOptionKey(rule: ParsedRule): string {
  if (rule.monthlyMode === 'DAY_OF_MONTH') return 'dom';
  return rule.relOrdinal === -1 ? 'last' : 'nth';
}

/** Fold a chosen option back into the rule. */
function applyMonthlyOption(rule: ParsedRule, opt: MonthlyOption): ParsedRule {
  if (opt.mode === 'DAY_OF_MONTH') {
    return { ...rule, monthlyMode: 'DAY_OF_MONTH', byMonthDay: opt.day };
  }
  return {
    ...rule,
    monthlyMode: 'WEEKDAY',
    relOrdinal: opt.ordinal,
    relWeekday: opt.weekday,
  };
}

/** Fill start-derived defaults for any monthly/yearly field a parsed
 *  rule left unspecified (0 / ''), so the controls always render
 *  concrete values and `buildRRule` emits a complete rule. */
function resolveAgainstStart(parsed: ParsedRule, start: Date): ParsedRule {
  if (parsed.freq !== 'MONTHLY' && parsed.freq !== 'YEARLY') return parsed;
  return {
    ...parsed,
    byMonthDay: parsed.byMonthDay || start.getDate(),
    relOrdinal: parsed.relOrdinal || nthWeekdayOfMonth(start),
    relWeekday: parsed.relWeekday || JS_DAY_TO_RRULE[start.getDay()],
    byMonth: parsed.byMonth || start.getMonth() + 1,
  };
}

function intlWeekdayName(locale: string, rruleDay: string): string {
  // Pick any date that falls on the target weekday. 2024-01-01 is a
  // Monday; offset from there by the RRULE code's index.
  const order = ['MO', 'TU', 'WE', 'TH', 'FR', 'SA', 'SU'];
  const idx = Math.max(0, order.indexOf(rruleDay));
  const d = new Date(2024, 0, 1 + idx);
  try {
    return new Intl.DateTimeFormat(locale, { weekday: 'long' }).format(d);
  } catch {
    return rruleDay;
  }
}

function intlMonthName(locale: string, date: Date): string {
  try {
    return new Intl.DateTimeFormat(locale, { month: 'long' }).format(date);
  } catch {
    return String(date.getMonth() + 1);
  }
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
