import { useTranslation } from 'react-i18next';

import type { RecurrenceCapabilities, RecurrenceFreq } from '../api/types';
import type {
  TaskAnchor,
  TaskFixedDate,
  TaskFreq,
  TaskPlacement,
  TaskRecurrenceValue,
} from './taskRecurrence';

/**
 * Recurrence editor for tasks. The value model + backend conversion
 * live in `./taskRecurrence`; this module is the React editor only.
 *
 * Like the event {@link RecurrenceSelector}, it gates each control on
 * the target list's recurrence capabilities: options the source can't
 * store (e.g. Vikunja has no yearly, weekday picker, explicit
 * day-of-month or COUNT / UNTIL end) render disabled with a hint instead
 * of being silently dropped on save. Absent capabilities ⇒ full support
 * (the local store + most adapters).
 */
const WEEKDAYS: { iso: string; key: string }[] = [
  { iso: 'MO', key: 'mon' },
  { iso: 'TU', key: 'tue' },
  { iso: 'WE', key: 'wed' },
  { iso: 'TH', key: 'thu' },
  { iso: 'FR', key: 'fri' },
  { iso: 'SA', key: 'sat' },
  { iso: 'SU', key: 'sun' },
];

/** Permissive fallback when no `capabilities` prop is supplied — every
 *  axis fully supported. Mirrors `RecurrenceCapabilities::default`. */
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

/** Lowercase the selector's `TaskFreq` to the wire `RecurrenceFreq`.
 *  `NONE` has no capability axis and returns null. */
function freqKey(freq: TaskFreq): RecurrenceFreq | null {
  return freq === 'NONE' ? null : (freq.toLowerCase() as RecurrenceFreq);
}

function freqSupported(freq: TaskFreq, caps: RecurrenceCapabilities): boolean {
  const k = freqKey(freq);
  return k === null || caps.frequencies.includes(k);
}

function intervalSupported(
  freq: TaskFreq,
  caps: RecurrenceCapabilities,
): boolean {
  const k = freqKey(freq);
  return k === null || caps.interval_frequencies.includes(k);
}

const FREQ_OPTIONS: { value: Exclude<TaskFreq, 'NONE'>; key: string }[] = [
  { value: 'DAILY', key: 'daily' },
  { value: 'WEEKLY', key: 'weekly' },
  { value: 'MONTHLY', key: 'monthly' },
  { value: 'YEARLY', key: 'yearly' },
];

export interface TaskRecurrenceSelectorProps {
  value: TaskRecurrenceValue;
  onChange: (next: TaskRecurrenceValue) => void;
  /** Recurrence shapes the target list's adapter can store. Unsupported
   *  options render disabled with a hint. Absent ⇒ full support. */
  capabilities?: RecurrenceCapabilities;
}

