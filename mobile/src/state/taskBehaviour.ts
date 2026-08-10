import {
  FULL_DAY_WINDOW,
  MINUTES_PER_DAY,
  type PriorityScale,
  type TaskList,
  type TaskStatus,
} from '@aperio/shared';

import { getUserPref, setUserPref } from '../api/prefs';

// The task-behaviour knobs (Settings → Tasks), SYNCED across the user's devices
// via the `tasks.*` user-prefs (§19.2.1 always-sync keys) — the mobile twin of
// the desktop TaskCascadeProvider. Eight globals (check-off mode, status
// coupling, auto-date, auto-self-assign, visual effort sizing, calendar
// day-view mode, carry-over default, day-start trigger) + a per-list
// override map (`tasks.listOverrides`). (The day-view mode rides the
// `calendar.dayViewMode` key, shared 1:1 with the desktop toolbar toggle.)
// The check-off path reads the EFFECTIVE
// (per-list-resolved) cascade/auto-date, so a per-list override set on ANY
// device applies here. carryOverDefault + dayStartTrigger drive the day-start
// review / deadline-pin checkers. Best-effort: a read failure falls back to the
// desktop defaults; a write still applies for this session.

export type CheckoffMode = 'toggle' | 'cycle';
export type CarryOverDefault = 'ask' | 'today' | 'backlog';
/** When the day-start checkers fire on a long-running app. */
export type DayStartTrigger = 'app-start' | '00:00' | '06:00' | '08:00' | '12:00';

/** Per-list override of any subset of the knobs; absent fields inherit the
 *  global default. */
export interface ListOverrides {
  cascade?: boolean;
  autoDate?: boolean;
  carryOverDefault?: CarryOverDefault;
}

/** The resolved per-list values (override per field, else global) — all non-null. */
export interface EffectiveListSettings {
  cascade: boolean;
  autoDate: boolean;
  carryOverDefault: CarryOverDefault;
}

export interface TaskBehaviour {
  /** Couple parent/subtask status — off ⇒ single-row writes. Default on. */
  cascadeEnabled: boolean;
  /** Pin a dateless task to today when it enters in_progress. Default on. */
  autoDate: boolean;
  /** Self-assign me on status change in shared lists that know "me". Default on. */
  autoSelfAssign: boolean;
  /** Render task tiles at different sizes by effort (small/medium/large). Default on. */
  visualEffortSizing: boolean;
  /** Two priority levels (normal / important) instead of low-medium-high.
   *  Default OFF — the three-level system is what everyone had. */
  twoLevelPriority: boolean;
  /** Remind of today's untimed (date-only) scheduled tasks. Default on. */
  remindUntimedToday: boolean;
  /** Remind when a task's deadline day has arrived. Default on. */
  remindDeadlineArrived: boolean;
  /** Remind X days before a deadline (the X is `deadlineCountdownDays`). Default on. */
  remindDeadlineCountdown: boolean;
  /** Global "X days before a deadline" lead time (clamped 1..30). Default 3. */
  deadlineCountdownDays: number;
  /** Single-day calendar layout: proportional hour-grid or compact list. Default grid. */
  dayViewMode: 'grid' | 'list';
  /** Visible-window START of the calendar hour-grid, minutes from midnight
   *  (half-hour grid, [0, 1440]). Default 0 (midnight). */
  dayStartMin: number;
  /** Visible-window END of the calendar hour-grid, minutes from midnight
   *  (half-hour grid, [0, 1440]). Default 1440 (end of day). */
  dayEndMin: number;
  /** What a check-off does: flip open↔completed, or cycle through in_progress. */
  checkoffMode: CheckoffMode;
  /** Day-start action for tasks whose scheduled day passed + still open. */
  carryOverDefault: CarryOverDefault;
  /** When the day-start checkers fire. */
  dayStartTrigger: DayStartTrigger;
  /** Per-list overrides, keyed by task-list id; absent ⇒ inherit. */
  listOverrides: Record<string, ListOverrides>;
}

