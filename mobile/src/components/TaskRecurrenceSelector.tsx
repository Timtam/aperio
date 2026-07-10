import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import type {
  RecurrenceCapabilities,
  RecurrenceFreq,
  TaskAnchor,
  TaskFixedDate,
  TaskFreq,
  TaskPlacement,
  TaskRecurrenceValue,
} from '@aperio/shared';

import { selectableCheckState, selectableRole } from '../a11y/roles';
import { useListFocusManager } from '../a11y/useListFocusManager';
import { useThemedStyles, type ThemeColors } from '../theme';
import { RadioGroup } from './RadioGroup';

// Mobile recurrence editor — faithful RN port of the desktop
// TaskRecurrenceSelector. The value model + backend converters are shared
// (@aperio/shared). `capabilities` (the owning list's recurrence caps, stamped
// by the Host from the adapter's plugin manifest) FILTERS options the backend
// can't store — an unsupported frequency / interval / weekday picker /
// day-of-month / end-mode is dropped rather than offered then silently lost on
// save. Absent → full RFC-5545 (FULL_CAPS). <select> → RadioGroup, numbers →
// numeric TextInput, weekday checkboxes → Pressables, fixed-dates → rows.

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

/** Lowercase a `TaskFreq` to the wire `RecurrenceFreq`; `NONE` → null. */
function freqKey(freq: TaskFreq): RecurrenceFreq | null {
  return freq === 'NONE' ? null : (freq.toLowerCase() as RecurrenceFreq);
}

function freqSupported(freq: TaskFreq, caps: RecurrenceCapabilities): boolean {
  const k = freqKey(freq);
  return k === null || caps.frequencies.includes(k);
}

function intervalSupported(freq: TaskFreq, caps: RecurrenceCapabilities): boolean {
  const k = freqKey(freq);
  return k === null || caps.interval_frequencies.includes(k);
}

const WEEKDAYS: { iso: string; key: string }[] = [
  { iso: 'MO', key: 'mon' },
  { iso: 'TU', key: 'tue' },
  { iso: 'WE', key: 'wed' },
  { iso: 'TH', key: 'thu' },
  { iso: 'FR', key: 'fri' },
  { iso: 'SA', key: 'sat' },
  { iso: 'SU', key: 'sun' },
];

/** Parse an integer from a text field, clamped to [min, max]; empty/NaN → min. */
function clampInt(raw: string, min: number, max: number): number {
  const n = Math.trunc(Number(raw));
  if (!Number.isFinite(n)) return min;
  return Math.min(max, Math.max(min, n));
}

