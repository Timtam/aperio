/**
 * Task recurrence value model + conversion to/from the backend struct.
 *
 * Tasks model recurrence as a structured value (frequency + interval +
 * optional weekdays / day-of-month + optional end), not as an RRULE
 * string — the generator runs server-side after completion, so we never
 * need full RFC 5545 expressiveness.
 *
 * Platform-agnostic + React-free: lives in `@aperio/shared` so the desktop
 * editor (`TaskRecurrenceSelector`) and the mobile editor consume one source
 * of truth and the conversion round-trip is unit-tested in isolation. The
 * backend shape matches `cal_core::TaskRecurrence` exactly, so the JSON that
 * `toBackend` produces round-trips through both the desktop Tauri commands and
 * the mobile cal-ffi bridge unchanged.
 */
export type TaskFreq = 'NONE' | 'DAILY' | 'WEEKLY' | 'MONTHLY' | 'YEARLY';

/** DESIGN §9.12: from when the next instance is computed. */
export type TaskAnchor = 'FROM_DATE' | 'FROM_COMPLETION';
/** DESIGN §9.12: where the next instance lands. */
export type TaskPlacement = 'SCHEDULE' | 'BACKLOG';
/** A yearless calendar trigger, e.g. `{ month: 4, day: 1 }` for "April 1". */
export interface TaskFixedDate {
  month: number; // 1..12
  day: number; // 1..31
}

export interface TaskRecurrenceValue {
  freq: TaskFreq;
  interval: number;
  byDay: string[]; // ISO weekday short names, "MO".."SU"
  /** 1..31, only meaningful when freq = 'MONTHLY' and value > 0. */
  dayOfMonth: number;
  endMode: 'NEVER' | 'UNTIL';
  until: string; // YYYY-MM-DD, only meaningful when endMode = 'UNTIL'
  /** DESIGN §9.12: advance from the task's own date or from completion. */
  anchor: TaskAnchor;
  /** DESIGN §9.12: schedule the next instance on a day, or surface it in
   *  the backlog (undated, gated by its resurface date). */
  placement: TaskPlacement;
  /** DESIGN §9.12: when non-empty, these (month, day) triggers drive the
   *  schedule instead of freq/interval — e.g. the seasonal shoe-swap. */
  fixedDates: TaskFixedDate[];
}

export const TASK_RECURRENCE_DEFAULT: TaskRecurrenceValue = {
  freq: 'NONE',
  interval: 1,
  byDay: [],
  dayOfMonth: 0,
  endMode: 'NEVER',
  until: '',
  anchor: 'FROM_DATE',
  placement: 'SCHEDULE',
  fixedDates: [],
};

// ── Conversion between the form value and the backend struct ────────────────

interface BackendRecurrence {
  frequency: 'daily' | 'weekly' | 'monthly' | 'yearly';
  interval: number;
  day_of_week: string[] | null;
  day_of_month: number | null;
  end: BackendEnd | null;
  anchor: 'from_date' | 'from_completion';
  placement: 'schedule' | 'backlog';
  fixed_dates: TaskFixedDate[] | null;
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
  const fixedDates = sanitizeFixedDates(value.fixedDates);
  return {
    frequency: value.freq.toLowerCase() as BackendRecurrence['frequency'],
    // Backlog placement allows interval 0 (resurface immediately on
    // completion — the dishwasher case); scheduled rules stay ≥ 1.
    interval:
      value.placement === 'BACKLOG'
        ? Math.max(0, value.interval)
        : Math.max(1, value.interval),
    day_of_week:
      value.freq === 'WEEKLY' && value.byDay.length > 0
        ? value.byDay.map((d) => WEEKDAY_TO_BACKEND[d]).filter(Boolean)
        : null,
    day_of_month:
      value.freq === 'MONTHLY' && value.dayOfMonth > 0
        ? value.dayOfMonth
        : null,
    end,
    anchor: value.anchor === 'FROM_COMPLETION' ? 'from_completion' : 'from_date',
    placement: value.placement === 'BACKLOG' ? 'backlog' : 'schedule',
    fixed_dates: fixedDates.length > 0 ? fixedDates : null,
  };
}

/** Keep only well-formed (month 1..12, day 1..31) triggers. */
function sanitizeFixedDates(dates: TaskFixedDate[]): TaskFixedDate[] {
  return dates.filter(
    (d) =>
      Number.isInteger(d.month) &&
      d.month >= 1 &&
      d.month <= 12 &&
      Number.isInteger(d.day) &&
      d.day >= 1 &&
      d.day <= 31,
  );
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
  const placement: TaskPlacement = r.placement === 'backlog' ? 'BACKLOG' : 'SCHEDULE';
  const anchor: TaskAnchor =
    r.anchor === 'from_completion' ? 'FROM_COMPLETION' : 'FROM_DATE';
  const fixedDates = Array.isArray(r.fixed_dates)
    ? sanitizeFixedDates(r.fixed_dates)
    : [];
  // Backlog rules may legitimately carry interval 0 (resurface immediately);
  // scheduled rules are clamped to ≥ 1 as before.
  const rawInterval = typeof r.interval === 'number' ? r.interval : 1;
  const interval =
    placement === 'BACKLOG' ? Math.max(0, rawInterval) : Math.max(1, rawInterval);
  return {
    freq: validFreq,
    interval,
    byDay,
    dayOfMonth:
      typeof r.day_of_month === 'number' && r.day_of_month >= 1 && r.day_of_month <= 31
        ? r.day_of_month
        : 0,
    endMode,
    until,
    anchor,
    placement,
    fixedDates,
  };
}