const CASCADE_KEY = 'tasks.cascadeStatusCoupling';
const AUTO_DATE_KEY = 'tasks.autoDateOnStart';
const AUTO_SELF_ASSIGN_KEY = 'tasks.autoSelfAssign';
const VISUAL_EFFORT_SIZING_KEY = 'tasks.visualEffortSizing';
// Same key string as the desktop TaskCascadeProvider, so the two sync 1:1.
const TWO_LEVEL_PRIORITY_KEY = 'tasks.twoLevelPriority';
// Day-start reminder knobs (synced). Three on/off booleans (default ON) + a
// numeric "X days before" lead time stored as a string like dayStartTrigger
// (parsed + clamped 1..30 on hydrate). The reminder LOGIC that consumes these
// lands in a later step; this module only owns the prefs.
const REMIND_UNTIMED_TODAY_KEY = 'tasks.remindUntimedToday';
const REMIND_DEADLINE_ARRIVED_KEY = 'tasks.remindDeadlineArrived';
const REMIND_DEADLINE_COUNTDOWN_KEY = 'tasks.remindDeadlineCountdown';
const DEADLINE_COUNTDOWN_DAYS_KEY = 'tasks.deadlineCountdownDays';
// Cross-device synced single-day calendar layout. SAME key string as the
// desktop toolbar's day-view-mode pref so the two sync 1:1 (must match exactly).
const CALENDAR_DAY_VIEW_MODE_KEY = 'calendar.dayViewMode';
// Cross-device synced visible day-window of the calendar hour-grid. Two integer
// minute values from midnight stored as strings (e.g. "420" = 07:00). SAME key
// strings as the desktop TaskCascadeProvider so the two sync 1:1.
const CALENDAR_DAY_START_MIN_KEY = 'calendar.dayStartMin';
const CALENDAR_DAY_END_MIN_KEY = 'calendar.dayEndMin';
const CHECKOFF_KEY = 'tasks.checkoffMode';
const CARRY_OVER_KEY = 'tasks.carryOverDefault';
const DAY_START_TRIGGER_KEY = 'tasks.dayStartTrigger';
const LIST_OVERRIDES_KEY = 'tasks.listOverrides';

/** The desktop defaults ("do what we've always done"). */
export const TASK_BEHAVIOUR_DEFAULTS: TaskBehaviour = {
  cascadeEnabled: true,
  autoDate: true,
  autoSelfAssign: true,
  visualEffortSizing: true,
  twoLevelPriority: false,
  remindUntimedToday: true,
  remindDeadlineArrived: true,
  remindDeadlineCountdown: true,
  deadlineCountdownDays: 3,
  dayViewMode: 'grid',
  dayStartMin: FULL_DAY_WINDOW.startMin,
  dayEndMin: FULL_DAY_WINDOW.endMin,
  checkoffMode: 'toggle',
  carryOverDefault: 'ask',
  dayStartTrigger: '00:00',
  listOverrides: {},
};

const CARRY_OVER_VALUES: readonly CarryOverDefault[] = ['ask', 'today', 'backlog'];
export function isCarryOverDefault(v: unknown): v is CarryOverDefault {
  return typeof v === 'string' && (CARRY_OVER_VALUES as readonly string[]).includes(v);
}

const DAY_START_VALUES: readonly DayStartTrigger[] = [
  'app-start',
  '00:00',
  '06:00',
  '08:00',
  '12:00',
];
export function isDayStartTrigger(v: unknown): v is DayStartTrigger {
  return typeof v === 'string' && (DAY_START_VALUES as readonly string[]).includes(v);
}

/** Boolean prefs default ON; only the literal `"false"` turns them off (matches
 *  the desktop TaskCascadeProvider hydration). */
function parseBool(stored: string | null): boolean {
  return stored !== 'false';
}

/** The priority system as the shared display + ordering helpers want it. */
export function priorityScaleFor(twoLevelPriority: boolean): PriorityScale {
  return twoLevelPriority ? 'two' : 'three';
}

/** The opposite default: a knob that is OFF until a literal 'true' says so. */
function parseOptIn(stored: string | null): boolean {
  return stored === 'true';
}

function parseCheckoff(stored: string | null): CheckoffMode {
  return stored === 'cycle' ? 'cycle' : 'toggle';
}

/** Default "X days before a deadline" lead time + the clamp bounds. */
export const DEADLINE_COUNTDOWN_DAYS_DEFAULT = 3;
export const DEADLINE_COUNTDOWN_DAYS_MIN = 1;
export const DEADLINE_COUNTDOWN_DAYS_MAX = 30;