export function TaskRecurrenceSelector({
  value,
  onChange,
  capabilities,
}: TaskRecurrenceSelectorProps) {
  const { t } = useTranslation();
  const caps = capabilities ?? FULL_CAPS;
  const update = (patch: Partial<TaskRecurrenceValue>) =>
    onChange({ ...value, ...patch });

  const intervalEnabled = intervalSupported(value.freq, caps);
  // Backlog placement (DESIGN §9.12) allows interval 0 — "resurface
  // immediately on completion" (the dishwasher). Scheduled rules stay ≥ 1.
  const minInterval = value.placement === 'BACKLOG' ? 0 : 1;
  // Clamp the displayed interval when this source can't store one for the
  // frequency, so the disabled input shows what'll actually round-trip
  // (matches the event selector).
  const shownInterval = intervalEnabled ? value.interval : minInterval;
  const unsupportedHint = (
    <span className="form__hint recurrence__unsupported">
      {t('dialogs.task.recurrence.unsupportedHint')}
    </span>
  );

  return (
    <div className="recurrence">
      <label className="form__field">
        <span className="form__label">
          {t('dialogs.task.recurrence.label')}
        </span>
        <select
          value={value.freq}
          onChange={(e) => {
            const freq = e.target.value as TaskFreq;
            update({
              freq,
              byDay: freq === 'WEEKLY' ? value.byDay : [],
              // Drop an interval the new frequency can't store.
              interval: intervalSupported(freq, caps) ? value.interval : 1,
            });
          }}
        >
          <option value="NONE">{t('dialogs.task.recurrence.none')}</option>
          {FREQ_OPTIONS.map((o) => (
            <option
              key={o.value}
              value={o.value}
              disabled={!freqSupported(o.value, caps)}
            >
              {t(`dialogs.task.recurrence.${o.key}`)}
            </option>
          ))}
        </select>
      </label>

      {value.freq !== 'NONE' && (
        <>
          {/* DESIGN §9.12 — placement: schedule the next instance on a day,
              or let it resurface in the backlog (undated). */}
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.task.recurrence.placementLabel')}
            </span>
            <select
              value={value.placement}
              onChange={(e) =>
                update({ placement: e.target.value as TaskPlacement })
              }
            >
              <option value="SCHEDULE">
                {t('dialogs.task.recurrence.placement.schedule')}
              </option>
              <option value="BACKLOG">
                {t('dialogs.task.recurrence.placement.backlog')}
              </option>
            </select>
          </label>

          {/* DESIGN §9.12 — anchor: advance from the task's own date, or
              from when it was completed (org-mode `.+`). */}
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.task.recurrence.anchorLabel')}
            </span>
            <select
              value={value.anchor}
              onChange={(e) =>
                update({ anchor: e.target.value as TaskAnchor })
              }
            >
              <option value="FROM_DATE">
                {t('dialogs.task.recurrence.anchor.fromDate')}
              </option>
              <option value="FROM_COMPLETION">
                {t('dialogs.task.recurrence.anchor.fromCompletion')}
              </option>
            </select>
          </label>

          {value.fixedDates.length === 0 && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.task.recurrence.intervalLabel', {
                  unit: t(`dialogs.task.recurrence.unit.${value.freq}`),
                })}
              </span>
              <input
                type="number"
                min={minInterval}
                max={365}
                value={shownInterval}
                disabled={!intervalEnabled}
                onChange={(e) =>
                  update({
                    interval: Math.max(
                      minInterval,
                      Number(e.target.value) || minInterval,
                    ),
                  })
                }
              />
              {!intervalEnabled && unsupportedHint}
              {minInterval === 0 && (
                <span className="form__hint">
                  {t('dialogs.task.recurrence.backlogIntervalHint')}
                </span>
              )}
            </label>
          )}

          {value.freq === 'WEEKLY' &&
            value.placement === 'SCHEDULE' &&
            value.fixedDates.length === 0 && (
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
                        disabled={!caps.weekly_byday}
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
              {!caps.weekly_byday && unsupportedHint}
            </fieldset>
          )}

          {value.freq === 'MONTHLY' &&
            value.placement === 'SCHEDULE' &&
            value.fixedDates.length === 0 && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.task.recurrence.dayOfMonthLabel')}
              </span>
              <input
                type="number"
                min={0}
                max={31}
                value={value.dayOfMonth || ''}
                disabled={!caps.monthly_day_of_month}
                placeholder={t('dialogs.task.recurrence.dayOfMonthPlaceholder')}
                onChange={(e) => {
                  const n = Number(e.target.value);
                  update({
                    dayOfMonth: Number.isFinite(n) && n > 0 && n <= 31 ? n : 0,
                  });
                }}
              />
              <span className="form__hint">
                {caps.monthly_day_of_month
                  ? t('dialogs.task.recurrence.dayOfMonthHint')
                  : t('dialogs.task.recurrence.unsupportedHint')}
              </span>
            </label>
          )}

          {/* DESIGN §9.12 — fixed (month, day) triggers, e.g. the seasonal
              shoe-swap on Apr 1 / Oct 1. When any exist they drive the
              schedule instead of frequency/interval. */}
          <FixedDatesEditor
            value={value.fixedDates}
            onChange={(fixedDates) => update({ fixedDates })}
            t={t}
          />

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
              <option value="UNTIL" disabled={!caps.until}>
                {t('dialogs.task.recurrence.end.until')}
              </option>
            </select>
            {!caps.until && value.endMode === 'UNTIL' && unsupportedHint}
          </label>

          {value.endMode === 'UNTIL' && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.task.recurrence.untilLabel')}
              </span>
              <input
                type="date"
                value={value.until}
                disabled={!caps.until}
                onChange={(e) => update({ until: e.target.value })}
              />
            </label>
          )}
        </>
      )}
    </div>
  );
}

interface FixedDatesEditorProps {
  value: TaskFixedDate[];
  onChange: (next: TaskFixedDate[]) => void;
  t: (key: string, vars?: Record<string, unknown>) => string;
}

/** Editor for the optional `fixed_dates` list (DESIGN §9.12). Each row is a
 *  month (1–12) + day (1–31) pair; "add" appends Jan 1 as a starting point.
 *  Values are clamped on input and re-validated on save by `toBackend`. */
function FixedDatesEditor({ value, onChange, t }: FixedDatesEditorProps) {
  const setAt = (i: number, patch: Partial<TaskFixedDate>) =>
    onChange(value.map((d, j) => (j === i ? { ...d, ...patch } : d)));
  const removeAt = (i: number) => onChange(value.filter((_, j) => j !== i));
  const add = () => onChange([...value, { month: 1, day: 1 }]);
  return (
    <fieldset className="form__field recurrence__fixed-dates">
      <legend className="form__label">
        {t('dialogs.task.recurrence.fixedDatesLabel')}
      </legend>
      {value.map((d, i) => (
        // Index key: rows are a simple add/remove list with fully controlled
        // inputs and no reordering, so position is a stable identity here.
        <div key={i} className="recurrence__fixed-date-row">
          <input
            type="number"
            min={1}
            max={12}
            aria-label={t('dialogs.task.recurrence.fixedDateMonth')}
            value={d.month}
            onChange={(e) => setAt(i, { month: clampInt(e.target.value, 1, 12) })}
          />
          <input
            type="number"
            min={1}
            max={31}
            aria-label={t('dialogs.task.recurrence.fixedDateDay')}
            value={d.day}
            onChange={(e) => setAt(i, { day: clampInt(e.target.value, 1, 31) })}
          />
          <button
            type="button"
            className="recurrence__fixed-date-remove"
            aria-label={t('dialogs.task.recurrence.fixedDateRemove')}
            onClick={() => removeAt(i)}
          >
            ×
          </button>
        </div>
      ))}
      <button
        type="button"
        className="recurrence__fixed-date-add"
        onClick={add}
      >
        {t('dialogs.task.recurrence.fixedDateAdd')}
      </button>
      <span className="form__hint">
        {t('dialogs.task.recurrence.fixedDatesHint')}
      </span>
    </fieldset>
  );
}

/** Parse an integer from an input value, clamped to `[min, max]`. */
function clampInt(raw: string, min: number, max: number): number {
  const n = Math.trunc(Number(raw));
  if (!Number.isFinite(n)) return min;
  return Math.min(max, Math.max(min, n));
}
