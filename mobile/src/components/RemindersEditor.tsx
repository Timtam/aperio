import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import {
  afterStartLabelKey,
  afterStartOffered,
  beforeEndOffered,
  allDayKindFor,
  allDayKindOption,
  allDayReminderOf,
  atStartLabelKey,
  clockOfText,
  clockText,
  DEFAULT_ALL_DAY_CLOCK,
  DEFAULT_MINUTES_BEFORE,
  isAfterStart,
  isFromEnd,
  minutesBeforeOfAllDay,
  RELATIVE_UNIT_MINUTES,
  reminderKindFor,
  reminderKindOption,
  relativeAmount,
  splitRelativeMinutes,
  type AllDayKindOption,
  type AllDayReminder,
  type RelativeUnit,
  type Reminder,
  type ReminderKindOption,
} from '@aperio/shared';

import { announceAround } from '../a11y/speechHold';
import { useListFocusManager, type RowRefCallback } from '../a11y/useListFocusManager';
import { useThemedStyles, type ThemeColors } from '../theme';
import { SelectFieldButton } from './SelectFieldButton';

// Mobile reminders editor — faithful RN port of the desktop RemindersEditor.
// The local engine supports relative / at-start / after-start / absolute /
// app-start reminders (e-mail is adapter-side; per-reminder sound is the
// desktop-only asset store, so sound is forced null here). An all-day
// appointment's rows are a day and a clock time. Which kind a stored reminder
// shows as, what a chosen kind becomes, and where "after the start" is offered
// is `shared/reminderKinds.ts`, the same rules the desktop uses. Reminders
// cross as cal_core::Reminder[] in the item JSON — no native change.

/**
 * A row of this editor. The task and event editors pass plain reminders; the
 * calendar editor passes the calendar's default entries, which also say where
 * the default lives (`attach`, see `DefaultReminder` in the shared types),
 * and turns on the `placement` field so the user can choose per entry.
 */
export type EditableReminder = Reminder & { attach?: boolean };

type Placement = 'local' | 'attach';
type ClockChoice = 'day_start' | 'clock';

/** A row with its placement set, always written out.
 *
 *  Explicit both ways: the event editor distinguishes "rides on the event"
 *  from "Aperio keeps it", and an absent flag would read as the first. A list
 *  stored before the choice existed has no flag and still reads as local,
 *  which is the behaviour it always had. */
