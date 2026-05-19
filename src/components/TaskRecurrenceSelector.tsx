import { useTranslation } from 'react-i18next';

/**
 * Recurrence editor for tasks.
 *
 * Tasks model recurrence as a structured value (frequency + interval +
 * optional weekdays + optional end), not as an RRULE string — the
 * generator runs server-side after completion, so we never need full
 * RFC 5545 expressiveness. Compared to the event selector this widget
 * is intentionally smaller; the rest of the spec's surface (day-of-
 * month, count-based end) belongs to the sync wave when those fields
 * become observable through external adapters.
 */
export type TaskFreq = 'NONE' | 'DAILY' | 'WEEKLY' | 'MONTHLY' | 'YEARLY';

export interface TaskRecurrenceValue {
  freq: TaskFreq;
  interval: number;
  byDay: string[]; // ISO weekday short names, "MO".."SU"
  endMode: 'NEVER' | 'UNTIL';
  until: string; // YYYY-MM-DD, only meaningful when endMode = 'UNTIL'
}

export const TASK_RECURRENCE_DEFAULT: TaskRecurrenceValue = {
  freq: 'NONE',
  interval: 1,
  byDay: [],
  endMode: 'NEVER',
  until: '',
};

const WEEKDAYS: { iso: string; key: string }[] = [
  { iso: 'MO', key: 'mon' },
  { iso: 'TU', key: 'tue' },
  { iso: 'WE', key: 'wed' },
  { iso: 'TH', key: 'thu' },
  { iso: 'FR', key: 'fri' },
  { iso: 'SA', key: 'sat' },
  { iso: 'SU', key: 'sun' },
];

export interface TaskRecurrenceSelectorProps {
  value: TaskRecurrenceValue;
  onChange: (next: TaskRecurrenceValue) => void;
}

export function TaskRecurrenceSelector({
  value,
  onChange,
}: TaskRecurrenceSelectorProps) {
  const { t } = useTranslation();
  const update = (patch: Partial<TaskRecurrenceValue>) =>
    onChange({ ...value, ...patch });

  return (
    <div className="recurrence">
      <label className="form__field">
        <span className="form__label">
          {t('dialogs.task.recurrence.label')}
        </span>
        <select
          value={value.freq}
          onChange={(e) =>
            update({
              freq: e.target.value as TaskFreq,
              byDay: e.target.value === 'WEEKLY' ? value.byDay : [],
            })
          }
        >
          <option value="NONE">{t('dialogs.task.recurrence.none')}</option>
          <option value="DAILY">{t('dialogs.task.recurrence.daily')}</option>
          <option value="WEEKLY">{t('dialogs.task.recurrence.weekly')}</option>
          <option value="MONTHLY">{t('dialogs.task.recurrence.monthly')}</option>
          <option value="YEARLY">{t('dialogs.task.recurrence.yearly')}</option>
        </select>
      </label>

      {value.freq !== 'NONE' && (
        <>
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.task.recurrence.intervalLabel', {
                unit: t(`dialogs.task.recurrence.unit.${value.freq}`),
              })}
            </span>
            <input
              type="number"
              min={1}
              max={365}
              value={value.interval}
              onChange={(e) =>
                update({
                  interval: Math.max(1, Number(e.target.value) || 1),
                })
              }
            />
          </label>

          {value.freq === 'WEEKLY' && (
            <fieldset className="form__field recurrence__weekdays">
              <legend className="form__label">
                {t('dialogs.task.recurrence.weekdays')}
              </legend>
              <div className="recurrence__weekdays-row">
                {WEEKDAYS.map((d) => {
                  const checked = value.byDay.includes(d.iso);
                  return (
                    <label key={d.iso} className="recurrence__weekday">
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={(e) => {
                          const next = e.target.checked
                            ? [...value.byDay, d.iso]
                            : value.byDay.filter((x) => x !== d.iso);
                          update({ byDay: next });
                        }}
                      />
                      <span>{t(`dialogs.task.recurrence.short.${d.key}`)}</span>
                    </label>
                  );
                })}
              </div>
            </fieldset>
          )}

          <label className="form__field">
            <span className="form__label">
              {t('dialogs.task.recurrence.endLabel')}
            </span>
            <select
              value={value.endMode}
              onChange={(e) =>
                update({ endMode: e.target.value as 'NEVER' | 'UNTIL' })
              }
            >
              <option value="NEVER">
                {t('dialogs.task.recurrence.end.never')}
              </option>
              <option value="UNTIL">
                {t('dialogs.task.recurrence.end.until')}
              </option>
            </select>
          </label>

          {value.endMode === 'UNTIL' && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.task.recurrence.untilLabel')}
              </span>
              <input
                type="date"
                value={value.until}
                onChange={(e) => update({ until: e.target.value })}
              />
            </label>
          )}
        </>
      )}
    </div>
  );
}

// ── Conversion between the form value and the backend struct ────────────────

interface BackendRecurrence {
  frequency: 'daily' | 'weekly' | 'monthly' | 'yearly';
  interval: number;
  day_of_week: string[] | null;
  day_of_month: number | null;
  end: BackendEnd | null;
}

type BackendEnd =
  | { type: 'never' }
  | { type: 'on_date'; date: string }
  | { type: 'after'; occurrences: number };

const WEEKDAY_TO_BACKEND: Record<string, string> = {
  MO: 'monday',
  TU: 'tuesday',
  WE: 'wednesday',
  TH: 'thursday',
  FR: 'friday',
  SA: 'saturday',
  SU: 'sunday',
};

const WEEKDAY_FROM_BACKEND: Record<string, string> = Object.fromEntries(
  Object.entries(WEEKDAY_TO_BACKEND).map(([k, v]) => [v, k]),
);

export function toBackend(
  value: TaskRecurrenceValue,
): BackendRecurrence | null {
  if (value.freq === 'NONE') return null;
  const end: BackendEnd | null =
    value.endMode === 'UNTIL' && value.until
      ? { type: 'on_date', date: value.until }
      : { type: 'never' };
  return {
    frequency: value.freq.toLowerCase() as BackendRecurrence['frequency'],
    interval: value.interval,
    day_of_week:
      value.freq === 'WEEKLY' && value.byDay.length > 0
        ? value.byDay.map((d) => WEEKDAY_TO_BACKEND[d]).filter(Boolean)
        : null,
    day_of_month: null,
    end,
  };
}

export function fromBackend(raw: unknown): TaskRecurrenceValue {
  if (!raw || typeof raw !== 'object') return { ...TASK_RECURRENCE_DEFAULT };
  const r = raw as Partial<BackendRecurrence>;
  const freq = (r.frequency ?? '').toUpperCase();
  const validFreq: TaskFreq =
    freq === 'DAILY' ||
    freq === 'WEEKLY' ||
    freq === 'MONTHLY' ||
    freq === 'YEARLY'
      ? (freq as TaskFreq)
      : 'NONE';
  const byDay = (r.day_of_week ?? [])
    .map((d) => WEEKDAY_FROM_BACKEND[d])
    .filter(Boolean);
  let endMode: 'NEVER' | 'UNTIL' = 'NEVER';
  let until = '';
  if (r.end && typeof r.end === 'object' && 'type' in r.end) {
    if (r.end.type === 'on_date') {
      endMode = 'UNTIL';
      until = r.end.date;
    }
  }
  return {
    freq: validFreq,
    interval: Math.max(1, r.interval ?? 1),
    byDay,
    endMode,
    until,
  };
}
