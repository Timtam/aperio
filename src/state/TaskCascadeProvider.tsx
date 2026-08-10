import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

import {
  FULL_DAY_WINDOW,
  MINUTES_PER_DAY,
  type PriorityScale,
} from '@aperio/shared';

import { getUserPref } from '../api/client';
import { TaskCascadeContext } from './taskCascadeContext';
import { useDebouncedPrefWrite } from './useDebouncedPrefWrite';

/**
 * Global task-behaviour preferences. Owns three independent knobs the
 * user can flip from the Settings → Tasks tab; all of them affect how
 * Aperio reacts to status / scheduling changes across the app.
 *
 *   1. **Cascade-status-coupling** — when on (default), the planners
 *      in `taskCascade.ts` propagate status changes between a task
 *      and its descendants / ancestors. Off makes every task an
 *      island and the planners degrade to single-row writes.
 *
 *   2. **Auto-date-on-start** — when on (default), a backlog task
 *      transitioning into `in_progress` gets `scheduled_date` pinned
 *      to today. Off disables that pin; the planners are still
 *      called for the cascade, just without the `todayKey` option.
 *
 *   3. **Carry-over default action** — what `CarryOverChecker` does
 *      on app startup when there are tasks whose scheduled day has
 *      passed and are still open. `ask` (default) opens the
 *      dialog. `today` and `backlog` run a silent batch action
 *      and announce the outcome via the live region.
 *
 * Lives in one context so a single hydration round-trip serves every
 * consumer. The fields are independent — setting one doesn't touch
 * the others — and each has its own debounced persistence so a flurry
 * of clicks in the settings UI doesn't hammer SQLite.
 *
 * Storage: three string keys in `user_prefs`. Defaults apply when the
 * key is missing or unparseable. The provider keeps the name
 * `TaskCascadeProvider` for backwards compatibility with existing
 * imports — the public surface just gained two new fields.
 */

const CASCADE_KEY = 'tasks.cascadeStatusCoupling';
const AUTO_DATE_KEY = 'tasks.autoDateOnStart';
const CARRY_OVER_KEY = 'tasks.carryOverDefault';
const DAY_START_TRIGGER_KEY = 'tasks.dayStartTrigger';
const CHECKOFF_MODE_KEY = 'tasks.checkoffMode';
const AUTO_SELF_ASSIGN_KEY = 'tasks.autoSelfAssign';
const VISUAL_EFFORT_SIZING_KEY = 'tasks.visualEffortSizing';
// Two-level priority (normal / important) instead of low-medium-high. Default
// off = the three-level original; only a literal stored 'true' switches. A
// preference about how the user works, not about this device, so it rides the
// synced `tasks.` prefix like every other knob on the Tasks tab.
const TWO_LEVEL_PRIORITY_KEY = 'tasks.twoLevelPriority';
const CALENDAR_DAY_VIEW_MODE_KEY = 'calendar.dayViewMode';
// Visible day-window of the calendar hour-grid (synced). Two integer minute
// values from midnight stored as strings (e.g. "420" = 07:00). Parsed +
// validated on hydrate AND in the setter: clamped to [0, 1440], rounded to the
// nearest 30 (half-hour granularity); a start >= end pair falls back to the
// full day. The grid renderers consume `dayStartMin`/`dayEndMin`; this provider
// only owns the prefs.
const CALENDAR_DAY_START_MIN_KEY = 'calendar.dayStartMin';
const CALENDAR_DAY_END_MIN_KEY = 'calendar.dayEndMin';
// Day-start reminder knobs (synced). Three on/off booleans (default ON,
// only a literal stored 'false' disables) + a numeric "X days before"
// value stored as a string like dayStartTrigger (parsed + clamped 1..30
// on hydrate, fallback 3 on garbage). The reminder LOGIC that consumes
// these lands in a later step; this provider only owns the prefs.
const REMIND_UNTIMED_TODAY_KEY = 'tasks.remindUntimedToday';
const REMIND_DEADLINE_ARRIVED_KEY = 'tasks.remindDeadlineArrived';
const REMIND_DEADLINE_COUNTDOWN_KEY = 'tasks.remindDeadlineCountdown';
const DEADLINE_COUNTDOWN_DAYS_KEY = 'tasks.deadlineCountdownDays';
/**
 * Single JSON pref holding the per-list override map. Keyed by
 * task-list id, value is a `ListOverrides` record carrying any
 * subset of the three knobs the user wants to override for that
 * list. Missing keys / absent fields fall back to the global
 * default.
 *
 * Storing as one blob (rather than `tasks.list.{id}.*` keys per
 * field) keeps the hydration round-trip a single fetch and means
 * we don't have to enumerate all known lists to discover overrides
 * — the JSON blob is self-describing.
 */