export function TaskRecurrenceSelector({
  value,
  onChange,
  capabilities,
}: {
  value: TaskRecurrenceValue;
  onChange: (next: TaskRecurrenceValue) => void;
  /** The owning task list's recurrence capabilities; absent → full RFC-5545. */
  capabilities?: RecurrenceCapabilities;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const caps = capabilities ?? FULL_CAPS;
  const intervalOk = intervalSupported(value.freq, caps);
  const update = (patch: Partial<TaskRecurrenceValue>) =>
    onChange({ ...value, ...patch });

  // Backlog placement (DESIGN §9.12) allows interval 0 ("resurface immediately
  // on completion"); scheduled rules stay ≥ 1.
  const minInterval = value.placement === 'BACKLOG' ? 0 : 1;
  const showInterval = value.fixedDates.length === 0;
  const shownInterval = intervalOk ? value.interval : 1;

  // Draft text for the interval field so the user can CLEAR it mid-edit —
  // binding straight to the number snapped an emptied field back to the
  // minimum, forcing "type the new digit first, then delete the old" (which
  // silently gave e.g. every-13-weeks). Push a number up only when the draft
  // is valid (>= minInterval); an empty/invalid draft keeps the last value and
  // restores it on blur. Resync when the model changes.
  const [intervalText, setIntervalText] = useState(String(shownInterval));
  useEffect(() => {
    setIntervalText(String(shownInterval));
  }, [shownInterval]);
  const commitInterval = (text: string) => {
    setIntervalText(text);
    const n = Number.parseInt(text, 10);
    if (Number.isFinite(n) && n >= minInterval) {
      update({ interval: Math.min(365, n) });
    }
  };
  const showWeekdays =
    value.freq === 'WEEKLY' &&
    value.placement === 'SCHEDULE' &&
    value.fixedDates.length === 0 &&
    caps.weekly_byday;
  const showDayOfMonth =
    value.freq === 'MONTHLY' &&
    value.placement === 'SCHEDULE' &&
    value.fixedDates.length === 0 &&
    caps.monthly_day_of_month;

  return (
    <View style={styles.group}>
      <RadioGroup<TaskFreq>
        label={t('dialogs.task.recurrence.label')}
        value={value.freq}
        options={(
          [
            { value: 'NONE', label: t('dialogs.task.recurrence.none') },
            { value: 'DAILY', label: t('dialogs.task.recurrence.daily') },
            { value: 'WEEKLY', label: t('dialogs.task.recurrence.weekly') },
            { value: 'MONTHLY', label: t('dialogs.task.recurrence.monthly') },
            { value: 'YEARLY', label: t('dialogs.task.recurrence.yearly') },
          ] as { value: TaskFreq; label: string }[]
        ).filter((o) => freqSupported(o.value, caps))}
        onChange={(freq) =>
          update({
            freq,
            byDay: freq === 'WEEKLY' ? value.byDay : [],
            interval: intervalSupported(freq, caps) ? value.interval : 1,
          })
        }
      />

      {value.freq !== 'NONE' && (
        <>
          <RadioGroup<TaskPlacement>
            label={t('dialogs.task.recurrence.placementLabel')}
            value={value.placement}
            options={[
              {
                value: 'SCHEDULE',
                label: t('dialogs.task.recurrence.placement.schedule'),
              },
              {
                value: 'BACKLOG',
                label: t('dialogs.task.recurrence.placement.backlog'),
              },
            ]}
            onChange={(placement) => update({ placement })}
          />

          <RadioGroup<TaskAnchor>
            label={t('dialogs.task.recurrence.anchorLabel')}
            value={value.anchor}
            options={[
              {
                value: 'FROM_DATE',
                label: t('dialogs.task.recurrence.anchor.fromDate'),
              },
              {
                value: 'FROM_COMPLETION',
                label: t('dialogs.task.recurrence.anchor.fromCompletion'),
              },
            ]}
            onChange={(anchor) => update({ anchor })}
          />

          {showInterval && (
            <View style={styles.field}>
              <Text style={styles.label}>
                {t('dialogs.task.recurrence.intervalLabel', {
                  unit: t(`dialogs.task.recurrence.unit.${value.freq}`),
                })}
              </Text>
              <TextInput
                style={[styles.input, !intervalOk && styles.inputDisabled]}
                value={intervalText}
                onChangeText={commitInterval}
                onBlur={() => setIntervalText(String(shownInterval))}
                editable={intervalOk}
                keyboardType="number-pad"
                accessibilityLabel={t('dialogs.task.recurrence.intervalLabel', {
                  unit: t(`dialogs.task.recurrence.unit.${value.freq}`),
                })}
              />
              {minInterval === 0 && (
                <Text style={styles.hint} accessibilityRole="text">
                  {t('dialogs.task.recurrence.backlogIntervalHint')}
                </Text>
              )}
            </View>
          )}

          {showWeekdays && (
            <View
              accessibilityLabel={t('dialogs.task.recurrence.weekdays')}
              style={styles.field}
            >
              <Text style={styles.label}>
                {t('dialogs.task.recurrence.weekdays')}
              </Text>
              <View style={styles.weekdayRow}>
                {WEEKDAYS.map((d) => {
                  const checked = value.byDay.includes(d.iso);
                  return (
                    <Pressable
                      key={d.iso}
                      accessible
                      accessibilityRole={selectableRole('checkbox')}
                      accessibilityState={selectableCheckState(checked)}
                      accessibilityLabel={t(
                        `dialogs.task.recurrence.short.${d.key}`,
                      )}
                      onPress={() =>
                        update({
                          byDay: checked
                            ? value.byDay.filter((x) => x !== d.iso)
                            : [...value.byDay, d.iso],
                        })
                      }
                      style={({ pressed }) => [
                        styles.weekday,
                        checked && styles.weekdayChecked,
                        pressed && styles.pressed,
                      ]}
                    >
                      <Text
                        style={[
                          styles.weekdayText,
                          checked && styles.weekdayTextChecked,
                        ]}
                        importantForAccessibility="no"
                      >
                        {t(`dialogs.task.recurrence.short.${d.key}`)}
                      </Text>
                    </Pressable>
                  );
                })}
              </View>
            </View>
          )}

          {showDayOfMonth && (
            <View style={styles.field}>
              <Text style={styles.label}>
                {t('dialogs.task.recurrence.dayOfMonthLabel')}
              </Text>
              <TextInput
                style={styles.input}
                value={value.dayOfMonth > 0 ? String(value.dayOfMonth) : ''}
                onChangeText={(v) => {
                  const n = Math.trunc(Number(v));
                  update({
                    dayOfMonth: Number.isFinite(n) && n > 0 && n <= 31 ? n : 0,
                  });
                }}
                keyboardType="number-pad"
                placeholder={t('dialogs.task.recurrence.dayOfMonthPlaceholder')}
                accessibilityLabel={t('dialogs.task.recurrence.dayOfMonthLabel')}
              />
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.task.recurrence.dayOfMonthHint')}
              </Text>
            </View>
          )}

          <FixedDatesEditor
            value={value.fixedDates}
            onChange={(fixedDates) => update({ fixedDates })}
          />

          <RadioGroup<'NEVER' | 'UNTIL'>
            label={t('dialogs.task.recurrence.endLabel')}
            value={value.endMode}
            options={[
              {
                value: 'NEVER' as 'NEVER' | 'UNTIL',
                label: t('dialogs.task.recurrence.end.never'),
              },
              ...(caps.until
                ? [
                    {
                      value: 'UNTIL' as 'NEVER' | 'UNTIL',
                      label: t('dialogs.task.recurrence.end.until'),
                    },
                  ]
                : []),
            ]}
            onChange={(endMode) => update({ endMode })}
          />

          {value.endMode === 'UNTIL' && (
            <View style={styles.field}>
              <Text style={styles.label}>
                {t('dialogs.task.recurrence.untilLabel')}
              </Text>
              <TextInput
                style={styles.input}
                value={value.until}
                onChangeText={(until) => update({ until })}
                placeholder="YYYY-MM-DD"
                accessibilityLabel={t('dialogs.task.recurrence.untilLabel')}
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

function FixedDatesEditor({
  value,
  onChange,
}: {
  value: TaskFixedDate[];
  onChange: (next: TaskFixedDate[]) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  // Move SR focus to the new/sibling row after add/remove (RN won't on its own).
  const { registerRow, registerAdd, onAdd, onRemove } = useListFocusManager(
    value.length,
  );
  const setAt = (i: number, patch: Partial<TaskFixedDate>) =>
    onChange(value.map((d, j) => (j === i ? { ...d, ...patch } : d)));
  const removeAt = (i: number) => {
    onRemove(i);
    onChange(value.filter((_, j) => j !== i));
  };
  const add = () => {
    onAdd();
    onChange([...value, { month: 1, day: 1 }]);
  };

  return (
    <View style={styles.field}>
      <Text style={styles.label}>
        {t('dialogs.task.recurrence.fixedDatesLabel')}
      </Text>
      {value.map((d, i) => (
        // Index key: a simple add/remove list with controlled inputs, no
        // reordering, so position is a stable identity.
        <View key={i} style={styles.fixedRow}>
          <TextInput
            ref={registerRow(i)}
            style={[styles.input, styles.fixedInput]}
            value={String(d.month)}
            onChangeText={(v) => setAt(i, { month: clampInt(v, 1, 12) })}
            keyboardType="number-pad"
            accessibilityLabel={t('dialogs.task.recurrence.fixedDateMonth')}
          />
          <TextInput
            style={[styles.input, styles.fixedInput]}
            value={String(d.day)}
            onChangeText={(v) => setAt(i, { day: clampInt(v, 1, 31) })}
            keyboardType="number-pad"
            accessibilityLabel={t('dialogs.task.recurrence.fixedDateDay')}
          />
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.task.recurrence.fixedDateRemove')}
            onPress={() => removeAt(i)}
            style={({ pressed }) => [styles.removeBtn, pressed && styles.pressed]}
          >
            <Text style={styles.removeBtnText} importantForAccessibility="no">
              ×
            </Text>
          </Pressable>
        </View>
      ))}
      <Pressable
        ref={registerAdd}
        accessibilityRole="button"
        accessibilityLabel={t('dialogs.task.recurrence.fixedDateAdd')}
        onPress={add}
        style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
      >
        <Text style={styles.ghostButtonText}>
          {t('dialogs.task.recurrence.fixedDateAdd')}
        </Text>
      </Pressable>
      <Text style={styles.hint} accessibilityRole="text">
        {t('dialogs.task.recurrence.fixedDatesHint')}
      </Text>
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    group: { gap: 14 },
    field: { gap: 6 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    hint: { fontSize: 13, color: c.textSecondary },
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
    fixedRow: { flexDirection: 'row', alignItems: 'center', gap: 10 },
    fixedInput: { flex: 1 },
    removeBtn: {
      paddingVertical: 10,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    removeBtnText: { fontSize: 20, color: c.danger, fontWeight: '700' },
    ghostButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    ghostButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    pressed: { backgroundColor: c.surfacePressed },
  });
