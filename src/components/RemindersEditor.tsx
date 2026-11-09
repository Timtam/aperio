import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

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
  type ReminderKindOption,
} from '@aperio/shared';

import type { Reminder } from '../api/types';
import { useAnnouncer } from '../a11y/announcerContext';
import { SoundPicker } from './SoundPicker';

/**
 * Editor for the `reminders` field on an event or task (DESIGN.md
 * section 14).
 *
 * The kinds the local engine supports (the rules in `shared/reminderKinds.ts`):
 *  - **Relative** — fire N minutes / hours / days before the event
 *    start or task due date.
 *  - **At start** — fire when the appointment starts or the task is due.
 *    Stored as relative, 0 minutes before.
 *  - **After start** — fire N minutes / hours / days after the start
 *    (negative minutes), only where it arrives (TODO B7 222).
 *  - **Absolute** — fire at a fixed wall-clock time.
 *  - **At app start** — fire on the next launch after the due time.
 *
 * An all-day appointment's rows read differently: a day (the day itself, N
 * days before, N days after) and a clock time (the day-start time or one of
 * its own), the reading the planner gives its own reminders.
 *
 * E-mail reminders are an adapter-side feature and land with Phase 6.
 * Per-reminder sound overrides land with the asset store in the sync
 * wave.
 *
 * The component is uncontrolled by design: the parent dialog owns the
 * `reminders` array and rebuilds it on every change. Reminders are a
 * short list (typically 0–3 entries) so the cost of cloning is
 * negligible and the parent gets full Undo-via-form-reset for free.
 */
/**
 * A row of this editor. Event and task dialogs pass plain reminders; the
 * calendar-defaults panel passes entries that also say where the default
 * lives (`attach`, see `DefaultReminder` in the shared types) and turns on
 * the `placement` field so the user can choose per entry.
 */
export type EditableReminder = Reminder & { attach?: boolean };

/** A row with its placement set, always written out.
 *
 *  Explicit both ways: the event editor distinguishes "rides on the event"
 *  from "Aperio keeps it", and an absent flag would read as the first. A list
 *  stored before the choice existed has no flag and still reads as local,
 *  which is the behaviour it always had. */
function withPlacement(row: EditableReminder, attach: boolean): EditableReminder {
  return { kind: row.kind, sound: row.sound, attach };
}

export interface RemindersEditorProps {
  value: EditableReminder[];
  onChange: (next: EditableReminder[]) => void;
  /**
   * Whether this editor is sitting inside an event or task dialog —
   * affects only the help text (events fire "before start", tasks
   * "before due").
   */
  mode: 'event' | 'task';
  /**
   * The reminders are an all-day appointment's: each row is a day and a clock
   * time instead of minutes before a start it does not have.
   */
  allDay?: boolean;
  /**
   * The calendar keeps a reminder after the start
   * (`CalendarRow.reminder_capabilities.after_start`). Where it does not, "after
   * the start" is offered only for a row that stays in Aperio, and a row that
   * has it anyway says it will not arrive.
   */
  afterStartKept?: boolean;
  /**
   * The calendar keeps a reminder counted from the end
   * (`CalendarRow.reminder_capabilities.before_end`). Where it does not,
   * "before the end" and "at the end" are offered only for a row that stays
   * in Aperio, and a row that has one anyway says it will not arrive.
   */
  beforeEndKept?: boolean;
  /**
   * Show the per-row "Applies" choice — only in Aperio, or attached to new
   * events. Only the calendar-defaults panel turns this on, and only for
   * calendars whose new appointments can carry reminders at all.
   */
  placement?: boolean;
  /**
   * Which surface this is, when `placement` is on. It decides what a row ADDED
   * here starts as — a calendar default starts "only in Aperio", a row on an
   * appointment starts attached, which is what a reminder on an appointment
   * has always been — and how the attached option is worded: a calendar
   * default reaches NEW appointments, an event row is on THIS one.
   */
  placementSurface?: 'calendar' | 'event';
  /**
   * Whether "on next app start" is on offer. The calendar-defaults surfaces
   * turn it OFF: that kind is host-local by construction — no wire format
   * carries it, and the collector that fires it reads an entry's OWN
   * reminders from the local store — so as a calendar default it could never
   * ring for the external calendars the defaults exist for. Offering a
   * setting that saves and then does nothing is worse than not offering it.
   * A row that already carries the kind keeps it on the list, so an older
   * list still reads truthfully and can be changed.
   */
  allowAppStart?: boolean;
}

