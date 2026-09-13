import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

import {
  countdownDaysToStore,
  dayWindowToStore,
  effectiveListSettings,
  NOTHING_STORED,
  readTaskSettings,
  withListOverride,
  type CarryOverDefault,
  type EffectiveListSettings,
  type ListOverrides,
  type PriorityScale,
} from '@aperio/shared';

import { getUserPref } from '../api/client';
import { TaskCascadeContext } from './taskCascadeContext';
import { useDebouncedPrefWrite } from './useDebouncedPrefWrite';
import { useUserPrefsChanged } from './useUserPrefsChanged';

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
 * Storage: string keys in `user_prefs`. How each stored string reads, and
 * what a change stores, is the core's (`cal_core::task_settings`, through
 * `@aperio/shared`); defaults apply when a key is missing, unparseable or
 * unreadable. The provider keeps the name
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

/**
 * Every key this provider owns, for the "a sync round wrote a preference"
 * listener. Kept beside the constants so a new knob that forgets this list
 * simply does not re-read — rather than the listener silently re-reading keys
 * that belong to somebody else.
 */
const OWNED_KEYS: readonly string[] = [
  CASCADE_KEY,
  AUTO_DATE_KEY,
  CARRY_OVER_KEY,
  DAY_START_TRIGGER_KEY,
  CHECKOFF_MODE_KEY,
  AUTO_SELF_ASSIGN_KEY,
  VISUAL_EFFORT_SIZING_KEY,
  TWO_LEVEL_PRIORITY_KEY,
  CALENDAR_DAY_VIEW_MODE_KEY,
  CALENDAR_DAY_START_MIN_KEY,
  CALENDAR_DAY_END_MIN_KEY,
  REMIND_UNTIMED_TODAY_KEY,
  REMIND_DEADLINE_ARRIVED_KEY,
  REMIND_DEADLINE_COUNTDOWN_KEY,
  DEADLINE_COUNTDOWN_DAYS_KEY,
  LIST_OVERRIDES_KEY,
];

const WRITE_DEBOUNCE_MS = 150;

export type { CarryOverDefault, EffectiveListSettings, ListOverrides } from '@aperio/shared';

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
 * Stored verbatim as a string. The core reads it as one of the five
 * offered values below, anything else as `'00:00'` — and the host's
 * all-day reminders read it the same way.
 */
export type DayStartTrigger = string;

/**
 * How the check-off gesture (Space / clicking the circle) advances a
 * task's status:
 *   - `'toggle'` (default): flip between `open` and `completed`, the
 *     historical behaviour.
 *   - `'cycle'`: step `open → in_progress → completed → open`, so a
 *     three-state workflow is reachable from the keyboard / one click.
 */
export type CheckoffMode = 'toggle' | 'cycle';

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
  /** Increments whenever these settings were re-read because a sync round
   *  brought a change from another device. Consumers that show the settings
   *  can use it to say so — a control that changes under the user's hands
   *  without a word is worse than the stale value it replaced. */
  prefRevision: number;
}

