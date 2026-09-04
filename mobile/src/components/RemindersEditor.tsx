import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import type { Reminder } from '@aperio/shared';

import { useListFocusManager, type RowRefCallback } from '../a11y/useListFocusManager';
import { useThemedStyles, type ThemeColors } from '../theme';
import { SelectFieldButton } from './SelectFieldButton';

// Mobile reminders editor — faithful RN port of the desktop RemindersEditor in
// `task` mode. The local engine supports relative / absolute / app-start
// reminders (e-mail is adapter-side; per-reminder sound is the desktop-only
// asset store, so sound is forced null here). Reminders cross as
// cal_core::Reminder[] in the task JSON — no native change.

type ReminderKindOption = 'relative' | 'absolute' | 'app_start';
type RelativeUnit = 'minutes' | 'hours' | 'days';

const UNIT_FACTORS: Record<RelativeUnit, number> = {
  minutes: 1,
  hours: 60,
  days: 60 * 24,
};

/** Decompose `minutes_before` into the largest whole unit + amount. */
function splitRelative(minutes: number): { amount: number; unit: RelativeUnit } {
  if (minutes <= 0) return { amount: 0, unit: 'minutes' };
  if (minutes % UNIT_FACTORS.days === 0) {
    return { amount: minutes / UNIT_FACTORS.days, unit: 'days' };
  }
  if (minutes % UNIT_FACTORS.hours === 0) {
    return { amount: minutes / UNIT_FACTORS.hours, unit: 'hours' };
  }
  return { amount: minutes, unit: 'minutes' };
}

const DEFAULT_RELATIVE: Reminder = {
  kind: { type: 'relative', minutes_before: 15 },
  sound: null,
};

/**
 * A row of this editor. The task and event editors pass plain reminders; the
 * calendar editor passes the calendar's default entries, which also say where
 * the default lives (`attach`, see `DefaultReminder` in the shared types),
 * and turns on the `placement` field so the user can choose per entry.
 */
export type EditableReminder = Reminder & { attach?: boolean };

type Placement = 'local' | 'attach';

/** A row with its placement set, always written out.
 *
 *  Explicit both ways: the event editor distinguishes "rides on the event"
 *  from "Aperio keeps it", and an absent flag would read as the first. A list
 *  stored before the choice existed has no flag and still reads as local,
 *  which is the behaviour it always had. */
function withPlacement(row: EditableReminder, attach: boolean): EditableReminder {
  return { kind: row.kind, sound: row.sound, attach };
}

function defaultsForKind(kind: ReminderKindOption): Reminder['kind'] {
  switch (kind) {
    case 'relative':
      return { type: 'relative', minutes_before: 15 };
    case 'absolute': {
      // Default to the next full hour tomorrow.
      const at = new Date();
      at.setMinutes(0, 0, 0);
      at.setDate(at.getDate() + 1);
      return { type: 'absolute', at: at.toISOString() };
    }
    case 'app_start':
      return { type: 'app_start' };
  }
}

const pad = (n: number) => String(n).padStart(2, '0');

function isoToDateTime(iso: string): { date: string; time: string } {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return { date: '', time: '' };
  return {
    date: `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`,
    time: `${pad(d.getHours())}:${pad(d.getMinutes())}`,
  };
}

/** Combine a local `YYYY-MM-DD` + `HH:MM` into an RFC-3339 UTC instant, or
 *  null when the date is missing/unparseable (the row keeps its last value). */
function dateTimeToIso(date: string, time: string): string | null {
  if (!date.trim()) return null;
  const d = new Date(`${date.trim()}T${(time.trim() || '00:00')}`);
  if (Number.isNaN(d.getTime())) return null;
  return d.toISOString();
}

