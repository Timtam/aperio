import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import type {
  TaskAnchor,
  TaskFixedDate,
  TaskFreq,
  TaskPlacement,
  TaskRecurrenceValue,
} from '@aperio/shared';

import { RadioGroup } from './RadioGroup';

// Mobile recurrence editor — faithful RN port of the desktop
// TaskRecurrenceSelector. The value model + backend converters are shared
// (@aperio/shared). The local store reports no capabilities, so (matching the
// desktop's FULL_CAPS fallback) every axis is enabled — no capability gating.
// <select> → RadioGroup, <input type=number> → numeric TextInput, weekday
// checkboxes → toggle Pressables, the fixed-dates list → add/remove rows.

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
}: {
  value: TaskRecurrenceValue;
  onChange: (next: TaskRecurrenceValue) => void;
}) {
  const { t } = useTranslation();
  const update = (patch: Partial<TaskRecurrenceValue>) =>
    onChange({ ...value, ...patch });

  // Backlog placement (DESIGN §9.12) allows interval 0 ("resurface immediately
  // on completion"); scheduled rules stay ≥ 1.
  const minInterval = value.placement === 'BACKLOG' ? 0 : 1;
  const showInterval = value.fixedDates.length === 0;
  const showWeekdays =
    value.freq === 'WEEKLY' &&
    value.placement === 'SCHEDULE' &&
    value.fixedDates.length === 0;
  const showDayOfMonth =
    value.freq === 'MONTHLY' &&
    value.placement === 'SCHEDULE' &&
    value.fixedDates.length === 0;

  return (
    <View style={styles.group}>
      <RadioGroup<TaskFreq>
        label={t('dialogs.task.recurrence.label')}
        value={value.freq}
        options={[
          { value: 'NONE', label: t('dialogs.task.recurrence.none') },
          { value: 'DAILY', label: t('dialogs.task.recurrence.daily') },
          { value: 'WEEKLY', label: t('dialogs.task.recurrence.weekly') },
          { value: 'MONTHLY', label: t('dialogs.task.recurrence.monthly') },
          { value: 'YEARLY', label: t('dialogs.task.recurrence.yearly') },
        ]}
        onChange={(freq) =>
          update({ freq, byDay: freq === 'WEEKLY' ? value.byDay : [] })
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
                style={styles.input}
                value={String(value.interval)}
                onChangeText={(v) =>
                  update({ interval: clampInt(v, minInterval, 365) })
                }
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
                      accessibilityRole="checkbox"
                      accessibilityState={{ checked }}
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
              { value: 'NEVER', label: t('dialogs.task.recurrence.end.never') },
              { value: 'UNTIL', label: t('dialogs.task.recurrence.end.until') },
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
  const setAt = (i: number, patch: Partial<TaskFixedDate>) =>
    onChange(value.map((d, j) => (j === i ? { ...d, ...patch } : d)));
  const removeAt = (i: number) => onChange(value.filter((_, j) => j !== i));
  const add = () => onChange([...value, { month: 1, day: 1 }]);

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

const styles = StyleSheet.create({
  group: { gap: 14 },
  field: { gap: 6 },
  label: { fontSize: 15, fontWeight: '600', color: '#2b3240' },
  hint: { fontSize: 13, color: '#5b6573' },
  input: {
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 12,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  weekdayRow: { flexDirection: 'row', flexWrap: 'wrap', gap: 8 },
  weekday: {
    paddingVertical: 10,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  weekdayChecked: { borderColor: '#1d4ed8', backgroundColor: '#eaf0fd' },
  weekdayText: { fontSize: 15, color: '#10131a' },
  weekdayTextChecked: { fontWeight: '700', color: '#1d3a2f' },
  fixedRow: { flexDirection: 'row', alignItems: 'center', gap: 10 },
  fixedInput: { flex: 1 },
  removeBtn: {
    paddingVertical: 10,
    paddingHorizontal: 16,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  removeBtnText: { fontSize: 20, color: '#b42318', fontWeight: '700' },
  ghostButton: {
    paddingVertical: 12,
    paddingHorizontal: 18,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
    alignItems: 'center',
  },
  ghostButtonText: { fontSize: 16, fontWeight: '600', color: '#1d3a2f' },
  pressed: { backgroundColor: '#e4ebf5' },
});
