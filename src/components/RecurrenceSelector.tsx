import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

/**
 * Recurrence editor for the event dialog.
 *
 * Speaks the subset of RFC 5545 that 95% of users actually need:
 * FREQ, INTERVAL, COUNT, UNTIL, BYDAY (for the weekly weekday picker).
 * Anything more exotic (BYMONTHDAY, BYSETPOS, …) can come later; the
 * surface is intentionally small to keep the UI accessible by keyboard.
 *
 * Communicates with the parent purely through a `value: string | null`
 * (the RRULE body, without the "RRULE:" prefix) and an `onChange`
 * callback. Storing the body matches what `cal-core::EventRecurrence`
 * already keeps in `rrule`.
 */
export type Freq = 'NONE' | 'DAILY' | 'WEEKLY' | 'MONTHLY' | 'YEARLY';

export type EndMode = 'NEVER' | 'COUNT' | 'UNTIL';

export interface RecurrenceSelectorProps {
  value: string | null;
  onChange: (rrule: string | null) => void;
}

const WEEKDAYS: { iso: number; rrule: string; key: string }[] = [
  { iso: 1, rrule: 'MO', key: 'mon' },
  { iso: 2, rrule: 'TU', key: 'tue' },
  { iso: 3, rrule: 'WE', key: 'wed' },
  { iso: 4, rrule: 'TH', key: 'thu' },
  { iso: 5, rrule: 'FR', key: 'fri' },
  { iso: 6, rrule: 'SA', key: 'sat' },
  { iso: 7, rrule: 'SU', key: 'sun' },
];

export function RecurrenceSelector({ value, onChange }: RecurrenceSelectorProps) {
  const { t } = useTranslation();
  const parsed = useMemo(() => parseRRule(value), [value]);

  const update = (next: ParsedRule) => onChange(buildRRule(next));

  return (
    <div className="recurrence">
      <label className="form__field">
        <span className="form__label">
          {t('dialogs.event.recurrence.label')}
        </span>
        <select
          value={parsed.freq}
          onChange={(e) =>
            update({
              ...parsed,
              freq: e.target.value as Freq,
              // Reset BYDAY when switching away from weekly.
              byDay: e.target.value === 'WEEKLY' ? parsed.byDay : [],
            })
          }
        >
          <option value="NONE">{t('dialogs.event.recurrence.none')}</option>
          <option value="DAILY">{t('dialogs.event.recurrence.daily')}</option>
          <option value="WEEKLY">{t('dialogs.event.recurrence.weekly')}</option>
          <option value="MONTHLY">{t('dialogs.event.recurrence.monthly')}</option>
          <option value="YEARLY">{t('dialogs.event.recurrence.yearly')}</option>
        </select>
      </label>

      {parsed.freq !== 'NONE' && (
        <>
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.event.recurrence.intervalLabel', {
                unit: t(`dialogs.event.recurrence.unit.${parsed.freq}`),
              })}
            </span>
            <input
              type="number"
              min={1}
              max={365}
              value={parsed.interval}
              onChange={(e) =>
                update({
                  ...parsed,
                  interval: Math.max(1, Number(e.target.value) || 1),
                })
              }
            />
          </label>

          {parsed.freq === 'WEEKLY' && (
            <fieldset className="form__field recurrence__weekdays">
              <legend className="form__label">
                {t('dialogs.event.recurrence.weekdays')}
              </legend>
              <div className="recurrence__weekdays-row">
                {WEEKDAYS.map((d) => {
                  const checked = parsed.byDay.includes(d.rrule);
                  return (
                    <label key={d.rrule} className="recurrence__weekday">
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={(e) => {
                          const next = e.target.checked
                            ? [...parsed.byDay, d.rrule]
                            : parsed.byDay.filter((x) => x !== d.rrule);
                          update({ ...parsed, byDay: next });
                        }}
                      />
                      <span>{t(`dialogs.event.recurrence.short.${d.key}`)}</span>
                    </label>
                  );
                })}
              </div>
            </fieldset>
          )}

          <label className="form__field">
            <span className="form__label">
              {t('dialogs.event.recurrence.endLabel')}
            </span>
            <select
              value={parsed.endMode}
              onChange={(e) =>
                update({ ...parsed, endMode: e.target.value as EndMode })
              }
            >
              <option value="NEVER">
                {t('dialogs.event.recurrence.end.never')}
              </option>
              <option value="COUNT">
                {t('dialogs.event.recurrence.end.count')}
              </option>
              <option value="UNTIL">
                {t('dialogs.event.recurrence.end.until')}
              </option>
            </select>
          </label>

          {parsed.endMode === 'COUNT' && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.event.recurrence.countLabel')}
              </span>
              <input
                type="number"
                min={1}
                max={9999}
                value={parsed.count}
                onChange={(e) =>
                  update({
                    ...parsed,
                    count: Math.max(1, Number(e.target.value) || 1),
                  })
                }
              />
            </label>
          )}

          {parsed.endMode === 'UNTIL' && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.event.recurrence.untilLabel')}
              </span>
              <input
                type="date"
                value={parsed.until}
                onChange={(e) => update({ ...parsed, until: e.target.value })}
              />
            </label>
          )}
        </>
      )}
    </div>
  );
}