export function RemindersEditor({
  value,
  onChange,
  mode = 'task',
  placement = false,
  placementSurface = 'calendar',
  allowAppStart = true,
}: {
  value: EditableReminder[];
  onChange: (next: EditableReminder[]) => void;
  /** Whether the relative reminder anchors on a task's due date ("Before due")
   *  or an event's start ("Before start"). Mirrors the desktop editor's mode. */
  mode?: 'event' | 'task';
  /** Show the per-row "Applies" choice — only in Aperio, or attached to new
   *  events. Only the calendar editor turns this on, and only for calendars
   *  whose new appointments can carry reminders at all. */
  placement?: boolean;
  /** Which surface this is, when `placement` is on. It decides what a row
   *  ADDED here starts as — a calendar default starts "only in Aperio", a row
   *  on an appointment starts attached — and how the attached option is
   *  worded: a calendar default reaches NEW appointments, an event row is on
   *  THIS one. */
  placementSurface?: 'calendar' | 'event';
  /** Whether "on next app start" is on offer. The calendar editor turns it
   *  OFF: that kind is host-local by construction — no wire format carries it,
   *  and the collector that fires it reads an entry's OWN reminders from the
   *  local store — so as a calendar default it could never ring for the
   *  external calendars the defaults exist for. A row that already carries the
   *  kind keeps it on the list, so an older list still reads truthfully. */
  allowAppStart?: boolean;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  // Move SR focus to the new/sibling row after add/remove (RN won't on its own).
  const { registerRow, registerAdd, onAdd, onRemove } = useListFocusManager(
    value.length,
  );
  const update = (i: number, next: EditableReminder) => {
    const out = value.slice();
    out[i] = next;
    onChange(out);
  };
  const remove = (i: number) => {
    onRemove(i);
    const out = value.slice();
    out.splice(i, 1);
    onChange(out);
  };
  const add = () => {
    onAdd();
    onChange([
      ...value,
      { ...DEFAULT_RELATIVE, attach: placementSurface === 'event' },
    ]);
  };

  return (
    <View style={styles.field}>
      <Text style={styles.label}>{t('reminders.label')}</Text>
      {value.length === 0 ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t('reminders.empty')}
        </Text>
      ) : (
        value.map((reminder, i) => (
          // Index key: controlled add/remove list, no reordering.
          <ReminderRow
            key={i}
            value={reminder}
            mode={mode}
            position={i + 1}
            placement={placement}
            placementSurface={placementSurface}
            allowAppStart={allowAppStart}
            rowRef={registerRow(i)}
            onChange={(next) => update(i, next)}
            onRemove={() => remove(i)}
          />
        ))
      )}
      <Pressable
        ref={registerAdd}
        accessibilityRole="button"
        accessibilityLabel={t('reminders.add')}
        onPress={add}
        style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
      >
        <Text style={styles.ghostButtonText}>{t('reminders.add')}</Text>
      </Pressable>
    </View>
  );
}

function ReminderRow({
  value,
  onChange,
  onRemove,
  mode,
  position,
  placement,
  placementSurface,
  allowAppStart,
  rowRef,
}: {
  value: EditableReminder;
  onChange: (next: EditableReminder) => void;
  onRemove: () => void;
  mode: 'event' | 'task';
  position: number;
  /** Show the "Applies" choice. */
  placement: boolean;
  /** Which surface it belongs to — see the editor's props. */
  placementSurface: 'calendar' | 'event';
  /** Offer "on next app start" — see the editor's props. */
  allowAppStart: boolean;
  /** Focus target for the SR focus manager (the row's label). */
  rowRef: RowRefCallback;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  // Local-store task reminders are relative/absolute/app_start; an email kind
  // (adapter-side) would show as relative until re-picked — won't occur here.
  const kindOption: ReminderKindOption =
    value.kind.type === 'absolute'
      ? 'absolute'
      : value.kind.type === 'app_start'
        ? 'app_start'
        : 'relative';
  // Never drop the kind a row already has: a list stored before the offer was
  // withdrawn must still read as what it is, and stay changeable.
  const showAppStart = allowAppStart || kindOption === 'app_start';

  return (
    <View style={styles.row}>
      <Text ref={rowRef} style={styles.rowLabel} accessibilityRole="text">
        {t('reminders.rowLabel', { n: position })}
      </Text>

      <SelectFieldButton<ReminderKindOption>
        label={t('reminders.kindLabel')}
        value={kindOption}
        options={[
          {
            value: 'relative',
            label: t(
              mode === 'event'
                ? 'reminders.kind.relativeEvent'
                : 'reminders.kind.relativeTask',
            ),
          },
          { value: 'absolute', label: t('reminders.kind.absolute') },
          ...(showAppStart
            ? [{ value: 'app_start' as const, label: t('reminders.kind.appStart') }]
            : []),
        ]}
        onChange={(next) => onChange({ ...value, kind: defaultsForKind(next) })}
      />

      {value.kind.type === 'relative' && (
        <RelativeFields
          position={position}
          minutes={value.kind.minutes_before}
          onChange={(minutes) =>
            onChange({ ...value, kind: { type: 'relative', minutes_before: minutes } })
          }
        />
      )}

      {value.kind.type === 'absolute' && (
        <AbsoluteFields
          position={position}
          iso={value.kind.at}
          onChange={(iso) => onChange({ ...value, kind: { type: 'absolute', at: iso } })}
        />
      )}

      {value.kind.type === 'app_start' && (
        <Text style={styles.hint} accessibilityRole="text">
          {t(
            allowAppStart
              ? 'reminders.appStartHint'
              : 'reminders.appStartNotForDefaults',
          )}
        </Text>
      )}

      {/* Where a calendar default lives — the overlay ("only in Aperio") or
          written into new appointments. A collapsed picker, like the kind. */}
      {placement && (
        <SelectFieldButton<Placement>
          label={t('reminders.placement.label')}
          value={value.attach ? 'attach' : 'local'}
          options={[
            { value: 'local', label: t('reminders.placement.local') },
            {
              value: 'attach',
              label: t(
                placementSurface === 'event'
                  ? 'reminders.placement.attachEvent'
                  : 'reminders.placement.attach',
              ),
            },
          ]}
          onChange={(next) => onChange(withPlacement(value, next === 'attach'))}
        />
      )}

      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('reminders.removeAria', { n: position })}
        onPress={onRemove}
        style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
      >
        <Text style={styles.ghostButtonText}>{t('reminders.remove')}</Text>
      </Pressable>
    </View>
  );
}