const LIST_OVERRIDES_KEY = 'tasks.listOverrides';
const WRITE_DEBOUNCE_MS = 150;

/** Default "X days before a deadline" reminder lead time. */
const DEADLINE_COUNTDOWN_DAYS_DEFAULT = 3;
const DEADLINE_COUNTDOWN_DAYS_MIN = 1;
const DEADLINE_COUNTDOWN_DAYS_MAX = 30;

/**
 * Parse the stored "X days before" string and clamp it to 1..30.
 * Anything non-numeric / out of an integer range falls back to the
 * default of 3 — same defensive posture as the `dayStartTrigger`
 * string pref.
 */
function parseCountdownDays(stored: string | null): number {
  if (stored === null) return DEADLINE_COUNTDOWN_DAYS_DEFAULT;
  const n = Number.parseInt(stored, 10);
  if (!Number.isFinite(n)) return DEADLINE_COUNTDOWN_DAYS_DEFAULT;
  return Math.min(
    DEADLINE_COUNTDOWN_DAYS_MAX,
    Math.max(DEADLINE_COUNTDOWN_DAYS_MIN, n),
  );
}

/**
 * Snap a raw minute value to the visible-day-window grid: integer, clamped to
 * `[0, 1440]`, rounded to the nearest 30 (half-hour granularity). A non-finite
 * input falls back to `fallback` (already snapped).
 */
function snapWindowMinute(value: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  const rounded = Math.round(value / 30) * 30;
  return Math.min(MINUTES_PER_DAY, Math.max(0, rounded));
}

/**
 * Validate a `(start, end)` day-window pair. Each end is snapped to the
 * half-hour grid in `[0, 1440]`; if `start >= end` after snapping, the whole
 * pair falls back to the FULL day window (the historical behaviour). Used by
 * BOTH the hydration parse and the `setDayWindow` setter so an invalid value
 * can never reach state or storage.
 */