export function RemindersEditor({
  value,
  onChange,
  mode,
  allDay = false,
  afterStartKept = false,
  beforeEndKept = false,
  placement = false,
  placementSurface = 'calendar',
  allowAppStart = true,
}: RemindersEditorProps) {
  const { t } = useTranslation();
  const allDayRows = mode === 'event' && allDay;

  const update = useCallback(
    (i: number, next: EditableReminder) => {
      const out = value.slice();
      out[i] = next;
      onChange(out);
    },
    [value, onChange],
  );

  const remove = useCallback(
    (i: number) => {
      const out = value.slice();
      out.splice(i, 1);
      onChange(out);
    },
    [value, onChange],
  );

  const add = useCallback(() => {
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
  }, [value, onChange, placementSurface, allDayRows]);

  return (
    <fieldset className="form__field reminders">
      <legend className="form__label">{t('reminders.label')}</legend>

      {value.length === 0 ? (
        <p className="form__hint">{t('reminders.empty')}</p>
      ) : (
        <ul className="reminders__list" role="list">
          {value.map((reminder, i) => (
            <li key={i} className="reminders__row">
              <ReminderRow
                value={reminder}
                onChange={(next) => update(i, next)}
                onRemove={() => remove(i)}
                mode={mode}
                allDayRow={allDayRows}
                afterStartKept={afterStartKept}
                beforeEndKept={beforeEndKept}
                position={i + 1}
                placement={placement}
                placementSurface={placementSurface}
                allowAppStart={allowAppStart}
              />
            </li>
          ))}
        </ul>
      )}

      <button
        type="button"
        className="form__action reminders__add"
        onClick={add}
      >
        {t('reminders.add')}
      </button>
    </fieldset>
  );
}

interface ReminderRowProps {
  value: EditableReminder;
  onChange: (next: EditableReminder) => void;
  onRemove: () => void;
  mode: 'event' | 'task';
  /** An all-day appointment's row: a day and a clock time. */
  allDayRow: boolean;
  /** The calendar keeps "after the start" — see `RemindersEditorProps`. */
  afterStartKept: boolean;
  /** The calendar keeps "before the end" — see `RemindersEditorProps`. */
  beforeEndKept: boolean;
  /** 1-based index used in the row's aria-label. */
  position: number;
  /** Show the "Applies" choice. */
  placement: boolean;
  /** Which surface it belongs to — see `RemindersEditorProps`. */
  placementSurface: 'calendar' | 'event';
  /** Offer "on next app start" — see `RemindersEditorProps`. */
  allowAppStart: boolean;
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
}: ReminderRowProps) {
  const { t } = useTranslation();
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
  const hintId = useId();
  // NVDA in focus mode never stops on a static paragraph inside the dialog's
  // role=application: the hint describes the row's selects, and is said once
  // when it appears (the placement just changed, another calendar was picked)
  // — not on opening, where the selects' description carries it.
  const announce = useAnnouncer();
  const wasNotKept = useRef(notKept);
  useEffect(() => {
    if (notKept && !wasNotKept.current) announce(notKeptText);
    wasNotKept.current = notKept;
  }, [notKept, notKeptText, announce]);
  const describedBy = notKept ? hintId : undefined;

  return (
    <div
      className="reminders__row-inner"
      role="group"
      aria-label={t('reminders.rowLabel', { n: position })}
    >
      {allDayRow ? (
        <AllDayKindFields
          value={value}
          onChange={onChange}
          afterStart={afterStart}
          allowAppStart={allowAppStart}
          describedBy={describedBy}
        />
      ) : (
        <TimedKindFields
          value={value}
          onChange={onChange}
          mode={mode}
          afterStart={afterStart}
          fromEnd={fromEnd}
          allowAppStart={allowAppStart}
          describedBy={describedBy}
        />
      )}

      {notKept && (
        <p className="form__hint" id={hintId}>
          {notKeptText}
        </p>
      )}

      {value.kind.type === 'absolute' && (
        <AbsoluteField
          iso={value.kind.at}
          onChange={(iso) =>
            onChange({ ...value, kind: { type: 'absolute', at: iso } })
          }
        />
      )}

      {value.kind.type === 'app_start' && (
        <p className="form__hint">
          {t(
            allowAppStart
              ? 'reminders.appStartHint'
              : 'reminders.appStartNotForDefaults',
          )}
        </p>
      )}

      {/* §14.4 per-reminder sound override — the most specific level.
          `null` inherits the item/container/global default. Hidden for
          app_start (host-local catch-up) and email (adapter-delivered),
          where a per-reminder local sound doesn't apply. */}
      {(value.kind.type === 'relative' ||
        value.kind.type === 'relative_to_end' ||
        value.kind.type === 'absolute') && (
        <SoundPicker
          value={value.sound}
          onChange={(sound) => onChange({ ...value, sound })}
          compact
        />
      )}

      {/* Where a calendar default lives — the overlay ("only in Aperio") or
          written into new appointments. A native select, like the kind. */}
      {placement && (
        <label className="form__field">
          <span className="form__label">{t('reminders.placement.label')}</span>
          <select
            value={value.attach ? 'attach' : 'local'}
            aria-describedby={describedBy}
            onChange={(e) =>
              onChange(withPlacement(value, e.target.value === 'attach'))
            }
          >
            <option value="local">{t('reminders.placement.local')}</option>
            <option value="attach">
              {t(
                placementSurface === 'event'
                  ? 'reminders.placement.attachEvent'
                  : 'reminders.placement.attach',
              )}
            </option>
          </select>
        </label>
      )}

      <button
        type="button"
        className="form__action form__action--danger reminders__remove"
        onClick={onRemove}
        aria-label={t('reminders.removeAria', { n: position })}
      >
        {t('reminders.remove')}
      </button>
    </div>
  );
}