function RelativeFields({
  position,
  minutes,
  onChange,
}: {
  position: number;
  minutes: number;
  onChange: (minutes: number) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { amount, unit } = splitRelative(minutes);
  const rowPrefix = t('reminders.rowLabel', { n: position });
  return (
    <View style={styles.relativeRow}>
      <View style={styles.amountField}>
        <Text style={styles.label}>{t('reminders.amountLabel')}</Text>
        <TextInput
          style={styles.input}
          value={String(amount)}
          onChangeText={(v) => {
            const n = Math.trunc(Number(v));
            const safe = Number.isFinite(n) && n > 0 ? n : 1;
            onChange(safe * UNIT_FACTORS[unit]);
          }}
          keyboardType="number-pad"
          accessibilityLabel={`${rowPrefix} – ${t('reminders.amountLabel')}`}
        />
      </View>
      <SelectFieldButton<RelativeUnit>
        label={t('reminders.unitLabel')}
        value={unit}
        options={[
          { value: 'minutes', label: t('reminders.unit.minutes') },
          { value: 'hours', label: t('reminders.unit.hours') },
          { value: 'days', label: t('reminders.unit.days') },
        ]}
        onChange={(next) => onChange(Math.max(1, amount) * UNIT_FACTORS[next])}
      />
    </View>
  );
}

function AbsoluteFields({
  position,
  iso,
  onChange,
}: {
  position: number;
  iso: string;
  onChange: (iso: string) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const initial = isoToDateTime(iso);
  const [date, setDate] = useState(initial.date);
  const [time, setTime] = useState(initial.time);
  const apply = (d: string, tm: string) => {
    const next = dateTimeToIso(d, tm);
    if (next) onChange(next);
  };
  const rowPrefix = t('reminders.rowLabel', { n: position });
  return (
    <View style={styles.field}>
      <Text style={styles.label}>{t('reminders.absoluteAtLabel')}</Text>
      <TextInput
        style={styles.input}
        value={date}
        onChangeText={(d) => {
          setDate(d);
          apply(d, time);
        }}
        placeholder="YYYY-MM-DD"
        accessibilityLabel={`${rowPrefix} – ${t('reminders.absoluteAtLabel')} – ${t('dialogs.task.fields.scheduled.date')}`}
        autoCapitalize="none"
        autoCorrect={false}
      />
      <TextInput
        style={styles.input}
        value={time}
        onChangeText={(tm) => {
          setTime(tm);
          apply(date, tm);
        }}
        placeholder="HH:MM"
        accessibilityLabel={`${rowPrefix} – ${t('reminders.absoluteAtLabel')} – ${t('dialogs.task.fields.scheduled.time')}`}
        autoCapitalize="none"
        autoCorrect={false}
      />
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    field: { gap: 6 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    hint: { fontSize: 13, color: c.textSecondary },
    row: {
      gap: 10,
      padding: 12,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    rowLabel: { fontSize: 14, fontWeight: '700', color: c.textPrimary },
    relativeRow: { gap: 10 },
    amountField: { gap: 6 },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.background,
    },
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