/** Parse the stored "X days before" string and clamp it to 1..30; anything
 *  non-numeric / out of range falls back to the default of 3 (mirrors the
 *  desktop TaskCascadeProvider hydration). */
export function parseCountdownDays(stored: string | null): number {
  if (stored === null) return DEADLINE_COUNTDOWN_DAYS_DEFAULT;
  const n = Number.parseInt(stored, 10);
  if (!Number.isFinite(n)) return DEADLINE_COUNTDOWN_DAYS_DEFAULT;
  return Math.min(
    DEADLINE_COUNTDOWN_DAYS_MAX,
    Math.max(DEADLINE_COUNTDOWN_DAYS_MIN, n),
  );
}

/** Snap a raw minute value to the visible-day-window grid: integer, clamped to
 *  [0, 1440], rounded to the nearest 30 (half-hour granularity). A non-finite
 *  input falls back to `fallback` (already snapped). */
function snapWindowMinute(value: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  const rounded = Math.round(value / 30) * 30;
  return Math.min(MINUTES_PER_DAY, Math.max(0, rounded));
}

/** Validate a `(start, end)` day-window pair: each end snapped to the half-hour
 *  grid in [0, 1440]; a `start >= end` pair falls back to the FULL day window.
 *  Used by both the read parse and `writeDayWindow` (mirrors the desktop
 *  TaskCascadeProvider). */
export function validateDayWindow(
  startRaw: number,
  endRaw: number,
): { startMin: number; endMin: number } {
  const startMin = snapWindowMinute(startRaw, FULL_DAY_WINDOW.startMin);
  const endMin = snapWindowMinute(endRaw, FULL_DAY_WINDOW.endMin);
  if (startMin >= endMin) {
    return { startMin: FULL_DAY_WINDOW.startMin, endMin: FULL_DAY_WINDOW.endMin };
  }
  return { startMin, endMin };
}

/** Parse the two stored minute strings into a validated day-window; a
 *  missing/garbage edge parses to NaN → snapped to the full-day default, and an
 *  out-of-order pair falls back to the full day. */
function parseDayWindow(
  startStored: string | null,
  endStored: string | null,
): { startMin: number; endMin: number } {
  const startNum =
    startStored === null ? FULL_DAY_WINDOW.startMin : Number.parseInt(startStored, 10);
  const endNum =
    endStored === null ? FULL_DAY_WINDOW.endMin : Number.parseInt(endStored, 10);
  return validateDayWindow(startNum, endNum);
}

/** Parse + sanitise the listOverrides JSON per-list-per-field so one corrupt
 *  entry doesn't poison the others (mirrors the desktop hydration). */
function parseListOverrides(stored: string | null): Record<string, ListOverrides> {
  if (!stored) return {};
  try {
    const parsed = JSON.parse(stored) as unknown;
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};
    const out: Record<string, ListOverrides> = {};
    for (const [listId, raw] of Object.entries(parsed)) {
      if (!raw || typeof raw !== 'object') continue;
      const r = raw as Record<string, unknown>;
      const entry: ListOverrides = {};
      if (typeof r.cascade === 'boolean') entry.cascade = r.cascade;
      if (typeof r.autoDate === 'boolean') entry.autoDate = r.autoDate;
      if (isCarryOverDefault(r.carryOverDefault)) entry.carryOverDefault = r.carryOverDefault;
      if (
        entry.cascade !== undefined ||
        entry.autoDate !== undefined ||
        entry.carryOverDefault !== undefined
      ) {
        out[listId] = entry;
      }
    }
    return out;
  } catch {
    return {};
  }
}

/** Read all synced knobs + the per-list override map, or the desktop
 *  defaults when unset/unreadable. */
