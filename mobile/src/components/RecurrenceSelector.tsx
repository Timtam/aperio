import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { StyleSheet, Text, TextInput, View } from 'react-native';

import {
  buildRRule,
  deriveMonthlyOptions,
  JS_DAY_TO_RRULE,
  nthWeekdayOfMonth,
  parseRRule,
} from '@aperio/shared';
import type {
  EndMode,
  Freq,
  MonthlyOption,
  ParsedRule,
  RecurrenceCapabilities,
  RecurrenceFreq,
} from '@aperio/shared';

import { useThemedStyles, type ThemeColors } from '../theme';
import { MultiSelectFieldButton } from './MultiSelectFieldButton';
import { SelectFieldButton } from './SelectFieldButton';

// Mobile EVENT recurrence editor — a faithful RN port of the desktop
// RecurrenceSelector. The RRULE parse/build logic is shared (@aperio/shared
// rrule.ts); the value model is the raw RRULE body string (what
// cal_core::EventRecurrence keeps in `rrule`), `null` = non-recurring.
// <select> → collapsed picker, <input type=number> → numeric TextInput, weekday
// checkboxes → a collapsed weekday multi-select, <input type=date> → a YYYY-MM-DD field (the
// reliable SR input used elsewhere in the event editor).
//
// Capability gating: `capabilities` (the owning calendar's recurrence caps,
// stamped by the Host from the adapter's plugin manifest) FILTERS options the
// backend can't store — an unsupported frequency / interval / weekday picker /
// monthly mode / end-mode is dropped from the picker rather than offered then
// silently lost on save. (The desktop greys them out; on mobile, hiding keeps a
// screen-reader pass short.) Absent → full RFC-5545 (FULL_CAPS).

const WEEKDAYS: { rrule: string; key: string }[] = [
  { rrule: 'MO', key: 'mon' },
  { rrule: 'TU', key: 'tue' },
  { rrule: 'WE', key: 'wed' },
  { rrule: 'TH', key: 'thu' },
  { rrule: 'FR', key: 'fri' },
  { rrule: 'SA', key: 'sat' },
  { rrule: 'SU', key: 'sun' },
];

/** Permissive fallback when no `capabilities` prop is supplied — every axis
 *  supported. Mirrors `plugin_core::RecurrenceCapabilities::default`. */
const FULL_CAPS: RecurrenceCapabilities = {
  frequencies: ['daily', 'weekly', 'monthly', 'yearly'],
  interval_frequencies: ['daily', 'weekly', 'monthly', 'yearly'],
  relative_monthly: true,
  relative_yearly: true,
  weekly_byday: true,
  monthly_day_of_month: true,
  count: true,
  until: true,
};

/** Lowercase a selector `Freq` to the wire `RecurrenceFreq`; `NONE` → null. */
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

/** Parse an integer from a text field, clamped to [min, max]; empty/NaN → min. */
function clampInt(raw: string, min: number, max: number): number {
  const n = Math.trunc(Number(raw));
  if (!Number.isFinite(n)) return min;
  return Math.min(max, Math.max(min, n));
}