export function TaskCascadeProvider({ children }: { children: ReactNode }) {
  // Defaults are the "do what we've always done" behaviour so first
  // paint matches the legacy app even before user_prefs hydrates: what
  // nothing stored reads as, asked of the core once.
  const defaults = useMemo(() => readTaskSettings(NOTHING_STORED), []);
  const [enabled, setEnabledState] = useState(defaults.cascadeEnabled);
  const [autoDate, setAutoDateState] = useState(defaults.autoDate);
  const [autoSelfAssign, setAutoSelfAssignState] = useState(defaults.autoSelfAssign);
  // Visual effort-sizing defaults ON; only a literal stored 'false' disables.
  const [visualEffortSizing, setVisualEffortSizingState] = useState(defaults.visualEffortSizing);
  // Two-level priority defaults OFF (three levels, as before); only a literal
  // stored 'true' switches it on.
  const [twoLevelPriority, setTwoLevelPriorityState] = useState(defaults.twoLevelPriority);
  // Day-start reminder knobs. The three booleans default ON (only a
  // literal stored 'false' disables); the countdown lead time defaults
  // to 3 days (parsed + clamped 1..30 on hydrate).
  const [remindUntimedToday, setRemindUntimedTodayState] = useState(defaults.remindUntimedToday);
  const [remindDeadlineArrived, setRemindDeadlineArrivedState] = useState(
    defaults.remindDeadlineArrived,
  );
  const [remindDeadlineCountdown, setRemindDeadlineCountdownState] = useState(
    defaults.remindDeadlineCountdown,
  );
  const [deadlineCountdownDays, setDeadlineCountdownDaysState] = useState(
    defaults.deadlineCountdownDays,
  );
  // Calendar day/week layout defaults to the hour-grid; only a literal stored
  // 'list' switches to the compact list (anything else falls back to grid).
  const [dayViewMode, setDayViewModeState] = useState<CalendarDayViewMode>(defaults.dayViewMode);
  // Visible day window of the hour-grid. Defaults to the full day (the
  // historical behaviour); hydration + the setter snap to the half-hour grid.
  const [dayStartMin, setDayStartMinState] = useState<number>(defaults.dayStartMin);
  const [dayEndMin, setDayEndMinState] = useState<number>(defaults.dayEndMin);
  const [carryOverDefault, setCarryOverDefaultState] = useState<CarryOverDefault>(
    defaults.carryOverDefault,
  );
  // Default '00:00' means "as soon as the local date rolls over",
  // which is what users of always-on PCs expect.
  const [dayStartTrigger, setDayStartTriggerState] = useState<DayStartTrigger>(
    defaults.dayStartTrigger,
  );
  // Default 'toggle' = the historical open ↔ completed flip.
  const [checkoffMode, setCheckoffModeState] = useState<CheckoffMode>(defaults.checkoffMode);
  const [listOverrides, setListOverridesState] = useState<Record<string, ListOverrides>>(
    defaults.listOverrides,
  );
  const [hydrating, setHydrating] = useState(true);

  /**
   * A revision that increments whenever the values below came from STORAGE
   * rather than from the user — i.e. a sync round wrote one of our keys and
   * we re-read them. Passed to every write-back so it re-seeds its baseline
   * instead of restating a peer's change as our own.
   */
  const [prefRevision, setPrefRevision] = useState(0);

  /**
   * Read every key and apply it. Runs once at startup and again whenever a
   * sync round reports that one of them changed, so this must be idempotent
   * and must assign in BOTH directions: an early version only ever set the
   * non-default value ("if the stored string is 'false', turn it off"), which
   * is fine for a one-shot hydration and wrong for a re-read — a peer turning
   * something back ON would never have arrived.
   *
   * `external` marks the re-read case, and bumps the revision in the same
   * batch as the values so no write-back ever sees a new value with an old
   * revision.
   */
  const readAndApply = useCallback(
    async (isCancelled: () => boolean, external: boolean) => {
      const [
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
      ] = await Promise.all([
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
      ]);
      if (isCancelled()) return;
      // Read by the core: a default-on switch goes off only on a literal
      // 'false', the opt-in on only on 'true'; the numbers parse, snap and
      // clamp; the enumerations accept only their members; the override map
      // is checked per list and per field. A key whose read failed above is
      // simply not stored, so it alone falls back to its default.
      const read = readTaskSettings({
        cascade: cascadeRaw,
        autoDate: autoDateRaw,
        autoSelfAssign: autoSelfAssignRaw,
        visualEffortSizing: visualEffortSizingRaw,
        twoLevelPriority: twoLevelPriorityRaw,
        remindUntimedToday: remindUntimedTodayRaw,
        remindDeadlineArrived: remindDeadlineArrivedRaw,
        remindDeadlineCountdown: remindDeadlineCountdownRaw,
        deadlineCountdownDays: deadlineCountdownDaysRaw,
        dayViewMode: dayViewModeRaw,
        dayStartMin: dayStartMinRaw,
        dayEndMin: dayEndMinRaw,
        checkoffMode: checkoffRaw,
        carryOverDefault: carryOverRaw,
        dayStartTrigger: triggerRaw,
        listOverrides: listOverridesRaw,
      });
      setEnabledState(read.cascadeEnabled);
      setAutoDateState(read.autoDate);
      setAutoSelfAssignState(read.autoSelfAssign);
      setVisualEffortSizingState(read.visualEffortSizing);
      setTwoLevelPriorityState(read.twoLevelPriority);
      setRemindUntimedTodayState(read.remindUntimedToday);
      setRemindDeadlineArrivedState(read.remindDeadlineArrived);
      setRemindDeadlineCountdownState(read.remindDeadlineCountdown);
      setDeadlineCountdownDaysState(read.deadlineCountdownDays);
      setDayViewModeState(read.dayViewMode);
      setDayStartMinState(read.dayStartMin);
      setDayEndMinState(read.dayEndMin);
      setCarryOverDefaultState(read.carryOverDefault);
      setDayStartTriggerState(read.dayStartTrigger);
      setCheckoffModeState(read.checkoffMode);
      setListOverridesState(read.listOverrides);
      if (external) setPrefRevision((r) => r + 1);
    },
    [],
  );

  useEffect(() => {
    let cancelled = false;
    void readAndApply(() => cancelled, false).finally(() => {
      if (!cancelled) setHydrating(false);
    });
    return () => {
      cancelled = true;
    };
  }, [readAndApply]);

  // A sync round wrote one of our keys: read it back and adopt it. Without
  // this, a setting changed on another device sat in SQLite while this window
  // went on showing the old value until the next launch.
  useUserPrefsChanged(OWNED_KEYS, () => {
    void readAndApply(() => false, true);
  });

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

  useDebouncedPrefWrite(
    CASCADE_KEY,
    enabled ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    AUTO_DATE_KEY,
    autoDate ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    AUTO_SELF_ASSIGN_KEY,
    autoSelfAssign ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    VISUAL_EFFORT_SIZING_KEY,
    visualEffortSizing ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    TWO_LEVEL_PRIORITY_KEY,
    twoLevelPriority ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    REMIND_UNTIMED_TODAY_KEY,
    remindUntimedToday ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    REMIND_DEADLINE_ARRIVED_KEY,
    remindDeadlineArrived ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    REMIND_DEADLINE_COUNTDOWN_KEY,
    remindDeadlineCountdown ? 'true' : 'false',
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    DEADLINE_COUNTDOWN_DAYS_KEY,
    String(deadlineCountdownDays),
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    CALENDAR_DAY_VIEW_MODE_KEY,
    dayViewMode,
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  // The visible day window: state already holds validated, snapped minutes
  // (the setter and the hydrate parse both guarantee it), so this is just the
  // integer as a string.
  useDebouncedPrefWrite(
    CALENDAR_DAY_START_MIN_KEY,
    String(dayStartMin),
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    CALENDAR_DAY_END_MIN_KEY,
    String(dayEndMin),
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    CARRY_OVER_KEY,
    carryOverDefault,
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    DAY_START_TRIGGER_KEY,
    dayStartTrigger,
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  useDebouncedPrefWrite(
    CHECKOFF_MODE_KEY,
    checkoffMode,
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
  );
  // The override map compares as its JSON, which is exactly the comparison
  // that matters: a re-render that rebuilds an equal object writes nothing.
  useDebouncedPrefWrite(
    LIST_OVERRIDES_KEY,
    JSON.stringify(listOverrides),
    hydrating,
    WRITE_DEBOUNCE_MS,
    prefRevision,
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
    // Rounded and clamped on the way in too, so a stray UI value can never
    // persist out of range (defence in depth alongside the read).
    setDeadlineCountdownDaysState(countdownDaysToStore(value));
  }, []);
  const setDayViewMode = useCallback((value: CalendarDayViewMode) => {
    setDayViewModeState(value);
  }, []);
  const setDayWindow = useCallback((startMin: number, endMin: number) => {
    // Validate the pair on the way in (snap to half-hour, clamp, full-day
    // fallback when start >= end) so an invalid value can never reach state or
    // the debounced persistence — defence in depth alongside the hydrate parse.
    const win = dayWindowToStore(startMin, endMin);
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
        // The core strips absent fields, drops an emptied list and keeps the
        // map's key order; an unchanged map keeps its identity, so nothing
        // re-renders or writes.
        const next = withListOverride(prev, listId, override);
        return JSON.stringify(next) === JSON.stringify(prev) ? prev : next;
      });
    },
    [],
  );

  const effectiveForList = useCallback(
    (listId: string): EffectiveListSettings => {
      return effectiveListSettings(
        { cascadeEnabled: enabled, autoDate, carryOverDefault },
        listOverrides,
        listId,
      );
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
      prefRevision,
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
      prefRevision,
    ],
  );

  return (
    <TaskCascadeContext.Provider value={value}>
      {children}
    </TaskCascadeContext.Provider>
  );
}