export async function readTaskBehaviour(): Promise<TaskBehaviour> {
  try {
    const [
      cascade,
      autoDate,
      selfAssign,
      effortSizing,
      twoLevelPriority,
      remindUntimed,
      remindDeadlineArrived,
      remindDeadlineCountdown,
      countdownDays,
      dayViewMode,
      dayStartMin,
      dayEndMin,
      checkoff,
      carryOver,
      trigger,
      overrides,
    ] = await Promise.all([
      getUserPref(CASCADE_KEY),
      getUserPref(AUTO_DATE_KEY),
      getUserPref(AUTO_SELF_ASSIGN_KEY),
      getUserPref(VISUAL_EFFORT_SIZING_KEY),
      getUserPref(TWO_LEVEL_PRIORITY_KEY),
      getUserPref(REMIND_UNTIMED_TODAY_KEY),
      getUserPref(REMIND_DEADLINE_ARRIVED_KEY),
      getUserPref(REMIND_DEADLINE_COUNTDOWN_KEY),
      getUserPref(DEADLINE_COUNTDOWN_DAYS_KEY),
      getUserPref(CALENDAR_DAY_VIEW_MODE_KEY),
      getUserPref(CALENDAR_DAY_START_MIN_KEY),
      getUserPref(CALENDAR_DAY_END_MIN_KEY),
      getUserPref(CHECKOFF_KEY),
      getUserPref(CARRY_OVER_KEY),
      getUserPref(DAY_START_TRIGGER_KEY),
      getUserPref(LIST_OVERRIDES_KEY),
    ]);
    const dayWindow = parseDayWindow(dayStartMin, dayEndMin);
    return {
      cascadeEnabled: parseBool(cascade),
      autoDate: parseBool(autoDate),
      autoSelfAssign: parseBool(selfAssign),
      visualEffortSizing: parseBool(effortSizing),
      twoLevelPriority: parseOptIn(twoLevelPriority),
      remindUntimedToday: parseBool(remindUntimed),
      remindDeadlineArrived: parseBool(remindDeadlineArrived),
      remindDeadlineCountdown: parseBool(remindDeadlineCountdown),
      deadlineCountdownDays: parseCountdownDays(countdownDays),
      dayViewMode: dayViewMode === 'list' ? 'list' : 'grid',
      dayStartMin: dayWindow.startMin,
      dayEndMin: dayWindow.endMin,
      checkoffMode: parseCheckoff(checkoff),
      carryOverDefault: isCarryOverDefault(carryOver) ? carryOver : 'ask',
      dayStartTrigger: isDayStartTrigger(trigger) ? trigger : '00:00',
      listOverrides: parseListOverrides(overrides),
    };
  } catch {
    return { ...TASK_BEHAVIOUR_DEFAULTS };
  }
}

async function writeBest(key: string, value: string): Promise<void> {
  try {
    await setUserPref(key, value);
  } catch {
    // Ignore — the chosen value still applies via local state this session.
  }
}

export const writeCascadeEnabled = (v: boolean): Promise<void> =>
  writeBest(CASCADE_KEY, v ? 'true' : 'false');
export const writeAutoDate = (v: boolean): Promise<void> =>
  writeBest(AUTO_DATE_KEY, v ? 'true' : 'false');
export const writeAutoSelfAssign = (v: boolean): Promise<void> =>
  writeBest(AUTO_SELF_ASSIGN_KEY, v ? 'true' : 'false');
export const writeVisualEffortSizing = (v: boolean): Promise<void> =>
  writeBest(VISUAL_EFFORT_SIZING_KEY, v ? 'true' : 'false');
export const writeTwoLevelPriority = (v: boolean): Promise<void> =>
  writeBest(TWO_LEVEL_PRIORITY_KEY, v ? 'true' : 'false');
export const writeRemindUntimedToday = (v: boolean): Promise<void> =>
  writeBest(REMIND_UNTIMED_TODAY_KEY, v ? 'true' : 'false');
export const writeRemindDeadlineArrived = (v: boolean): Promise<void> =>
  writeBest(REMIND_DEADLINE_ARRIVED_KEY, v ? 'true' : 'false');
export const writeRemindDeadlineCountdown = (v: boolean): Promise<void> =>
  writeBest(REMIND_DEADLINE_COUNTDOWN_KEY, v ? 'true' : 'false');
