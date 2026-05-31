/**
 * Task recurrence value model + conversion to/from the backend struct.
 *
 * Tasks model recurrence as a structured value (frequency + interval +
 * optional weekdays / day-of-month + optional end), not as an RRULE
 * string — the generator runs server-side after completion, so we never
 * need full RFC 5545 expressiveness.
 *
 * Kept free of React so the editor component (`TaskRecurrenceSelector`)
 * stays a pure-component module (Fast Refresh) and so the conversion
 * round-trip can be unit-tested in isolation.
 */
export type TaskFreq = 'NONE' | 'DAILY' | 'WEEKLY' | 'MONTHLY' | 'YEARLY';

export interface TaskRecurrenceValue {
  freq: TaskFreq;
  interval: number;
  byDay: string[]; // ISO weekday short names, "MO".."SU"
  /** 1..31, only meaningful when freq = 'MONTHLY' and value > 0. */
  dayOfMonth: number;
  endMode: 'NEVER' | 'UNTIL';
  until: string; // YYYY-MM-DD, only meaningful when endMode = 'UNTIL'
}

export const TASK_RECURRENCE_DEFAULT: TaskRecurrenceValue = {
  freq: 'NONE',
  interval: 1,
  byDay: [],
  dayOfMonth: 0,
  endMode: 'NEVER',
  until: '',
};

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
    day_of_month:
      value.freq === 'MONTHLY' && value.dayOfMonth > 0
        ? value.dayOfMonth
        : null,
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
    dayOfMonth:
      typeof r.day_of_month === 'number' && r.day_of_month >= 1 && r.day_of_month <= 31
        ? r.day_of_month
        : 0,
    endMode,
    until,
  };
}