interface ParsedRule {
  freq: Freq;
  interval: number;
  byDay: string[];
  endMode: EndMode;
  count: number;
  /** YYYY-MM-DD */
  until: string;
}

const DEFAULT_RULE: ParsedRule = {
  freq: 'NONE',
  interval: 1,
  byDay: [],
  endMode: 'NEVER',
  count: 10,
  until: '',
};

export function parseRRule(value: string | null): ParsedRule {
  if (!value) return { ...DEFAULT_RULE };
  const body = value.toUpperCase().startsWith('RRULE:')
    ? value.slice('RRULE:'.length)
    : value;
  const parts = new Map<string, string>();
  body.split(';').forEach((piece) => {
    const [k, v] = piece.split('=');
    if (k && v) parts.set(k.trim().toUpperCase(), v.trim());
  });

  const freqStr = parts.get('FREQ') ?? 'NONE';
  const freq: Freq =
    freqStr === 'DAILY' ||
    freqStr === 'WEEKLY' ||
    freqStr === 'MONTHLY' ||
    freqStr === 'YEARLY'
      ? freqStr
      : 'NONE';

  const interval = Math.max(1, Number(parts.get('INTERVAL') ?? '1') || 1);
  const byDay = (parts.get('BYDAY') ?? '')
    .split(',')
    .map((x) => x.trim())
    .filter(Boolean);

  let endMode: EndMode = 'NEVER';
  let count = DEFAULT_RULE.count;
  let until = DEFAULT_RULE.until;
  if (parts.has('COUNT')) {
    endMode = 'COUNT';
    count = Math.max(1, Number(parts.get('COUNT')) || 1);
  } else if (parts.has('UNTIL')) {
    endMode = 'UNTIL';
    const raw = parts.get('UNTIL') ?? '';
    // Accept both "20260530" and "20260530T235959Z" forms.
    const m = raw.match(/^(\d{4})(\d{2})(\d{2})/);
    if (m) {
      until = `${m[1]}-${m[2]}-${m[3]}`;
    }
  }

  return { freq, interval, byDay, endMode, count, until };
}

export function buildRRule(p: ParsedRule): string | null {
  if (p.freq === 'NONE') return null;
  const out: string[] = [`FREQ=${p.freq}`];
  if (p.interval > 1) out.push(`INTERVAL=${p.interval}`);
  if (p.freq === 'WEEKLY' && p.byDay.length > 0) {
    out.push(`BYDAY=${p.byDay.join(',')}`);
  }
  if (p.endMode === 'COUNT') {
    out.push(`COUNT=${p.count}`);
  } else if (p.endMode === 'UNTIL' && p.until) {
    // RFC 5545 wants a basic-format UTC timestamp. End-of-day so the
    // last day is included whatever the user's timezone is.
    const [y, m, d] = p.until.split('-');
    if (y && m && d) {
      out.push(`UNTIL=${y}${m}${d}T235959Z`);
    }
  }
  return out.join(';');
}