/** Persist the countdown lead time (clamped 1..30) as a string. */
export const writeDeadlineCountdownDays = (v: number): Promise<void> => {
  const clamped = Math.min(
    DEADLINE_COUNTDOWN_DAYS_MAX,
    Math.max(
      DEADLINE_COUNTDOWN_DAYS_MIN,
      Number.isFinite(v) ? Math.round(v) : DEADLINE_COUNTDOWN_DAYS_DEFAULT,
    ),
  );
  return writeBest(DEADLINE_COUNTDOWN_DAYS_KEY, String(clamped));
};
export const writeDayViewMode = (m: 'grid' | 'list'): Promise<void> =>
  writeBest(CALENDAR_DAY_VIEW_MODE_KEY, m);
/** Persist the visible day window. The pair is validated (snapped to the
 *  half-hour grid, clamped; full-day fallback when start >= end) so an invalid
 *  value can never reach storage; both keys are written. */
export const writeDayWindow = async (
  startMin: number,
  endMin: number,
): Promise<void> => {
  const win = validateDayWindow(startMin, endMin);
  await Promise.all([
    writeBest(CALENDAR_DAY_START_MIN_KEY, String(win.startMin)),
    writeBest(CALENDAR_DAY_END_MIN_KEY, String(win.endMin)),
  ]);
};
export const writeCheckoffMode = (m: CheckoffMode): Promise<void> =>
  writeBest(CHECKOFF_KEY, m);
export const writeCarryOverDefault = (v: CarryOverDefault): Promise<void> =>
  writeBest(CARRY_OVER_KEY, v);
export const writeDayStartTrigger = (v: DayStartTrigger): Promise<void> =>
  writeBest(DAY_START_TRIGGER_KEY, v);
export const writeListOverrides = (map: Record<string, ListOverrides>): Promise<void> =>
  writeBest(LIST_OVERRIDES_KEY, JSON.stringify(map));

/** Resolve the effective cascade / auto-date / carry-over for `listId`: the
 *  per-list override wins per field, else the global default. (dayStartTrigger
 *  is global — a clock-time, not per-list.) */
export function effectiveForList(b: TaskBehaviour, listId: string): EffectiveListSettings {
  const o = b.listOverrides[listId];
  return {
    cascade: o?.cascade ?? b.cascadeEnabled,
    autoDate: o?.autoDate ?? b.autoDate,
    carryOverDefault: o?.carryOverDefault ?? b.carryOverDefault,
  };
}

/** A new override map with `listId` set to `override` (undefined fields
 *  stripped); an all-empty override drops the entry (→ inherit globals). Backs
 *  the per-list settings UI. */
export function withListOverride(
  map: Record<string, ListOverrides>,
  listId: string,
  override: ListOverrides,
): Record<string, ListOverrides> {
  const trimmed: ListOverrides = {};
  if (override.cascade !== undefined) trimmed.cascade = override.cascade;
  if (override.autoDate !== undefined) trimmed.autoDate = override.autoDate;
  if (override.carryOverDefault !== undefined) {
    trimmed.carryOverDefault = override.carryOverDefault;
  }
  const next = { ...map };
  if (
    trimmed.cascade === undefined &&
    trimmed.autoDate === undefined &&
    trimmed.carryOverDefault === undefined
  ) {
    delete next[listId];
  } else {
    next[listId] = trimmed;
  }
  return next;
}

/** Whether the owning provider stores `in_progress` as a distinct state
 *  (`task_capabilities.supports_in_progress`, absent → true). When false, the
 *  cycle drops the in_progress step (it would revert to open on read-back) and
 *  auto-date is skipped (nothing would persist). */
export function canStoreInProgress(list: TaskList | undefined): boolean {
  return list?.task_capabilities?.supports_in_progress ?? true;
}

/**
 * Next status for a check-off, honouring the mode. Ported verbatim from the
 * desktop useTaskStatusToggle:
 *   - toggle (default): completed → open; anything else → completed.
 *   - cycle: open → in_progress → completed → open; a cancelled task re-enters
 *     at open. Drops the in_progress step where the provider can't store it.
 */
export function nextCheckoffStatus(
  current: TaskStatus,
  mode: CheckoffMode,
  canInProgress: boolean,
): TaskStatus {
  if (mode === 'cycle') {
    switch (current) {
      case 'open':
        return canInProgress ? 'in_progress' : 'completed';
      case 'in_progress':
        return 'completed';
      case 'completed':
        return 'open';
      default:
        return 'open';
    }
  }
  return current === 'completed' ? 'open' : 'completed';
}