function withPlacement(row: EditableReminder, attach: boolean): EditableReminder {
  return { kind: row.kind, sound: row.sound, attach };
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
  allDay = false,
  afterStartKept = false,
  beforeEndKept = false,
  placement = false,
  placementSurface = 'calendar',
  allowAppStart = true,
}: {
  value: EditableReminder[];
  onChange: (next: EditableReminder[]) => void;
  /** Whether the relative reminder anchors on a task's due date ("Before due")
   *  or an event's start ("Before start"). Mirrors the desktop editor's mode. */
  mode?: 'event' | 'task';
  /** The reminders are an all-day appointment's: each row is a day and a
   *  clock time instead of minutes before a start it does not have. */
  allDay?: boolean;
  /** The calendar keeps a reminder after the start
   *  (`CalendarRow.reminder_capabilities.after_start`). Where it does not,
   *  "after the start" is offered only for a row that stays in Aperio, and a
   *  row that has it anyway says it will not arrive. */
  afterStartKept?: boolean;
  /** The calendar keeps a reminder counted from the end
   *  (`CalendarRow.reminder_capabilities.before_end`). Where it does not,
   *  "before the end" and "at the end" are offered only for a row that stays
   *  in Aperio, and a row that has one anyway says it will not arrive. */
  beforeEndKept?: boolean;
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
  const allDayRows = mode === 'event' && allDay;
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
    // An all-day row starts on the day itself at the day-start time, which
    // every calendar keeps; any other row 15 minutes before.
    const minutes = allDayRows ? 0 : DEFAULT_MINUTES_BEFORE;
    onChange([
      ...value,
      {
        kind: { type: 'relative', minutes_before: minutes },
        sound: null,
        attach: placementSurface === 'event',
      },
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
            allDayRow={allDayRows}
            afterStartKept={afterStartKept}
            beforeEndKept={beforeEndKept}
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
  allDayRow,
  afterStartKept,
  beforeEndKept,
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
  /** An all-day appointment's row: a day and a clock time. */
  allDayRow: boolean;
  /** The calendar keeps "after the start" — see the editor's props. */
  afterStartKept: boolean;
  /** The calendar keeps "before the end" — see the editor's props. */
  beforeEndKept: boolean;
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
  // Only where it arrives; a task list says nothing about it, so a task row
  // shows one it already has but is not offered a new one.
  const afterStart =
    mode === 'event' && afterStartOffered(value, { keptByCalendar: afterStartKept, placement });
  // The same for counting from the end; a task has no end.
  const fromEnd =
    mode === 'event' && beforeEndOffered(value, { keptByCalendar: beforeEndKept, placement });
  // A row that has it where it does not arrive says so, next to the kind.
  const afterStartNotKept = mode === 'event' && isAfterStart(value.kind) && !afterStart;
  const fromEndNotKept = isFromEnd(value.kind) && !fromEnd;
  const notKept = afterStartNotKept || fromEndNotKept;
  const notKeptText = `${t(
    fromEndNotKept ? 'reminders.fromEndNotKept' : 'reminders.afterStartNotKept',
  )}${placement ? ` ${t('reminders.afterStartKeptInAperio')}` : ''}`;
  // Said once when it appears (the placement just changed, another calendar
  // was picked): it sits above the control that changed, so a swipe would
  // not reach it. On opening it is simply next in the row.
  const wasNotKept = useRef(notKept);
  useEffect(() => {
    if (notKept && !wasNotKept.current) announceAround(notKeptText);
    wasNotKept.current = notKept;
  }, [notKept, notKeptText]);

  return (
    <View style={styles.row}>
      <Text ref={rowRef} style={styles.rowLabel} accessibilityRole="text">
        {t('reminders.rowLabel', { n: position })}
      </Text>

      {allDayRow ? (
        <AllDayKindFields
          value={value}
          onChange={onChange}
          position={position}
          afterStart={afterStart}
          allowAppStart={allowAppStart}
        />
      ) : (
        <TimedKindFields
          value={value}
          onChange={onChange}
          mode={mode}
          position={position}
          afterStart={afterStart}
          fromEnd={fromEnd}
          allowAppStart={allowAppStart}
        />
      )}

      {notKept && (
        <Text style={styles.hint} accessibilityRole="text">
          {notKeptText}
        </Text>
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

interface KindFieldsProps {
  value: EditableReminder;
  onChange: (next: EditableReminder) => void;
  position: number;
  /** "After the start" may be chosen for this row. */
  afterStart: boolean;
  /** "Before the end" and "at the end" may be chosen for this row. */
  fromEnd?: boolean;
  allowAppStart: boolean;
}

/** The kind of a row with a start time (or a task's due time), and its
 *  amount and unit. */
function TimedKindFields({
  value,
  onChange,
  mode,
  position,
  afterStart,
  fromEnd = false,
  allowAppStart,
}: KindFieldsProps & { mode: 'event' | 'task' }) {
  const { t } = useTranslation();
  // An email kind (adapter-side) shows as relative until re-picked.
  const kindOption = reminderKindOption(value.kind);
  // Never drop the kind a row already has: a list stored before the offer was
  // withdrawn must still read as what it is, and stay changeable.
  const showAppStart = allowAppStart || kindOption === 'app_start';
  const showAfterStart = afterStart || kindOption === 'after_start';
  const showBeforeEnd = fromEnd || kindOption === 'before_end';
  const showAtEnd = fromEnd || kindOption === 'at_end';
  // "After the end" is not offered; a row from a calendar that has one shows
  // it as what it is.
  const showAfterEnd = kindOption === 'after_end';
  const setMinutes = (minutes_before: number) =>
    onChange({ ...value, kind: { type: 'relative', minutes_before } });
  const setMinutesFromEnd = (minutes_before: number) =>
    onChange({ ...value, kind: { type: 'relative_to_end', minutes_before } });

  return (
    <>
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
          { value: 'at_start', label: t(atStartLabelKey(mode, false)) },
          ...(showAfterStart
            ? [{ value: 'after_start' as const, label: t(afterStartLabelKey(mode)) }]
            : []),
          ...(showBeforeEnd
            ? [{ value: 'before_end' as const, label: t('reminders.kind.beforeEnd') }]
            : []),
          ...(showAtEnd ? [{ value: 'at_end' as const, label: t('reminders.kind.atEnd') }] : []),
          ...(showAfterEnd
            ? [{ value: 'after_end' as const, label: t('reminders.kind.afterEnd') }]
            : []),
          { value: 'absolute', label: t('reminders.kind.absolute') },
          ...(showAppStart
            ? [{ value: 'app_start' as const, label: t('reminders.kind.appStart') }]
            : []),
        ]}
        onChange={(next) => {
          // Choosing the kind the row already has changes nothing: it would
          // turn "30 minutes before" back into the default 15. An email row
          // only SHOWS as "before start"; choosing it makes it one.
          if (next === kindOption && value.kind.type !== 'email') return;
          onChange({ ...value, kind: reminderKindFor(next, new Date()) });
        }}
      />

      {kindOption === 'relative' && value.kind.type === 'relative' && (
        <RelativeFields
          position={position}
          minutes={value.kind.minutes_before}
          onChange={setMinutes}
        />
      )}
      {kindOption === 'after_start' && value.kind.type === 'relative' && (
        <RelativeFields
          position={position}
          minutes={-value.kind.minutes_before}
          onChange={(minutes) => setMinutes(-minutes)}
        />
      )}
      {kindOption === 'before_end' && value.kind.type === 'relative_to_end' && (
        <RelativeFields
          position={position}
          minutes={value.kind.minutes_before}
          onChange={setMinutesFromEnd}
        />
      )}
      {kindOption === 'after_end' && value.kind.type === 'relative_to_end' && (
        <RelativeFields
          position={position}
          minutes={-value.kind.minutes_before}
          onChange={(minutes) => setMinutesFromEnd(-minutes)}
        />
      )}
    </>
  );
}

/**
 * The kind of an all-day appointment's row: the day itself, N days before or
 * after, each at the day-start time or at a clock time of its own.
 */
function AllDayKindFields({ value, onChange, position, afterStart, allowAppStart }: KindFieldsProps) {
  const { t } = useTranslation();
  const kindOption = allDayKindOption(value.kind);
  const showAppStart = allowAppStart || kindOption === 'app_start';
  const showDaysAfter = afterStart || kindOption === 'days_after';
  // Counting from the end is not offered on an all-day appointment; one it
  // has shows as what it is.
  const fromEndOption =
    kindOption === 'before_end' || kindOption === 'at_end' || kindOption === 'after_end';
  const setMinutesFromEnd = (minutes_before: number) =>
    onChange({ ...value, kind: { type: 'relative_to_end', minutes_before } });

  return (
    <>
      <SelectFieldButton<AllDayKindOption>
        label={t('reminders.kindLabel')}
        value={kindOption}
        options={[
          { value: 'on_day', label: t('reminders.kind.onTheDay') },
          { value: 'days_before', label: t('reminders.kind.daysBefore') },
          ...(showDaysAfter
            ? [{ value: 'days_after' as const, label: t('reminders.kind.daysAfter') }]
            : []),
          ...(fromEndOption
            ? [
                {
                  value: kindOption,
                  label: t(
                    kindOption === 'at_end'
                      ? 'reminders.kind.atEnd'
                      : kindOption === 'after_end'
                        ? 'reminders.kind.afterEnd'
                        : 'reminders.kind.beforeEnd',
                  ),
                },
              ]
            : []),
          { value: 'absolute', label: t('reminders.kind.absolute') },
          ...(showAppStart
            ? [{ value: 'app_start' as const, label: t('reminders.kind.appStart') }]
            : []),
        ]}
        onChange={(next) => {
          // The kind it already has changes nothing (see TimedKindFields).
          if (next === kindOption && value.kind.type !== 'email') return;
          onChange({ ...value, kind: allDayKindFor(next, new Date()) });
        }}
      />

      {kindOption === 'before_end' && value.kind.type === 'relative_to_end' && (
        <RelativeFields
          position={position}
          minutes={value.kind.minutes_before}
          onChange={setMinutesFromEnd}
        />
      )}
      {kindOption === 'after_end' && value.kind.type === 'relative_to_end' && (
        <RelativeFields
          position={position}
          minutes={-value.kind.minutes_before}
          onChange={(minutes) => setMinutesFromEnd(-minutes)}
        />
      )}
      {value.kind.type === 'relative' && (
        <AllDayFields
          // A new kind starts its fields afresh (the day-start time).
          key={kindOption}
          position={position}
          reminder={allDayReminderOf(value.kind.minutes_before)}
          // A clock time on the day itself is after its midnight: only where
          // "after the start" arrives, or on a row that has one already.
          clockOnTheDay={afterStart}
          onChange={(next) =>
            onChange({
              ...value,
              kind: { type: 'relative', minutes_before: minutesBeforeOfAllDay(next) },
            })
          }
        />
      )}
    </>
  );
}

/** The own time an all-day row shows: 00:00 for the whole day. */
function shownClock(clock: number | null): string {
  return clockText(clock ?? 0);
}

function AllDayFields({
  position,
  reminder,
  clockOnTheDay,
  onChange,
}: {
  position: number;
  reminder: AllDayReminder;
  clockOnTheDay: boolean;
  onChange: (next: AllDayReminder) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { day, clock } = reminder;
  const rowPrefix = t('reminders.rowLabel', { n: position });
  // Whether the row is set to a time of its own. Its own state, not read off
  // the value: 00:00 is stored as the whole day (it rings at the day-start
  // time), and an edit passes through "0:00" — read off the value, the
  // focused field vanished under the keyboard.
  const [ownTime, setOwnTime] = useState(clock !== null);
  // Once shown, the time choice stays for the row: choosing the day-start
  // time on the day itself must not remove the control that had focus.
  const [clockShown] = useState(clock !== null);
  const offerClock = day !== 0 || clockOnTheDay || clockShown || ownTime;
  // What is typed into the clock field, kept while it is not yet a time.
  const [clockDraft, setClockDraft] = useState(shownClock(clock));
  // The row's time changed from outside (a row removed above this one, so
  // another reminder sits here now): show that one. A draft that already
  // reads as this time is the user's own spelling and stays.
  useEffect(() => {
    if (clock !== null) setOwnTime(true);
    setClockDraft((draft) => ((clockOfText(draft) || null) === clock ? draft : shownClock(clock)));
  }, [clock]);

  return (
    <View style={styles.relativeRow}>
      {day !== 0 && (
        <WholeNumberField
          label={t('reminders.daysLabel')}
          accessibilityLabel={`${rowPrefix} – ${t('reminders.daysLabel')}`}
          value={Math.abs(day)}
          onChange={(n) => onChange({ day: day < 0 ? -n : n, clock })}
        />
      )}
      {offerClock && (
        <SelectFieldButton<ClockChoice>
          label={t('reminders.timeLabel')}
          value={ownTime ? 'clock' : 'day_start'}
          options={[
            { value: 'day_start', label: t('reminders.time.dayStart') },
            { value: 'clock', label: t('reminders.time.clock') },
          ]}
          onChange={(next) => {
            if (next === (ownTime ? 'clock' : 'day_start')) return;
            const own = next === 'clock';
            const nextClock = own ? DEFAULT_ALL_DAY_CLOCK : null;
            setOwnTime(own);
            setClockDraft(shownClock(nextClock));
            onChange({ day, clock: nextClock });
          }}
        />
      )}
      {ownTime && (
        <View style={styles.amountField}>
          <Text style={styles.label}>{t('reminders.clockLabel')}</Text>
          <TextInput
            style={styles.input}
            value={clockDraft}
            onChangeText={(text) => {
              setClockDraft(text);
              const next = clockOfText(text);
              // 00:00 is the whole day, stored so; the field stays.
              if (next !== null) onChange({ day, clock: next === 0 ? null : next });
            }}
            // What could not be read as a time goes back to the time the
            // row keeps, so the field never says one it does not save.
            onBlur={() => setClockDraft(shownClock(clock))}
            placeholder="HH:MM"
            keyboardType="numbers-and-punctuation"
            accessibilityLabel={`${rowPrefix} – ${t('reminders.clockLabel')}`}
            autoCapitalize="none"
            autoCorrect={false}
          />
        </View>
      )}
    </View>
  );
}

/**
 * A whole number from 1, typed. It may stand empty while it is edited — a
 * field that put its 1 straight back made "7" into "17" — and goes back to
 * the kept number when it is left empty or wrong.
 */
function WholeNumberField({
  label,
  accessibilityLabel,
  value,
  onChange,
}: {
  label: string;
  accessibilityLabel: string;
  value: number;
  onChange: (n: number) => void;
}) {
  const styles = useThemedStyles(makeStyles);
  const [draft, setDraft] = useState(String(value));
  useEffect(() => {
    setDraft((d) => (Number(d) === value ? d : String(value)));
  }, [value]);
  return (
    <View style={styles.amountField}>
      <Text style={styles.label}>{label}</Text>
      <TextInput
        style={styles.input}
        value={draft}
        onChangeText={(text) => {
          setDraft(text);
          const n = Math.trunc(Number(text));
          if (text.trim() !== '' && Number.isFinite(n) && n >= 1) onChange(relativeAmount(n));
        }}
        onBlur={() => setDraft(String(value))}
        keyboardType="number-pad"
        accessibilityLabel={accessibilityLabel}
      />
    </View>
  );
}

function RelativeFields({
  position,
  minutes,
  onChange,
}: {
  position: number;
  /** How far from the start, in minutes: before or after, the caller knows. */
  minutes: number;
  onChange: (minutes: number) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { amount, unit } = splitRelativeMinutes(minutes);
  const rowPrefix = t('reminders.rowLabel', { n: position });
  return (
    <View style={styles.relativeRow}>
      <WholeNumberField
        label={t('reminders.amountLabel')}
        accessibilityLabel={`${rowPrefix} – ${t('reminders.amountLabel')}`}
        value={amount}
        onChange={(n) => onChange(n * RELATIVE_UNIT_MINUTES[unit])}
      />
      <SelectFieldButton<RelativeUnit>
        label={t('reminders.unitLabel')}
        value={unit}
        options={[
          { value: 'minutes', label: t('reminders.unit.minutes') },
          { value: 'hours', label: t('reminders.unit.hours') },
          { value: 'days', label: t('reminders.unit.days') },
        ]}
        onChange={(next) => {
          // The unit it already has changes nothing (see the kind field).
          if (next === unit) return;
          onChange(Math.max(1, amount) * RELATIVE_UNIT_MINUTES[next]);
        }}
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
