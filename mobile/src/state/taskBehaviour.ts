import {
  countdownDaysToStore,
  dayWindowToStore,
  effectiveListSettings,
  NOTHING_STORED,
  readTaskSettings,
  type CarryOverDefault,
  type DayStartTriggerSetting,
  type EffectiveListSettings,
  type ListOverrides,
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
// review / deadline-pin checkers. How every stored string reads, and what is
// stored on a change, is the core's (`cal_core::task_settings`, through
// `@aperio/shared`); this module reads and writes the preference store.
// Best-effort: a key whose read fails falls back to its default alone; a write
// still applies for this session.

export type CheckoffMode = 'toggle' | 'cycle';
export type { CarryOverDefault, EffectiveListSettings, ListOverrides };
/** When the day-start checkers fire on a long-running app. */
export type DayStartTrigger = DayStartTriggerSetting;

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
// (parsed + clamped 1..30 on read).
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

let defaults: TaskBehaviour | null = null;

/** The desktop defaults ("do what we've always done") — what nothing stored
 *  reads as. Asked of the core once, on first use: never at import time, when
 *  the door is not bound yet. */
export function taskBehaviourDefaults(): TaskBehaviour {
  defaults ??= readTaskSettings(NOTHING_STORED);
  return defaults;
}

/** The priority system as the shared display + ordering helpers want it. */
export function priorityScaleFor(twoLevelPriority: boolean): PriorityScale {
  return twoLevelPriority ? 'two' : 'three';
}

/** One stored string, or null when unset — or when reading it failed, so that
 *  setting alone falls back to its default. */
const readOne = (key: string): Promise<string | null> => getUserPref(key).catch(() => null);

/** Read all synced knobs + the per-list override map. */
export async function readTaskBehaviour(): Promise<TaskBehaviour> {
  const [
    cascade,
    autoDate,
    autoSelfAssign,
    visualEffortSizing,
    twoLevelPriority,
    remindUntimedToday,
    remindDeadlineArrived,
    remindDeadlineCountdown,
    deadlineCountdownDays,
    dayViewMode,
    dayStartMin,
    dayEndMin,
    checkoffMode,
    carryOverDefault,
    dayStartTrigger,
    listOverrides,
  ] = await Promise.all([
    readOne(CASCADE_KEY),
    readOne(AUTO_DATE_KEY),
    readOne(AUTO_SELF_ASSIGN_KEY),
    readOne(VISUAL_EFFORT_SIZING_KEY),
    readOne(TWO_LEVEL_PRIORITY_KEY),
    readOne(REMIND_UNTIMED_TODAY_KEY),
    readOne(REMIND_DEADLINE_ARRIVED_KEY),
    readOne(REMIND_DEADLINE_COUNTDOWN_KEY),
    readOne(DEADLINE_COUNTDOWN_DAYS_KEY),
    readOne(CALENDAR_DAY_VIEW_MODE_KEY),
    readOne(CALENDAR_DAY_START_MIN_KEY),
    readOne(CALENDAR_DAY_END_MIN_KEY),
    readOne(CHECKOFF_KEY),
    readOne(CARRY_OVER_KEY),
    readOne(DAY_START_TRIGGER_KEY),
    readOne(LIST_OVERRIDES_KEY),
  ]);
  return readTaskSettings({
    cascade,
    autoDate,
    autoSelfAssign,
    visualEffortSizing,
    twoLevelPriority,
    remindUntimedToday,
    remindDeadlineArrived,
    remindDeadlineCountdown,
    deadlineCountdownDays,
    dayViewMode,
    dayStartMin,
    dayEndMin,
    checkoffMode,
    carryOverDefault,
    dayStartTrigger,
    listOverrides,
  });
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
/** Persist the countdown lead time (rounded, clamped 1..30) as a string. */
export const writeDeadlineCountdownDays = (v: number): Promise<void> =>
  writeBest(DEADLINE_COUNTDOWN_DAYS_KEY, String(countdownDaysToStore(v)));
export const writeDayViewMode = (m: 'grid' | 'list'): Promise<void> =>
  writeBest(CALENDAR_DAY_VIEW_MODE_KEY, m);
/** Persist the visible day window. The pair is validated (snapped to the
 *  half-hour grid, clamped; full-day fallback when start >= end) so an invalid
 *  value can never reach storage; both keys are written. */
export const writeDayWindow = async (
  startMin: number,
  endMin: number,
): Promise<void> => {
  const win = dayWindowToStore(startMin, endMin);
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
  return effectiveListSettings(b, b.listOverrides, listId);
}

/** A new override map with `listId` set to `override` (absent fields
 *  stripped); an all-empty override drops the entry (→ inherit globals). Backs
 *  the per-list settings UI. */
export { withListOverride } from '@aperio/shared';

/** Whether the owning provider stores `in_progress` as a distinct state
 *  (`task_capabilities.supports_in_progress`, absent → true). When false, the
 *  cycle drops the in_progress step (it would revert to open on read-back) and
 *  auto-date is skipped (nothing would persist). */
export function canStoreInProgress(list: TaskList | undefined): boolean {
  return list?.task_capabilities?.supports_in_progress ?? true;
}

/** Whether tasks in `list` can be filed into a section (a Vikunja bucket, a
 *  Todoist section). The desktop twin lives in `src/state/taskMoves.ts`; both
 *  read the adapter's own `sections` capability, which defaults to false —
 *  offering a picker a provider cannot store would lose the choice on save. */
export function canAssignSection(list: TaskList | undefined): boolean {
  return list?.task_capabilities?.sections ?? false;
}

/** Whether a task in `list` can carry a time of day at all. The desktop twin is
 *  `canSetTaskTime` in `src/state/taskMoves.ts`. Google Tasks, Microsoft To Do
 *  and Exchange keep whole days and drop anything finer without complaining, so
 *  the editor stops offering a time rather than losing it on the round trip. */
export function canSetTaskTime(list: TaskList | undefined): boolean {
  return list?.task_capabilities?.task_time_of_day ?? true;
}

/** Whether a task in `list` can carry the END of its planned block. The desktop
 *  twin is `canSetTaskSpan`. A span needs a time to hang off, so a source that
 *  cannot hold a time cannot hold a block either. */
export function canSetTaskSpan(list: TaskList | undefined): boolean {
  return (list?.task_capabilities?.task_span ?? false) && canSetTaskTime(list);
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