export function RecurrenceSelector({
  value,
  onChange,
  start,
  capabilities,
}: {
  value: string | null;
  onChange: (rrule: string | null) => void;
  /** Event start date — drives the derived monthly/yearly options + defaults.
   *  Falls back to today when omitted (only matters for the derived options). */
  start?: Date;
  /** The owning calendar's recurrence capabilities; absent → full RFC-5545. */
  capabilities?: RecurrenceCapabilities;
}) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const caps = capabilities ?? FULL_CAPS;
  const startKey = start ? start.toDateString() : '';
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const startDate = useMemo(() => start ?? new Date(), [startKey]);
  const parsed = useMemo(() => parseRRule(value), [value]);
  // Fill start-derived defaults so the monthly/yearly controls always show
  // concrete values; then clamp the interval to 1 when the source can't store
  // one for this frequency (e.g. EWS yearly) so the emitted RRULE stays honest.
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

  // Draft text for the interval field so the user can CLEAR it mid-edit —
  // binding straight to `rule.interval` snapped an emptied field back to 1,
  // forcing "type the new digit first, then delete the old" (which silently
  // gave e.g. every-13-weeks). Push a number up only when the draft parses to
  // a valid one; an empty/invalid draft keeps the last value and restores it
  // on blur. Resync when the model changes (freq switch, caps clamp).
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

  const isMonthlyish = rule.freq === 'MONTHLY' || rule.freq === 'YEARLY';
  // Relative ("third Wednesday") is gated per-frequency; an explicit
  // day-of-month is gated by its own axis (Vikunja can't store one).
  const relativeAllowed =
    rule.freq === 'YEARLY' ? caps.relative_yearly : caps.relative_monthly;
  const monthlyOptions = useMemo(
    () =>
      isMonthlyish
        ? deriveMonthlyOptions(startDate, relativeAllowed).filter(
            (o) => o.mode !== 'DAY_OF_MONTH' || caps.monthly_day_of_month,
          )
        : [],
    [isMonthlyish, startDate, relativeAllowed, caps.monthly_day_of_month],
  );
  const selectedOptionKey = monthlyOptionKey(rule);

  // Locale-aware names via Intl (no weekday/month dictionary in the i18n files).
  const monthName = intlMonthName(i18n.language, startDate);
  const optionLabel = (opt: MonthlyOption): string => {
    const yearly = rule.freq === 'YEARLY';
    if (opt.mode === 'DAY_OF_MONTH') {
      return yearly
        ? t('dialogs.event.recurrence.by.yearlyOnDay', { day: opt.day, month: monthName })
        : t('dialogs.event.recurrence.by.monthlyOnDay', { day: opt.day });
    }
    const wd = intlWeekdayName(i18n.language, opt.weekday);
    if (opt.ordinal === -1) {
      return yearly
        ? t('dialogs.event.recurrence.by.yearlyOnLastWeekday', { weekday: wd, month: monthName })
        : t('dialogs.event.recurrence.by.monthlyOnLastWeekday', { weekday: wd });
    }
    const position = t(`dialogs.event.recurrence.ordinal.${opt.ordinal}`);
    return yearly
      ? t('dialogs.event.recurrence.by.yearlyOnWeekday', { position, weekday: wd, month: monthName })
      : t('dialogs.event.recurrence.by.monthlyOnWeekday', { position, weekday: wd });
  };

  return (
    <View style={styles.group}>
      <SelectFieldButton<Freq>
        label={t('dialogs.event.recurrence.label')}
        value={rule.freq}
        options={(
          [
            { value: 'NONE', label: t('dialogs.event.recurrence.none') },
            { value: 'DAILY', label: t('dialogs.event.recurrence.daily') },
            { value: 'WEEKLY', label: t('dialogs.event.recurrence.weekly') },
            { value: 'MONTHLY', label: t('dialogs.event.recurrence.monthly') },
            { value: 'YEARLY', label: t('dialogs.event.recurrence.yearly') },
          ] as { value: Freq; label: string }[]
        ).filter((o) => freqSupported(o.value, caps))}
        onChange={(freq) =>
          update({ ...rule, freq, byDay: freq === 'WEEKLY' ? rule.byDay : [] })
        }
      />

      {rule.freq !== 'NONE' && (
        <>
          <View style={styles.field}>
            <Text style={styles.label}>
              {t('dialogs.event.recurrence.intervalLabel', {
                unit: t(`dialogs.event.recurrence.unit.${rule.freq}`),
              })}
            </Text>
            <TextInput
              style={[
                styles.input,
                !intervalSupported(rule.freq, caps) && styles.inputDisabled,
              ]}
              value={intervalText}
              onChangeText={commitInterval}
              onBlur={() => setIntervalText(String(rule.interval))}
              editable={intervalSupported(rule.freq, caps)}
              keyboardType="number-pad"
              accessibilityLabel={t('dialogs.event.recurrence.intervalLabel', {
                unit: t(`dialogs.event.recurrence.unit.${rule.freq}`),
              })}
            />
          </View>

          {rule.freq === 'WEEKLY' && caps.weekly_byday && (
            <MultiSelectFieldButton<string>
              label={t('dialogs.event.recurrence.weekdays')}
              values={rule.byDay}
              options={WEEKDAYS.map((d) => ({
                value: d.rrule,
                label: t(`dialogs.event.recurrence.short.${d.key}`),
              }))}
              emptyLabel={t('dialogs.event.recurrence.weekdaysNone')}
              onChange={(days) =>
                update({
                  ...rule,
                  // Canonical Mo–Su order regardless of toggle order, so the
                  // stored BYDAY stays deterministic.
                  byDay: WEEKDAYS.map((d) => d.rrule).filter((x) =>
                    days.includes(x),
                  ),
                })
              }
            />
          )}

          {isMonthlyish && (
            <SelectFieldButton<string>
              label={t('dialogs.event.recurrence.by.label')}
              value={selectedOptionKey}
              options={monthlyOptions.map((opt) => ({
                value: opt.key,
                label: optionLabel(opt),
              }))}
              onChange={(key) => {
                const opt = monthlyOptions.find((o) => o.key === key);
                if (opt) update(applyMonthlyOption(rule, opt));
              }}
            />
          )}

          <SelectFieldButton<EndMode>
            label={t('dialogs.event.recurrence.endLabel')}
            value={rule.endMode}
            options={[
              { value: 'NEVER' as EndMode, label: t('dialogs.event.recurrence.end.never') },
              ...(caps.count
                ? [{ value: 'COUNT' as EndMode, label: t('dialogs.event.recurrence.end.count') }]
                : []),
              ...(caps.until
                ? [{ value: 'UNTIL' as EndMode, label: t('dialogs.event.recurrence.end.until') }]
                : []),
            ]}
            onChange={(endMode) => update({ ...rule, endMode })}
          />

          {rule.endMode === 'COUNT' && (
            <View style={styles.field}>
              <Text style={styles.label}>{t('dialogs.event.recurrence.countLabel')}</Text>
              <TextInput
                style={styles.input}
                value={String(rule.count)}
                onChangeText={(v) => update({ ...rule, count: clampInt(v, 1, 9999) })}
                keyboardType="number-pad"
                accessibilityLabel={t('dialogs.event.recurrence.countLabel')}
              />
            </View>
          )}

          {rule.endMode === 'UNTIL' && (
            <View style={styles.field}>
              <Text style={styles.label}>{t('dialogs.event.recurrence.untilLabel')}</Text>
              <TextInput
                style={styles.input}
                value={rule.until}
                onChangeText={(until) => update({ ...rule, until })}
                placeholder="YYYY-MM-DD"
                accessibilityLabel={t('dialogs.event.recurrence.untilLabel')}
                autoCapitalize="none"
                autoCorrect={false}
              />
            </View>
          )}
        </>
      )}
    </View>
  );
}

/** Which option key matches the current rule. */
function monthlyOptionKey(rule: ParsedRule): string {
  if (rule.monthlyMode === 'DAY_OF_MONTH') return 'dom';
  return rule.relOrdinal === -1 ? 'last' : 'nth';
}

/** Fold a chosen monthly/yearly option back into the rule. */
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

/** Fill start-derived defaults for any monthly/yearly field a parsed rule left
 *  unspecified (0 / ''), so the controls always render concrete values and
 *  buildRRule emits a complete rule. */
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
  // 2024-01-01 is a Monday; offset from there by the RRULE code's index.
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

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    group: { gap: 14 },
    field: { gap: 6 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    inputDisabled: { opacity: 0.5 },
  });
