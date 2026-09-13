// The task settings — this surface's door into `cal_core::task_settings`.
//
// How the stored task and calendar preferences read, what a list's effective
// coupling, auto-date and carry-over are, and what is stored when a setting
// changes: that decision lives in the core now, and both surfaces ask it — the
// desktop `TaskCascadeProvider` and the mobile `taskBehaviour.ts`, which used
// to parse every value with their own copy of every rule. What stays with each
// surface is reading and writing the preference store: the core gets the
// stored strings, and a key whose read failed is handed over as not stored, so
// only that setting falls back to its default.
//
// Pinned by `crates/cal-core/tests/fixtures/taskSettings.json`, measured on the
// real code of both surfaces before the port; `taskSettings.contract.test.tsx`
// replays it through both surfaces, and the core's own contract test reads the
// same file.

import type {
  CarryOverDefault,
  DayWindowMinutes,
  TaskListEffective,
  TaskListOverride,
  TaskListOverrideEntry,
  TaskSettingsQuestion,
  TaskSettingsRead,
} from './types';

// ─────────────────────────────── The door ───────────────────────────────────

/** This surface's door into `cal_core::task_settings`: one question, one answer. */
export interface TaskSettingsRules {
  taskSettingsJson(inputJson: string): string;
}

let installedRules: TaskSettingsRules | null = null;

/** Bind this surface's door into the core. */
export function installTaskSettingsRules(rules: TaskSettingsRules): void {
  installedRules = rules;
}

function ask<T>(question: TaskSettingsQuestion): T {
  if (installedRules === null) {
    // Loud, not a local fallback. A fallback here would be the copy all over
    // again, and its failure is one nobody reports: a setting that reads one
    // way on the phone and another on the desktop.
    throw new Error(
      'task settings rules used before installTaskSettingsRules() — the surface ' +
        'must bind its door into cal_core::task_settings at startup',
    );
  }
  return JSON.parse(installedRules.taskSettingsJson(JSON.stringify(question))) as T;
}

// ─────────────────────────────── The shapes ─────────────────────────────────

/** The day-start triggers the surfaces offer. */
export type DayStartTriggerSetting = 'app-start' | '00:00' | '06:00' | '08:00' | '12:00';

/** Per-list override of any subset of the three per-list settings; an absent
 *  field inherits the global. */
export interface ListOverrides {
  cascade?: boolean;
  autoDate?: boolean;
  carryOverDefault?: CarryOverDefault;
}

/** A list's settings in effect. */
export interface EffectiveListSettings {
  cascade: boolean;
  autoDate: boolean;
  carryOverDefault: CarryOverDefault;
}

/** The stored strings, one per preference; `null` when not stored, or when
 *  reading it failed. */
export interface StoredTaskSettings {
  /** `tasks.cascadeStatusCoupling` */
  cascade: string | null;
  /** `tasks.autoDateOnStart` */
  autoDate: string | null;
  /** `tasks.autoSelfAssign` */
  autoSelfAssign: string | null;
  /** `tasks.visualEffortSizing` */
  visualEffortSizing: string | null;
  /** `tasks.twoLevelPriority` */
  twoLevelPriority: string | null;
  /** `tasks.remindUntimedToday` */
  remindUntimedToday: string | null;
  /** `tasks.remindDeadlineArrived` */
  remindDeadlineArrived: string | null;
  /** `tasks.remindDeadlineCountdown` */
  remindDeadlineCountdown: string | null;
  /** `tasks.deadlineCountdownDays` */
  deadlineCountdownDays: string | null;
  /** `calendar.dayViewMode` */
  dayViewMode: string | null;
  /** `calendar.dayStartMin` */
  dayStartMin: string | null;
  /** `calendar.dayEndMin` */
  dayEndMin: string | null;
  /** `tasks.checkoffMode` */
  checkoffMode: string | null;
  /** `tasks.carryOverDefault` */
  carryOverDefault: string | null;
  /** `tasks.dayStartTrigger` */
  dayStartTrigger: string | null;
  /** `tasks.listOverrides` */
  listOverrides: string | null;
}

/** Nothing stored: reading it gives the defaults. */
export const NOTHING_STORED: StoredTaskSettings = {
  cascade: null,
  autoDate: null,
  autoSelfAssign: null,
  visualEffortSizing: null,
  twoLevelPriority: null,
  remindUntimedToday: null,
  remindDeadlineArrived: null,
  remindDeadlineCountdown: null,
  deadlineCountdownDays: null,
  dayViewMode: null,
  dayStartMin: null,
  dayEndMin: null,
  checkoffMode: null,
  carryOverDefault: null,
  dayStartTrigger: null,
  listOverrides: null,
};

/** The settings, read. */
export interface TaskSettings {
  cascadeEnabled: boolean;
  autoDate: boolean;
  autoSelfAssign: boolean;
  visualEffortSizing: boolean;
  twoLevelPriority: boolean;
  remindUntimedToday: boolean;
  remindDeadlineArrived: boolean;
  remindDeadlineCountdown: boolean;
  /** 1..30. */
  deadlineCountdownDays: number;
  dayViewMode: 'grid' | 'list';
  /** Minutes from midnight, on the half hour. */
  dayStartMin: number;
  /** Minutes from midnight, on the half hour, after the start. */
  dayEndMin: number;
  checkoffMode: 'toggle' | 'cycle';
  carryOverDefault: CarryOverDefault;
  dayStartTrigger: DayStartTriggerSetting;
  /** Keyed by list id; every entry keeps at least one field. */
  listOverrides: Record<string, ListOverrides>;
}

