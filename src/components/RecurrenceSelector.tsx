import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { DEFAULT_RECURRENCE_CAPABILITIES } from '@aperio/shared';

import type { RecurrenceCapabilities, RecurrenceFreq } from '../api/types';
import {
  buildRRule,
  dayParts,
  deriveMonthlyOptions,
  localDateKey,
  parseRRule,
  recurrenceStartDate,
  resolveAgainstDay,
} from './rrule';
import type { EndMode, Freq, MonthlyOption, ParsedRule } from './rrule';

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
export interface RecurrenceSelectorProps {
  value: string | null;
  onChange: (rrule: string | null) => void;
  /** The day the series starts on, `YYYY-MM-DD` as the form holds it —
   *  drives the derived monthly/yearly options, read off the key by the
   *  core (`cal_core::day_parts`). Optional so non-event callers still
   *  work; today when omitted or not a day (only matters for the derived
   *  defaults, never for an explicit rule). */
  startDay?: string;
  /** Recurrence shapes the target calendar's adapter can store.
   *  Unsupported options are rendered disabled with a hint rather
   *  than hidden, so the user can see the option exists but isn't
   *  available for this source (e.g. EWS has no yearly interval).
   *  Absent → full RFC-5545 support (the local store + most
   *  adapters). */
  capabilities?: RecurrenceCapabilities;
}

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

export function RecurrenceSelector({
  value,
  onChange,
  startDay: startDayProp,
  capabilities,
}: RecurrenceSelectorProps) {
  const { t, i18n } = useTranslation();
  const caps = capabilities ?? DEFAULT_RECURRENCE_CAPABILITIES;
  // The day the derived options come from: the form's, or today where it
  // has none yet (a key the core refuses falls back too). The Date is for
  // naming the month only.
  const startDay = useMemo(
    () =>
      startDayProp && dayParts(startDayProp) !== null
        ? startDayProp
        : localDateKey(new Date()),
    [startDayProp],
  );
  const startDate = useMemo(() => recurrenceStartDate(startDay), [startDay]);
  const parsed = useMemo(() => parseRRule(value), [value]);
  // Resolve start-derived defaults so the monthly/yearly controls
  // always have concrete values to show even for a legacy
  // `FREQ=MONTHLY` rule that carried no day specifier. Then clamp
  // the interval to 1 when the target source can't store one for
  // this frequency (EWS yearly) — keeps the emitted RRULE honest
  // and the disabled input showing the value that'll actually
  // round-trip.
  const rule = useMemo(() => {
    const resolved = resolveAgainstDay(parsed, startDay);
    if (
      resolved.freq !== 'NONE' &&
      !intervalSupported(resolved.freq, caps) &&
      resolved.interval !== 1
    ) {
      return { ...resolved, interval: 1 };
    }
    return resolved;
  }, [parsed, startDay, caps]);

  const update = (next: ParsedRule) => onChange(buildRRule(next));

  const isMonthlyish = rule.freq === 'MONTHLY' || rule.freq === 'YEARLY';
  // Relative ("third Wednesday") options are gated per-frequency:
  // a source may store relative-monthly but not relative-yearly.
  const relativeAllowed =
    rule.freq === 'YEARLY' ? caps.relative_yearly : caps.relative_monthly;
  const monthlyOptions = useMemo(
    () =>
      isMonthlyish
        ? deriveMonthlyOptions(startDay, {
            relative: relativeAllowed,
            // Its own axis, and it was never asked: a source that repeats on
            // the item's own day and cannot take a separate day number still
            // had "on the 14th" offered here, and the value was accepted and
            // dropped on save.
            dayOfMonth: caps.monthly_day_of_month,
          })
        : [],
    [isMonthlyish, startDay, relativeAllowed, caps.monthly_day_of_month],
  );
  const selectedOptionKey = monthlyOptionKey(rule);
  const intervalEnabled = intervalSupported(rule.freq, caps);

  // The interval field keeps its OWN draft text so the user can clear it
  // mid-edit — binding straight to `rule.interval` (a number) snapped an
  // emptied field back to 1, forcing "type the new digit first, then delete
  // the old" (which silently produced e.g. "every 13 weeks"). We only push a
  // number up to the rule when the draft parses to a valid one; an empty /
  // invalid draft leaves the last valid value in place and restores it on
  // blur. Resync from the rule whenever IT changes (freq switch, caps clamp,
  // external edit).
  const [intervalText, setIntervalText] = useState(String(rule.interval));
  useEffect(() => {
    setIntervalText(String(rule.interval));
  }, [rule.interval]);
  const commitInterval = (text: string) => {
    setIntervalText(text);
    const n = Number.parseInt(text, 10);
    if (Number.isFinite(n) && n >= 1) {
      update({ ...rule, interval: Math.min(365, n) });
    }
  };

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
              value={intervalText}
              disabled={!intervalEnabled}
              onChange={(e) => commitInterval(e.target.value)}
              onBlur={() => setIntervalText(String(rule.interval))}
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

