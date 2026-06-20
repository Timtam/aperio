import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import {
  buildRRule,
  deriveMonthlyOptions,
  JS_DAY_TO_RRULE,
  nthWeekdayOfMonth,
  parseRRule,
} from '@aperio/shared';
import type { EndMode, Freq, MonthlyOption, ParsedRule } from '@aperio/shared';

import { selectableCheckState, selectableRole } from '../a11y/roles';
import { useThemedStyles, type ThemeColors } from '../theme';
import { RadioGroup } from './RadioGroup';

// Mobile EVENT recurrence editor — a faithful RN port of the desktop
// RecurrenceSelector. The RRULE parse/build logic is shared (@aperio/shared
// rrule.ts); the value model is the raw RRULE body string (what
// cal_core::EventRecurrence keeps in `rrule`), `null` = non-recurring.
// <select> → RadioGroup, <input type=number> → numeric TextInput, weekday
// checkboxes → toggle Pressables, <input type=date> → a YYYY-MM-DD field (the
// reliable SR input used elsewhere in the event editor).
//
// No capability gating: the mobile Host omits recurrence_capabilities this
// slice (every calendar reports full RFC-5545 support, the desktop FULL_CAPS
// fallback) — exactly as the mobile TaskRecurrenceSelector. When the Host
// starts stamping caps for external calendars, gating arrives with it.

const WEEKDAYS: { rrule: string; key: string }[] = [
  { rrule: 'MO', key: 'mon' },
  { rrule: 'TU', key: 'tue' },
  { rrule: 'WE', key: 'wed' },
  { rrule: 'TH', key: 'thu' },
  { rrule: 'FR', key: 'fri' },
  { rrule: 'SA', key: 'sat' },
  { rrule: 'SU', key: 'sun' },
];

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
}: {
  value: string | null;
  onChange: (rrule: string | null) => void;
  /** Event start date — drives the derived monthly/yearly options + defaults.
   *  Falls back to today when omitted (only matters for the derived options). */
  start?: Date;
}) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const startKey = start ? start.toDateString() : '';
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const startDate = useMemo(() => start ?? new Date(), [startKey]);
  const parsed = useMemo(() => parseRRule(value), [value]);
  // Fill start-derived defaults so the monthly/yearly controls always show
  // concrete values, even for a legacy FREQ=MONTHLY rule with no day specifier.
  const rule = useMemo(() => resolveAgainstStart(parsed, startDate), [parsed, startDate]);

  const update = (next: ParsedRule) => onChange(buildRRule(next));

  const isMonthlyish = rule.freq === 'MONTHLY' || rule.freq === 'YEARLY';
  const monthlyOptions = useMemo(
    () => (isMonthlyish ? deriveMonthlyOptions(startDate) : []),
    [isMonthlyish, startDate],
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
      <RadioGroup<Freq>
        label={t('dialogs.event.recurrence.label')}
        value={rule.freq}
        options={[
          { value: 'NONE', label: t('dialogs.event.recurrence.none') },
          { value: 'DAILY', label: t('dialogs.event.recurrence.daily') },
          { value: 'WEEKLY', label: t('dialogs.event.recurrence.weekly') },
          { value: 'MONTHLY', label: t('dialogs.event.recurrence.monthly') },
          { value: 'YEARLY', label: t('dialogs.event.recurrence.yearly') },
        ]}
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
              style={styles.input}
              value={String(rule.interval)}
              onChangeText={(v) => update({ ...rule, interval: clampInt(v, 1, 365) })}
              keyboardType="number-pad"
              accessibilityLabel={t('dialogs.event.recurrence.intervalLabel', {
                unit: t(`dialogs.event.recurrence.unit.${rule.freq}`),
              })}
            />
          </View>

          {rule.freq === 'WEEKLY' && (
            <View
              accessibilityLabel={t('dialogs.event.recurrence.weekdays')}
              style={styles.field}
            >
              <Text style={styles.label}>{t('dialogs.event.recurrence.weekdays')}</Text>
              <View style={styles.weekdayRow}>
                {WEEKDAYS.map((d) => {
                  const checked = rule.byDay.includes(d.rrule);
                  return (
                    <Pressable
                      key={d.rrule}
                      accessible
                      accessibilityRole={selectableRole('checkbox')}
                      accessibilityState={selectableCheckState(checked)}
                      accessibilityLabel={t(`dialogs.event.recurrence.short.${d.key}`)}
                      onPress={() =>
                        update({
                          ...rule,
                          byDay: checked
                            ? rule.byDay.filter((x) => x !== d.rrule)
                            : [...rule.byDay, d.rrule],
                        })
                      }
                      style={({ pressed }) => [
                        styles.weekday,
                        checked && styles.weekdayChecked,
                        pressed && styles.pressed,
                      ]}
                    >
                      <Text
                        style={[styles.weekdayText, checked && styles.weekdayTextChecked]}
                        importantForAccessibility="no"
                      >
                        {t(`dialogs.event.recurrence.short.${d.key}`)}
                      </Text>
                    </Pressable>
                  );
                })}
              </View>
            </View>
          )}

          {isMonthlyish && (
            <RadioGroup<string>
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

          <RadioGroup<EndMode>
            label={t('dialogs.event.recurrence.endLabel')}
            value={rule.endMode}
            options={[
              { value: 'NEVER', label: t('dialogs.event.recurrence.end.never') },
              { value: 'COUNT', label: t('dialogs.event.recurrence.end.count') },
              { value: 'UNTIL', label: t('dialogs.event.recurrence.end.until') },
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
    weekdayRow: { flexDirection: 'row', flexWrap: 'wrap', gap: 8 },
    weekday: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    weekdayChecked: { borderColor: c.accent, backgroundColor: c.surfaceSelected },
    weekdayText: { fontSize: 15, color: c.textPrimary },
    weekdayTextChecked: { fontWeight: '700', color: c.link },
    pressed: { backgroundColor: c.surfacePressed },
  });
