import { useTranslation } from 'react-i18next';

import type { TaskFreq, TaskRecurrenceValue } from './taskRecurrence';

/**
 * Recurrence editor for tasks. The value model + backend conversion
 * live in `./taskRecurrence`; this module is the React editor only.
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

          {value.freq === 'MONTHLY' && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.task.recurrence.dayOfMonthLabel')}
              </span>
              <input
                type="number"
                min={0}
                max={31}
                value={value.dayOfMonth || ''}
                placeholder={t('dialogs.task.recurrence.dayOfMonthPlaceholder')}
                onChange={(e) => {
                  const n = Number(e.target.value);
                  update({
                    dayOfMonth: Number.isFinite(n) && n > 0 && n <= 31 ? n : 0,
                  });
                }}
              />
              <span className="form__hint">
                {t('dialogs.task.recurrence.dayOfMonthHint')}
              </span>
            </label>
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