interface KindFieldsProps {
  value: EditableReminder;
  onChange: (next: EditableReminder) => void;
  /** "After the start" may be chosen for this row. */
  afterStart: boolean;
  /** "Before the end" and "at the end" may be chosen for this row. */
  fromEnd?: boolean;
  allowAppStart: boolean;
  /** The id of the row's "will not arrive" hint, while it shows. */
  describedBy?: string;
}

/** The kind of a row with a start time (or a task's due time), and its
 *  amount and unit. */
function TimedKindFields({
  value,
  onChange,
  mode,
  afterStart,
  fromEnd = false,
  allowAppStart,
  describedBy,
}: KindFieldsProps & { mode: 'event' | 'task' }) {
  const { t } = useTranslation();
  const option = reminderKindOption(value.kind);
  // Never drop the kind a row already has: a list stored before the offer was
  // withdrawn must still read as what it is, and stay changeable.
  const showAppStart = allowAppStart || option === 'app_start';
  const showAfterStart = afterStart || option === 'after_start';
  const showBeforeEnd = fromEnd || option === 'before_end';
  const showAtEnd = fromEnd || option === 'at_end';
  // "After the end" is not offered; a row from a calendar that has one shows
  // it as what it is.
  const showAfterEnd = option === 'after_end';

  const setKind = (next: ReminderKindOption) => {
    if (next === option) return;
    onChange({ ...value, kind: reminderKindFor(next, new Date()) });
  };
  const setMinutes = (minutes_before: number) =>
    onChange({ ...value, kind: { type: 'relative', minutes_before } });
  const setMinutesFromEnd = (minutes_before: number) =>
    onChange({ ...value, kind: { type: 'relative_to_end', minutes_before } });

  return (
    <>
      <label className="form__field">
        <span className="form__label">{t('reminders.kindLabel')}</span>
        <select
          value={option}
          aria-describedby={describedBy}
          onChange={(e) => setKind(e.target.value as ReminderKindOption)}
        >
          <option value="relative">
            {t(
              mode === 'event'
                ? 'reminders.kind.relativeEvent'
                : 'reminders.kind.relativeTask',
            )}
          </option>
          <option value="at_start">{t(atStartLabelKey(mode, false))}</option>
          {showAfterStart && (
            <option value="after_start">{t(afterStartLabelKey(mode))}</option>
          )}
          {showBeforeEnd && (
            <option value="before_end">{t('reminders.kind.beforeEnd')}</option>
          )}
          {showAtEnd && <option value="at_end">{t('reminders.kind.atEnd')}</option>}
          {showAfterEnd && (
            <option value="after_end">{t('reminders.kind.afterEnd')}</option>
          )}
          <option value="absolute">{t('reminders.kind.absolute')}</option>
          {showAppStart && (
            <option value="app_start">{t('reminders.kind.appStart')}</option>
          )}
        </select>
      </label>

      {option === 'relative' && value.kind.type === 'relative' && (
        <RelativeFields minutes={value.kind.minutes_before} onChange={setMinutes} />
      )}
      {option === 'after_start' && value.kind.type === 'relative' && (
        <RelativeFields
          minutes={-value.kind.minutes_before}
          onChange={(minutes) => setMinutes(-minutes)}
        />
      )}
      {option === 'before_end' && value.kind.type === 'relative_to_end' && (
        <RelativeFields minutes={value.kind.minutes_before} onChange={setMinutesFromEnd} />
      )}
      {option === 'after_end' && value.kind.type === 'relative_to_end' && (
        <RelativeFields
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
function AllDayKindFields({
  value,
  onChange,
  afterStart,
  allowAppStart,
  describedBy,
}: KindFieldsProps) {
  const { t } = useTranslation();
  const option = allDayKindOption(value.kind);
  const showAppStart = allowAppStart || option === 'app_start';
  const showDaysAfter = afterStart || option === 'days_after';
  // Not offered on an all-day appointment; one it has shows as what it is.
  const showBeforeEnd = option === 'before_end';

  const setKind = (next: AllDayKindOption) => {
    if (next === option) return;
    onChange({ ...value, kind: allDayKindFor(next, new Date()) });
  };

  return (
    <>
      <label className="form__field">
        <span className="form__label">{t('reminders.kindLabel')}</span>
        <select
          value={option}
          aria-describedby={describedBy}
          onChange={(e) => setKind(e.target.value as AllDayKindOption)}
        >
          <option value="on_day">{t('reminders.kind.onTheDay')}</option>
          <option value="days_before">{t('reminders.kind.daysBefore')}</option>
          {showDaysAfter && (
            <option value="days_after">{t('reminders.kind.daysAfter')}</option>
          )}
          {showBeforeEnd && (
            <option value="before_end">{t('reminders.kind.beforeEnd')}</option>
          )}
          <option value="absolute">{t('reminders.kind.absolute')}</option>
          {showAppStart && (
            <option value="app_start">{t('reminders.kind.appStart')}</option>
          )}
        </select>
      </label>

      {value.kind.type === 'relative_to_end' && (
        <RelativeFields
          minutes={value.kind.minutes_before}
          onChange={(minutes_before) =>
            onChange({ ...value, kind: { type: 'relative_to_end', minutes_before } })
          }
        />
      )}
      {value.kind.type === 'relative' && (
        <AllDayFields
          // A new kind starts its fields afresh (the day-start time).
          key={option}
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

interface AllDayFieldsProps {
  reminder: AllDayReminder;
  clockOnTheDay: boolean;
  onChange: (next: AllDayReminder) => void;
}

function AllDayFields({ reminder, clockOnTheDay, onChange }: AllDayFieldsProps) {
  const { t } = useTranslation();
  const { day, clock } = reminder;
  // Whether the row is set to a time of its own. Its own state, not read off
  // the value: 00:00 is stored as the whole day (it rings at the day-start
  // time), and an hour typed with its leading zero passes through 00:00 —
  // read off the value, the focused field vanished under the keyboard.
  const [ownTime, setOwnTime] = useState(clock !== null);
  useEffect(() => {
    if (clock !== null) setOwnTime(true);
  }, [clock]);
  // Once shown, the time choice stays for the row: choosing the day-start
  // time on the day itself must not remove the control that has focus.
  const [clockShown] = useState(clock !== null);
  const offerClock = day !== 0 || clockOnTheDay || clockShown || ownTime;

  const setDays = (typed: number) => {
    const n = relativeAmount(typed);
    onChange({ day: day < 0 ? -n : n, clock });
  };

  return (
    <div className="form__row reminders__relative">
      {day !== 0 && (
        <label className="form__field">
          <span className="form__label">{t('reminders.daysLabel')}</span>
          <input
            type="number"
            min={1}
            max={999}
            value={Math.abs(day)}
            onChange={(e) => setDays(Number(e.target.value))}
          />
        </label>
      )}
      {offerClock && (
        <label className="form__field">
          <span className="form__label">{t('reminders.timeLabel')}</span>
          <select
            value={ownTime ? 'clock' : 'day_start'}
            onChange={(e) => {
              const own = e.target.value === 'clock';
              setOwnTime(own);
              onChange({ day, clock: own ? DEFAULT_ALL_DAY_CLOCK : null });
            }}
          >
            <option value="day_start">{t('reminders.time.dayStart')}</option>
            <option value="clock">{t('reminders.time.clock')}</option>
          </select>
        </label>
      )}
      {ownTime && (
        <label className="form__field">
          <span className="form__label">{t('reminders.clockLabel')}</span>
          <input
            type="time"
            value={clockText(clock ?? 0)}
            onChange={(e) => {
              const next = clockOfText(e.target.value);
              // 00:00 is the whole day, stored so; the field stays.
              if (next !== null) onChange({ day, clock: next === 0 ? null : next });
            }}
          />
        </label>
      )}
    </div>
  );
}

interface RelativeFieldsProps {
  /** How far from the start, in minutes: before or after, the caller knows. */
  minutes: number;
  onChange: (minutes: number) => void;
}

function RelativeFields({ minutes, onChange }: RelativeFieldsProps) {
  const { t } = useTranslation();
  const { amount, unit } = splitRelativeMinutes(minutes);

  const setAmount = (next: number) => {
    onChange(relativeAmount(next) * RELATIVE_UNIT_MINUTES[unit]);
  };
  const setUnit = (next: RelativeUnit) => {
    if (next === unit) return;
    onChange(Math.max(1, amount) * RELATIVE_UNIT_MINUTES[next]);
  };

  return (
    <div className="form__row reminders__relative">
      <label className="form__field">
        <span className="form__label">{t('reminders.amountLabel')}</span>
        <input
          type="number"
          min={1}
          max={9999}
          value={amount}
          onChange={(e) => setAmount(Number(e.target.value))}
        />
      </label>
      <label className="form__field">
        <span className="form__label">{t('reminders.unitLabel')}</span>
        <select
          value={unit}
          onChange={(e) => setUnit(e.target.value as RelativeUnit)}
        >
          <option value="minutes">{t('reminders.unit.minutes')}</option>
          <option value="hours">{t('reminders.unit.hours')}</option>
          <option value="days">{t('reminders.unit.days')}</option>
        </select>
      </label>
    </div>
  );
}

interface AbsoluteFieldProps {
  iso: string;
  onChange: (iso: string) => void;
}

function AbsoluteField({ iso, onChange }: AbsoluteFieldProps) {
  const { t } = useTranslation();
  // <input type="datetime-local"> expects "YYYY-MM-DDTHH:MM" without
  // timezone; we work in local time and re-attach the offset on save.
  const local = isoToLocalInput(iso);
  return (
    <label className="form__field">
      <span className="form__label">{t('reminders.absoluteAtLabel')}</span>
      <input
        type="datetime-local"
        value={local}
        onChange={(e) => {
          const next = localInputToIso(e.target.value);
          if (next) onChange(next);
        }}
      />
    </label>
  );
}

function isoToLocalInput(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  const hh = String(d.getHours()).padStart(2, '0');
  const mm = String(d.getMinutes()).padStart(2, '0');
  return `${y}-${m}-${day}T${hh}:${mm}`;
}

function localInputToIso(input: string): string | null {
  if (!input) return null;
  const d = new Date(input);
  if (Number.isNaN(d.getTime())) return null;
  return d.toISOString();
}