function validateDayWindow(
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

/**
 * Parse the two stored minute strings into a validated day-window. A
 * missing/garbage value parses to NaN, which `validateDayWindow` snaps to the
 * full-day default for that edge; an out-of-order pair falls back to the full
 * day.
 */
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

export type CarryOverDefault = 'ask' | 'today' | 'backlog';

/**
 * How the calendar's day + week views lay out events:
 *   - `'grid'` (default): the proportional 24h hour-grid — events
 *     absolute-positioned by start time, sized by duration.
 *   - `'list'`: a compact chronological list — events stack in normal
 *     flow, each block still sized by its duration (via
 *     `eventBlockFactor`), tasks sized by effort.
 *
 * Purely visual: the screen-reader model (roles, ids,
 * aria-activedescendant, keyboard, labels) is byte-for-byte identical
 * in both modes. Synced like `visualEffortSizing` so the choice
 * follows the user across devices.
 */
export type CalendarDayViewMode = 'grid' | 'list';

/**
 * When the three day-start checkers (CarryOver, MissedTasks,
 * DeadlinePin) should fire on a long-running app. Values:
 *
 *   - `'app-start'`: legacy mount-once. Fires only on initial
 *     launch — historical behaviour, opt-in for users who don't
 *     want re-fires while the app is running.
 *   - `'00:00'`: as soon as the local date rolls over (default).
 *   - Any other `HH:MM`: deferred to that morning hour on the new
 *     day so the user isn't woken up by a midnight dialog.
 *
 * Stored verbatim as a string; the runtime parses HH:MM with a
 * regex and falls back to immediate fire on garbage values.
 */
export type DayStartTrigger = string;

const DAY_START_TRIGGER_VALUES: readonly DayStartTrigger[] = [
  'app-start',
  '00:00',
  '06:00',
  '08:00',
  '12:00',
];

function isDayStartTrigger(value: unknown): value is DayStartTrigger {
  return (
    typeof value === 'string' &&
    (DAY_START_TRIGGER_VALUES as readonly string[]).includes(value)
  );
}

const CARRY_OVER_VALUES: readonly CarryOverDefault[] = [
  'ask',
  'today',
  'backlog',
];

function isCarryOverDefault(value: unknown): value is CarryOverDefault {
  return (
    typeof value === 'string' &&
    (CARRY_OVER_VALUES as readonly string[]).includes(value)
  );
}

/**
 * How the check-off gesture (Space / clicking the circle) advances a
 * task's status:
 *   - `'toggle'` (default): flip between `open` and `completed`, the
 *     historical behaviour.
 *   - `'cycle'`: step `open → in_progress → completed → open`, so a
 *     three-state workflow is reachable from the keyboard / one click.
 */
export type CheckoffMode = 'toggle' | 'cycle';

const CHECKOFF_MODE_VALUES: readonly CheckoffMode[] = ['toggle', 'cycle'];

function isCheckoffMode(value: unknown): value is CheckoffMode {
  return (
    typeof value === 'string' &&
    (CHECKOFF_MODE_VALUES as readonly string[]).includes(value)
  );
}

/**
 * Per-list override of any subset of the three task-behaviour
 * knobs. Absent fields inherit the corresponding global default —
 * a list with `{ carryOverDefault: 'today' }` keeps the global
 * cascade and auto-date and just changes the carry-over policy.
 *
 * Empty `{}` is semantically identical to "no override" but we
 * still drop the list key when all fields clear so the persisted
 * JSON stays minimal.
 */
export interface ListOverrides {
  cascade?: boolean;
  autoDate?: boolean;
  carryOverDefault?: CarryOverDefault;
}

/**
 * The merged values for a specific task list. `cascade`, `autoDate`,
 * `carryOverDefault` are guaranteed non-null — either inherited from
 * the global default or overridden by the per-list entry.
 */
export interface EffectiveListSettings {
  cascade: boolean;
  autoDate: boolean;
  carryOverDefault: CarryOverDefault;
}

export interface TaskCascadeContextValue {
  /** True when parent/subtask status coupling is active. */
  enabled: boolean;
  /** Set the cascade-coupling preference. Debounced-persisted. */
  setEnabled: (value: boolean) => void;
  /** True when "started → pin to today" auto-date is active. */
  autoDate: boolean;
  /** Set the auto-date preference. Debounced-persisted. */
  setAutoDate: (value: boolean) => void;
  /** True when "self-assign in shared lists on status change" is active. */
  autoSelfAssign: boolean;
  /** Set the auto-self-assign preference. Debounced-persisted. */
  setAutoSelfAssign: (value: boolean) => void;
  /** True when task tiles render at a size keyed off their effort. Purely
   *  visual; the effort is always in the SR label regardless. */
  visualEffortSizing: boolean;
  /** Set the visual-effort-sizing preference. Debounced-persisted (synced). */
  setVisualEffortSizing: (value: boolean) => void;
  /** True when the user's priority system has two levels (normal / important)
   *  rather than three. */
  twoLevelPriority: boolean;
  /** Set the two-level-priority preference. Debounced-persisted (synced). */
  setTwoLevelPriority: (value: boolean) => void;
  /** The same choice as {@link twoLevelPriority}, in the form every display /
   *  ordering helper in `@aperio/shared` takes — so no consumer has to spell
   *  the boolean→scale mapping out again. */
  priorityScale: PriorityScale;
  /** Remind of today's untimed (date-only) scheduled tasks. Default on. */
  remindUntimedToday: boolean;
  /** Set the untimed-today reminder preference. Debounced-persisted (synced). */
  setRemindUntimedToday: (value: boolean) => void;
  /** Remind when a task's deadline day has arrived. Default on. */
  remindDeadlineArrived: boolean;
  /** Set the deadline-arrived reminder preference. Debounced-persisted (synced). */
  setRemindDeadlineArrived: (value: boolean) => void;
  /** Remind X days before a deadline (the X is `deadlineCountdownDays`). Default on. */
  remindDeadlineCountdown: boolean;
  /** Set the deadline-countdown reminder preference. Debounced-persisted (synced). */
  setRemindDeadlineCountdown: (value: boolean) => void;
  /** Global "X days before a deadline" lead time (1..30). Default 3. */
  deadlineCountdownDays: number;
  /** Set the countdown lead time. Clamped 1..30. Debounced-persisted (synced). */
  setDeadlineCountdownDays: (value: number) => void;
  /** How the calendar day + week views lay out events ('grid' | 'list').
   *  Purely visual; the a11y model is identical in both modes. */
  dayViewMode: CalendarDayViewMode;
  /** Set the calendar day-view-mode preference. Debounced-persisted (synced). */
  setDayViewMode: (value: CalendarDayViewMode) => void;
  /** Visible-window START of the calendar hour-grid, minutes from midnight
   *  (half-hour grid, [0, 1440]). Default 0 (midnight). */
  dayStartMin: number;
  /** Visible-window END of the calendar hour-grid, minutes from midnight
   *  (half-hour grid, [0, 1440]). Default 1440 (end of day). */
  dayEndMin: number;
  /** Set the visible day window. The pair is validated (snapped to the
   *  half-hour grid, clamped, full-day fallback when start >= end) before it
   *  reaches state; both keys are debounced-persisted (synced). */
  setDayWindow: (startMin: number, endMin: number) => void;
  /** Carry-over default action used by `CarryOverChecker`. */
  carryOverDefault: CarryOverDefault;
  /** Set the carry-over default. Debounced-persisted. */
  setCarryOverDefault: (value: CarryOverDefault) => void;
  /** When the day-start checkers should fire on a long-running app. */
  dayStartTrigger: DayStartTrigger;
  /** Set the day-start-trigger preference. Debounced-persisted. */
  setDayStartTrigger: (value: DayStartTrigger) => void;
  /** How the check-off gesture advances a task's status. */
  checkoffMode: CheckoffMode;
  /** Set the check-off mode preference. Debounced-persisted. */
  setCheckoffMode: (value: CheckoffMode) => void;
  /** Per-list overrides for the cascade / auto-date / carry-over
   *  knobs. Keyed by task-list id. Absent keys mean "inherit". */
  listOverrides: Record<string, ListOverrides>;
  /** Replace the override entry for one list. Pass an empty object
   *  (or all-absent fields) to clear the override for that list —
   *  the entry is dropped from the persisted JSON and consumers
   *  fall back to the globals. */
  setListOverride: (listId: string, override: ListOverrides) => void;
  /** Resolve the effective {cascade, autoDate, carryOverDefault}
   *  values for a single list — per-list override wins per field,
   *  otherwise the global default applies. The dayStartTrigger is
   *  intentionally NOT per-list (it's a clock-time pref about WHEN
   *  the day-start checkers fire, not per-list behaviour). */
  effectiveForList: (listId: string) => EffectiveListSettings;
  /** True until the initial hydration round-trip returns. */
  hydrating: boolean;
}

export function TaskCascadeProvider({ children }: { children: ReactNode }) {
  // Defaults are the "do what we've always done" behaviour so first
  // paint matches the legacy app even before user_prefs hydrates.
  const [enabled, setEnabledState] = useState(true);
  const [autoDate, setAutoDateState] = useState(true);
  const [autoSelfAssign, setAutoSelfAssignState] = useState(true);
  // Visual effort-sizing defaults ON; only a literal stored 'false' disables.
  const [visualEffortSizing, setVisualEffortSizingState] = useState(true);
  // Two-level priority defaults OFF (three levels, as before); only a literal
  // stored 'true' switches it on.
  const [twoLevelPriority, setTwoLevelPriorityState] = useState(false);
  // Day-start reminder knobs. The three booleans default ON (only a
  // literal stored 'false' disables); the countdown lead time defaults
  // to 3 days (parsed + clamped 1..30 on hydrate).
  const [remindUntimedToday, setRemindUntimedTodayState] = useState(true);
  const [remindDeadlineArrived, setRemindDeadlineArrivedState] = useState(true);
  const [remindDeadlineCountdown, setRemindDeadlineCountdownState] =
    useState(true);
  const [deadlineCountdownDays, setDeadlineCountdownDaysState] = useState(
    DEADLINE_COUNTDOWN_DAYS_DEFAULT,
  );
  // Calendar day/week layout defaults to the hour-grid; only a literal stored
  // 'list' switches to the compact list (anything else falls back to grid).
  const [dayViewMode, setDayViewModeState] =
    useState<CalendarDayViewMode>('grid');
  // Visible day window of the hour-grid. Defaults to the full day (the
  // historical behaviour); hydration + the setter snap to the half-hour grid.
  const [dayStartMin, setDayStartMinState] = useState<number>(
    FULL_DAY_WINDOW.startMin,
  );
  const [dayEndMin, setDayEndMinState] = useState<number>(
    FULL_DAY_WINDOW.endMin,
  );
  const [carryOverDefault, setCarryOverDefaultState] =
    useState<CarryOverDefault>('ask');
  // Default '00:00' means "as soon as the local date rolls over",
  // which is what users of always-on PCs expect.
  const [dayStartTrigger, setDayStartTriggerState] =
    useState<DayStartTrigger>('00:00');
  // Default 'toggle' = the historical open ↔ completed flip.
  const [checkoffMode, setCheckoffModeState] =
    useState<CheckoffMode>('toggle');
  const [listOverrides, setListOverridesState] = useState<
    Record<string, ListOverrides>
  >({});
  const [hydrating, setHydrating] = useState(true);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      getUserPref(CASCADE_KEY).catch(() => null),
      getUserPref(AUTO_DATE_KEY).catch(() => null),
      getUserPref(CARRY_OVER_KEY).catch(() => null),
      getUserPref(DAY_START_TRIGGER_KEY).catch(() => null),
      getUserPref(CHECKOFF_MODE_KEY).catch(() => null),
      getUserPref(AUTO_SELF_ASSIGN_KEY).catch(() => null),
      getUserPref(VISUAL_EFFORT_SIZING_KEY).catch(() => null),
      getUserPref(TWO_LEVEL_PRIORITY_KEY).catch(() => null),
      getUserPref(REMIND_UNTIMED_TODAY_KEY).catch(() => null),
      getUserPref(REMIND_DEADLINE_ARRIVED_KEY).catch(() => null),
      getUserPref(REMIND_DEADLINE_COUNTDOWN_KEY).catch(() => null),
      getUserPref(DEADLINE_COUNTDOWN_DAYS_KEY).catch(() => null),
      getUserPref(CALENDAR_DAY_VIEW_MODE_KEY).catch(() => null),
      getUserPref(CALENDAR_DAY_START_MIN_KEY).catch(() => null),
      getUserPref(CALENDAR_DAY_END_MIN_KEY).catch(() => null),
      getUserPref(LIST_OVERRIDES_KEY).catch(() => null),
    ])
      .then(
        ([
          cascadeRaw,
          autoDateRaw,
          carryOverRaw,
          triggerRaw,
          checkoffRaw,
          autoSelfAssignRaw,
          visualEffortSizingRaw,
          twoLevelPriorityRaw,
          remindUntimedTodayRaw,
          remindDeadlineArrivedRaw,
          remindDeadlineCountdownRaw,
          deadlineCountdownDaysRaw,
          dayViewModeRaw,
          dayStartMinRaw,
          dayEndMinRaw,
          listOverridesRaw,
        ]) => {
          if (cancelled) return;
          // Cascade + auto-date follow the same on/off convention as
          // before — only literal "false" toggles the default off.
          if (cascadeRaw === 'false') setEnabledState(false);
          if (autoDateRaw === 'false') setAutoDateState(false);
          if (autoSelfAssignRaw === 'false') setAutoSelfAssignState(false);
          if (visualEffortSizingRaw === 'false')
            setVisualEffortSizingState(false);
          // Two-level priority is the opt-IN, so only a literal 'true' flips it.
          if (twoLevelPriorityRaw === 'true') setTwoLevelPriorityState(true);
          // Day-start reminder booleans default ON; only a literal
          // 'false' disables them (same convention as the others).
          if (remindUntimedTodayRaw === 'false')
            setRemindUntimedTodayState(false);
          if (remindDeadlineArrivedRaw === 'false')
            setRemindDeadlineArrivedState(false);
          if (remindDeadlineCountdownRaw === 'false')
            setRemindDeadlineCountdownState(false);
          // Countdown lead time: parse + clamp 1..30, fall back to 3.
          setDeadlineCountdownDaysState(
            parseCountdownDays(deadlineCountdownDaysRaw),
          );
          // Calendar day-view mode: only a literal stored 'list' switches
          // away from the grid default; anything else keeps 'grid'.
          if (dayViewModeRaw === 'list') setDayViewModeState('list');
          // Visible day window: parse + validate the two minute strings as a
          // pair (snap to the half-hour grid, clamp; full-day fallback when
          // start >= end). Always set both so a one-sided stored value still
          // lands on a consistent window.
          {
            const win = parseDayWindow(dayStartMinRaw, dayEndMinRaw);
            setDayStartMinState(win.startMin);
            setDayEndMinState(win.endMin);
          }
          // Carry-over default is a tri-state enum; reject anything
          // that doesn't match the allowed values and keep the default.
          if (isCarryOverDefault(carryOverRaw)) {
            setCarryOverDefaultState(carryOverRaw);
          }
          // Day-start trigger: same approach — accept only the known
          // enum members. Garbage falls back to the default '00:00'.
          if (isDayStartTrigger(triggerRaw)) {
            setDayStartTriggerState(triggerRaw);
          }
          // Check-off mode: accept only the known enum members; garbage
          // falls back to the default 'toggle'.
          if (isCheckoffMode(checkoffRaw)) {
            setCheckoffModeState(checkoffRaw);
          }
          // Per-list overrides: a JSON blob of `Record<listId,
          // ListOverrides>`. Validate per-list-per-field so a corrupt
          // entry for one list doesn't poison the others.
          if (listOverridesRaw) {
            try {
              const parsed = JSON.parse(listOverridesRaw) as unknown;
              if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
                const sanitised: Record<string, ListOverrides> = {};
                for (const [listId, raw] of Object.entries(parsed)) {
                  if (!raw || typeof raw !== 'object') continue;
                  const entry: ListOverrides = {};
                  const r = raw as Record<string, unknown>;
                  if (typeof r.cascade === 'boolean') entry.cascade = r.cascade;
                  if (typeof r.autoDate === 'boolean') entry.autoDate = r.autoDate;
                  if (isCarryOverDefault(r.carryOverDefault)) {
                    entry.carryOverDefault = r.carryOverDefault;
                  }
                  // Drop entries with no surviving fields so the
                  // in-memory map matches what we'd persist.
                  if (
                    entry.cascade !== undefined ||
                    entry.autoDate !== undefined ||
                    entry.carryOverDefault !== undefined
                  ) {
                    sanitised[listId] = entry;
                  }
                }
                setListOverridesState(sanitised);
              }
            } catch {
              // Bad JSON; leave the map empty so consumers fall back
              // to globals. The next write will overwrite the
              // corrupt value.
            }
          }
        },
      )
      .finally(() => {
        if (!cancelled) setHydrating(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // ── Persistence: debounced, and only when something actually changed ──
  //
  // One call per knob, all of them through `useDebouncedPrefWrite`, which
  // keeps a baseline of what storage holds and stays silent when the value
  // matches it. That silence is the point: these keys SYNC, `set_user_pref`
  // appends an event for every write without comparing values, and conflicts
  // resolve by "later wins" — so the write-back that used to fire the moment
  // hydration finished stamped a fresh timestamp on an old choice and could
  // beat a genuine change made on another device. See the hook's own comment.
  //
  // Serialisation stays here, next to the parse that reads it back, so the two
  // cannot drift apart.

  useDebouncedPrefWrite(CASCADE_KEY, enabled ? 'true' : 'false', hydrating, WRITE_DEBOUNCE_MS);
  useDebouncedPrefWrite(AUTO_DATE_KEY, autoDate ? 'true' : 'false', hydrating, WRITE_DEBOUNCE_MS);
  useDebouncedPrefWrite(
    AUTO_SELF_ASSIGN_KEY,
    autoSelfAssign ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
  );
  useDebouncedPrefWrite(
    VISUAL_EFFORT_SIZING_KEY,
    visualEffortSizing ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
  );
  useDebouncedPrefWrite(
    TWO_LEVEL_PRIORITY_KEY,
    twoLevelPriority ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
  );
  useDebouncedPrefWrite(
    REMIND_UNTIMED_TODAY_KEY,
    remindUntimedToday ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
  );
  useDebouncedPrefWrite(
    REMIND_DEADLINE_ARRIVED_KEY,
    remindDeadlineArrived ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
  );
  useDebouncedPrefWrite(
    REMIND_DEADLINE_COUNTDOWN_KEY,
    remindDeadlineCountdown ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
  );
  useDebouncedPrefWrite(
    DEADLINE_COUNTDOWN_DAYS_KEY,
    String(deadlineCountdownDays),
    hydrating,
    WRITE_DEBOUNCE_MS,
  );
  useDebouncedPrefWrite(CALENDAR_DAY_VIEW_MODE_KEY, dayViewMode, hydrating, WRITE_DEBOUNCE_MS);
  // The visible day window: state already holds validated, snapped minutes
  // (the setter and the hydrate parse both guarantee it), so this is just the
  // integer as a string.
  useDebouncedPrefWrite(
    CALENDAR_DAY_START_MIN_KEY,
    String(dayStartMin),
    hydrating,
    WRITE_DEBOUNCE_MS,
  );
  useDebouncedPrefWrite(CALENDAR_DAY_END_MIN_KEY, String(dayEndMin), hydrating, WRITE_DEBOUNCE_MS);
  useDebouncedPrefWrite(CARRY_OVER_KEY, carryOverDefault, hydrating, WRITE_DEBOUNCE_MS);
  useDebouncedPrefWrite(DAY_START_TRIGGER_KEY, dayStartTrigger, hydrating, WRITE_DEBOUNCE_MS);
  useDebouncedPrefWrite(CHECKOFF_MODE_KEY, checkoffMode, hydrating, WRITE_DEBOUNCE_MS);
  // The override map compares as its JSON, which is exactly the comparison
  // that matters: a re-render that rebuilds an equal object writes nothing.
  useDebouncedPrefWrite(
    LIST_OVERRIDES_KEY,
    JSON.stringify(listOverrides),
    hydrating,
    WRITE_DEBOUNCE_MS,
  );

  const setEnabled = useCallback((value: boolean) => {
    setEnabledState(value);
  }, []);
  const setAutoDate = useCallback((value: boolean) => {
    setAutoDateState(value);
  }, []);
  const setAutoSelfAssign = useCallback((value: boolean) => {
    setAutoSelfAssignState(value);
  }, []);
  const setVisualEffortSizing = useCallback((value: boolean) => {
    setVisualEffortSizingState(value);
  }, []);
  const setTwoLevelPriority = useCallback((value: boolean) => {
    setTwoLevelPriorityState(value);
  }, []);
  // The boolean, said the way the shared helpers want to hear it.
  const priorityScale: PriorityScale = twoLevelPriority ? 'two' : 'three';
  const setRemindUntimedToday = useCallback((value: boolean) => {
    setRemindUntimedTodayState(value);
  }, []);
  const setRemindDeadlineArrived = useCallback((value: boolean) => {
    setRemindDeadlineArrivedState(value);
  }, []);
  const setRemindDeadlineCountdown = useCallback((value: boolean) => {
    setRemindDeadlineCountdownState(value);
  }, []);
  const setDeadlineCountdownDays = useCallback((value: number) => {
    // Clamp on the way in too, so a stray UI value can never persist
    // out of range (defence in depth alongside the hydration clamp).
    const clamped = Math.min(
      DEADLINE_COUNTDOWN_DAYS_MAX,
      Math.max(
        DEADLINE_COUNTDOWN_DAYS_MIN,
        Number.isFinite(value)
          ? Math.round(value)
          : DEADLINE_COUNTDOWN_DAYS_DEFAULT,
      ),
    );
    setDeadlineCountdownDaysState(clamped);
  }, []);
  const setDayViewMode = useCallback((value: CalendarDayViewMode) => {
    setDayViewModeState(value);
  }, []);
  const setDayWindow = useCallback((startMin: number, endMin: number) => {
    // Validate the pair on the way in (snap to half-hour, clamp, full-day
    // fallback when start >= end) so an invalid value can never reach state or
    // the debounced persistence — defence in depth alongside the hydrate parse.
    const win = validateDayWindow(startMin, endMin);
    setDayStartMinState(win.startMin);
    setDayEndMinState(win.endMin);
  }, []);
  const setCarryOverDefault = useCallback((value: CarryOverDefault) => {
    setCarryOverDefaultState(value);
  }, []);
  const setDayStartTrigger = useCallback((value: DayStartTrigger) => {
    setDayStartTriggerState(value);
  }, []);
  const setCheckoffMode = useCallback((value: CheckoffMode) => {
    setCheckoffModeState(value);
  }, []);

  const setListOverride = useCallback(
    (listId: string, override: ListOverrides) => {
      setListOverridesState((prev) => {
        // Strip undefined fields so the persisted map matches the
        // in-memory shape.
        const trimmed: ListOverrides = {};
        if (override.cascade !== undefined) trimmed.cascade = override.cascade;
        if (override.autoDate !== undefined) trimmed.autoDate = override.autoDate;
        if (override.carryOverDefault !== undefined) {
          trimmed.carryOverDefault = override.carryOverDefault;
        }
        const isEmpty =
          trimmed.cascade === undefined &&
          trimmed.autoDate === undefined &&
          trimmed.carryOverDefault === undefined;
        if (isEmpty) {
          // Drop the list entry entirely — falls back to globals
          // for every field, identical to "no override".
          if (prev[listId] === undefined) return prev;
          const next = { ...prev };
          delete next[listId];
          return next;
        }
        return { ...prev, [listId]: trimmed };
      });
    },
    [],
  );

  const effectiveForList = useCallback(
    (listId: string): EffectiveListSettings => {
      const override = listOverrides[listId];
      return {
        cascade: override?.cascade ?? enabled,
        autoDate: override?.autoDate ?? autoDate,
        carryOverDefault: override?.carryOverDefault ?? carryOverDefault,
      };
    },
    [listOverrides, enabled, autoDate, carryOverDefault],
  );

  const value = useMemo<TaskCascadeContextValue>(
    () => ({
      enabled,
      setEnabled,
      autoDate,
      setAutoDate,
      autoSelfAssign,
      setAutoSelfAssign,
      visualEffortSizing,
      setVisualEffortSizing,
      twoLevelPriority,
      setTwoLevelPriority,
      priorityScale,
      remindUntimedToday,
      setRemindUntimedToday,
      remindDeadlineArrived,
      setRemindDeadlineArrived,
      remindDeadlineCountdown,
      setRemindDeadlineCountdown,
      deadlineCountdownDays,
      setDeadlineCountdownDays,
      dayViewMode,
      setDayViewMode,
      dayStartMin,
      dayEndMin,
      setDayWindow,
      carryOverDefault,
      setCarryOverDefault,
      dayStartTrigger,
      setDayStartTrigger,
      checkoffMode,
      setCheckoffMode,
      listOverrides,
      setListOverride,
      effectiveForList,
      hydrating,
    }),
    [
      enabled,
      setEnabled,
      autoDate,
      setAutoDate,
      autoSelfAssign,
      setAutoSelfAssign,
      visualEffortSizing,
      setVisualEffortSizing,
      twoLevelPriority,
      setTwoLevelPriority,
      priorityScale,
      remindUntimedToday,
      setRemindUntimedToday,
      remindDeadlineArrived,
      setRemindDeadlineArrived,
      remindDeadlineCountdown,
      setRemindDeadlineCountdown,
      deadlineCountdownDays,
      setDeadlineCountdownDays,
      dayViewMode,
      setDayViewMode,
      dayStartMin,
      dayEndMin,
      setDayWindow,
      carryOverDefault,
      setCarryOverDefault,
      dayStartTrigger,
      setDayStartTrigger,
      checkoffMode,
      setCheckoffMode,
      listOverrides,
      setListOverride,
      effectiveForList,
      hydrating,
    ],
  );

  return (
    <TaskCascadeContext.Provider value={value}>
      {children}
    </TaskCascadeContext.Provider>
  );
}