function toWire(o: ListOverrides): TaskListOverride {
  return {
    ...(o.cascade === undefined ? {} : { cascade: o.cascade }),
    ...(o.autoDate === undefined ? {} : { auto_date: o.autoDate }),
    ...(o.carryOverDefault === undefined ? {} : { carry_over_default: o.carryOverDefault }),
  };
}

/** The fields in their fixed order: coupling, auto-date, carry-over. */
function fromWire(o: TaskListOverride): ListOverrides {
  const out: ListOverrides = {};
  if (o.cascade !== undefined) out.cascade = o.cascade;
  if (o.auto_date !== undefined) out.autoDate = o.auto_date;
  if (o.carry_over_default !== undefined) out.carryOverDefault = o.carry_over_default;
  return out;
}

function entriesOf(overrides: Record<string, ListOverrides>): TaskListOverrideEntry[] {
  return Object.entries(overrides).map(([list_id, o]) => ({ list_id, settings: toWire(o) }));
}

/** A record with every list as an own field — `__proto__` included, which an
 *  assignment would have turned into the prototype. */
function recordOf(entries: TaskListOverrideEntry[]): Record<string, ListOverrides> {
  return Object.fromEntries(entries.map((e) => [e.list_id, fromWire(e.settings)]));
}

/** A number as the wire can carry it: `NaN` and the infinities travel as null. */
const finite = (value: number): number | null => (Number.isFinite(value) ? value : null);

// ─────────────────────────────── The rules ──────────────────────────────────

/** The settings from their stored strings. */
export function readTaskSettings(stored: StoredTaskSettings): TaskSettings {
  const r = ask<TaskSettingsRead>({
    rule: 'read',
    stored: {
      cascade: stored.cascade,
      auto_date: stored.autoDate,
      auto_self_assign: stored.autoSelfAssign,
      visual_effort_sizing: stored.visualEffortSizing,
      two_level_priority: stored.twoLevelPriority,
      remind_untimed_today: stored.remindUntimedToday,
      remind_deadline_arrived: stored.remindDeadlineArrived,
      remind_deadline_countdown: stored.remindDeadlineCountdown,
      deadline_countdown_days: stored.deadlineCountdownDays,
      day_view_mode: stored.dayViewMode,
      day_start_min: stored.dayStartMin,
      day_end_min: stored.dayEndMin,
      checkoff_mode: stored.checkoffMode,
      carry_over_default: stored.carryOverDefault,
      day_start_trigger: stored.dayStartTrigger,
      list_overrides: stored.listOverrides,
    },
  });
  return {
    cascadeEnabled: r.cascade_enabled,
    autoDate: r.auto_date,
    autoSelfAssign: r.auto_self_assign,
    visualEffortSizing: r.visual_effort_sizing,
    twoLevelPriority: r.two_level_priority,
    remindUntimedToday: r.remind_untimed_today,
    remindDeadlineArrived: r.remind_deadline_arrived,
    remindDeadlineCountdown: r.remind_deadline_countdown,
    deadlineCountdownDays: r.deadline_countdown_days,
    dayViewMode: r.day_view_mode,
    dayStartMin: r.day_start_min,
    dayEndMin: r.day_end_min,
    checkoffMode: r.checkoff_mode,
    carryOverDefault: r.carry_over_default,
    dayStartTrigger: r.day_start_trigger as DayStartTriggerSetting,
    listOverrides: recordOf(r.list_overrides),
  };
}

/** A list's settings in effect: its override per field, else the global. */
export function effectiveListSettings(
  globals: { cascadeEnabled: boolean; autoDate: boolean; carryOverDefault: CarryOverDefault },
  overrides: Record<string, ListOverrides>,
  listId: string,
): EffectiveListSettings {
  const own = Object.hasOwn(overrides, listId) ? overrides[listId] : undefined;
  const e = ask<TaskListEffective>({
    rule: 'effective',
    globals: {
      cascade_enabled: globals.cascadeEnabled,
      auto_date: globals.autoDate,
      carry_over_default: globals.carryOverDefault,
    },
    list_override: own === undefined ? null : toWire(own),
  });
  return { cascade: e.cascade, autoDate: e.auto_date, carryOverDefault: e.carry_over_default };
}

/** The countdown window as it is stored: rounded, clamped to 1..30, the
 *  default 3 for a number that is not finite. */
export function countdownDaysToStore(value: number): number {
  return ask<number>({ rule: 'countdown_days_to_store', value: finite(value) });
}

/** The visible day window as it is stored: snapped to the half hour, clamped
 *  to the day, the whole day when the start is not before the end. */
export function dayWindowToStore(
  startMin: number,
  endMin: number,
): { startMin: number; endMin: number } {
  const w = ask<DayWindowMinutes>({
    rule: 'day_window_to_store',
    start_min: finite(startMin),
    end_min: finite(endMin),
  });
  return { startMin: w.start_min, endMin: w.end_min };
}

/** The override map with `listId` set to `override`: absent fields stripped,
 *  an emptied list dropped, an existing list keeping its place. */
export function withListOverride(
  overrides: Record<string, ListOverrides>,
  listId: string,
  override: ListOverrides,
): Record<string, ListOverrides> {
  return recordOf(
    ask<TaskListOverrideEntry[]>({
      rule: 'with_list_override',
      list_overrides: entriesOf(overrides),
      list_id: listId,
      list_override: toWire(override),
    }),
  );
}
