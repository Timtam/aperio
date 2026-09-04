import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';

import type { Reminder } from '../api/types';
import { SoundPicker } from './SoundPicker';

/**
 * Editor for the `reminders` field on an event or task (DESIGN.md
 * section 14).
 *
 * Phase 5 ships the three reminder types the local engine actually
 * supports:
 *  - **Relative** — fire N minutes / hours / days before the event
 *    start or task due date.
 *  - **Absolute** — fire at a fixed wall-clock time.
 *  - **At app start** — fire on the next launch after the due time.
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

/** A row with its placement set — and without the flag when it stays local. */
function withPlacement(row: EditableReminder, attach: boolean): EditableReminder {
  const bare: EditableReminder = { kind: row.kind, sound: row.sound };
  return attach ? { ...bare, attach: true } : bare;
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
   * Show the per-row "Applies" choice — only in Aperio, or attached to new
   * events. Only the calendar-defaults panel turns this on, and only for
   * calendars whose new appointments can carry reminders at all.
   */
  placement?: boolean;
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

export function RemindersEditor({
  value,
  onChange,
  mode,
  placement = false,
  allowAppStart = true,
}: RemindersEditorProps) {
  const { t } = useTranslation();

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
    onChange([...value, { ...DEFAULT_RELATIVE }]);
  }, [value, onChange]);

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
                position={i + 1}
                placement={placement}
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
  /** 1-based index used in the row's aria-label. */
  position: number;
  /** Show the "Applies" choice (calendar defaults only). */
  placement: boolean;
  /** Offer "on next app start" — see `RemindersEditorProps`. */
  allowAppStart: boolean;
}

function ReminderRow({
  value,
  onChange,
  onRemove,
  mode,
  position,
  placement,
  allowAppStart,
}: ReminderRowProps) {
  const { t } = useTranslation();
  const kindType = value.kind.type;
  // Never drop the kind a row already has: a list stored before the offer was
  // withdrawn must still read as what it is, and stay changeable.
  const showAppStart = allowAppStart || kindType === 'app_start';

  const setKind = (next: ReminderKindOption) => {
    onChange({ ...value, kind: defaultsForKind(next, mode) });
  };

  return (
    <div
      className="reminders__row-inner"
      role="group"
      aria-label={t('reminders.rowLabel', { n: position })}
    >
      <label className="form__field">
        <span className="form__label">{t('reminders.kindLabel')}</span>
        <select
          value={kindType}
          onChange={(e) => setKind(e.target.value as ReminderKindOption)}
        >
          <option value="relative">
            {t(
              mode === 'event'
                ? 'reminders.kind.relativeEvent'
                : 'reminders.kind.relativeTask',
            )}
          </option>
          <option value="absolute">{t('reminders.kind.absolute')}</option>
          {showAppStart && (
            <option value="app_start">{t('reminders.kind.appStart')}</option>
          )}
        </select>
      </label>

      {value.kind.type === 'relative' && (
        <RelativeFields
          minutes={value.kind.minutes_before}
          onChange={(minutes) =>
            onChange({
              ...value,
              kind: { type: 'relative', minutes_before: minutes },
            })
          }
        />
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
            onChange={(e) =>
              onChange(withPlacement(value, e.target.value === 'attach'))
            }
          >
            <option value="local">{t('reminders.placement.local')}</option>
            <option value="attach">{t('reminders.placement.attach')}</option>
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

type ReminderKindOption = 'relative' | 'absolute' | 'app_start';

function defaultsForKind(
  kind: ReminderKindOption,
  _mode: 'event' | 'task',
): Reminder['kind'] {
  switch (kind) {
    case 'relative':
      return { type: 'relative', minutes_before: 15 };
    case 'absolute': {
      // Default to the next full hour tomorrow — far enough that the
      // user is unlikely to leave it as-is by accident.
      const at = new Date();
      at.setMinutes(0, 0, 0);
      at.setDate(at.getDate() + 1);
      return { type: 'absolute', at: at.toISOString() };
    }
    case 'app_start':
      return { type: 'app_start' };
  }
}

interface RelativeFieldsProps {
  minutes: number;
  onChange: (minutes: number) => void;
}

function RelativeFields({ minutes, onChange }: RelativeFieldsProps) {
  const { t } = useTranslation();
  const { amount, unit } = splitRelative(minutes);

  const setAmount = (next: number) => {
    const n = Number.isFinite(next) && next > 0 ? Math.floor(next) : 1;
    onChange(n * UNIT_FACTORS[unit]);
  };
  const setUnit = (next: RelativeUnit) => {
    onChange(Math.max(1, amount) * UNIT_FACTORS[next]);
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
