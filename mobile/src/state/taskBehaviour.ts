import type { TaskList, TaskStatus } from '@aperio/shared';

import { getUserPref, setUserPref } from '../api/prefs';

// The task-behaviour knobs (Settings → Tasks), SYNCED across the user's devices
// via the `tasks.*` user-prefs (§19.2.1 always-sync keys) — the mobile twin of
// the desktop TaskCascadeProvider. FIVE globals (check-off mode, status
// coupling, auto-date, carry-over default, day-start trigger) + a per-list
// override map (`tasks.listOverrides`). The check-off path reads the EFFECTIVE
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
const CHECKOFF_KEY = 'tasks.checkoffMode';
const CARRY_OVER_KEY = 'tasks.carryOverDefault';
const DAY_START_TRIGGER_KEY = 'tasks.dayStartTrigger';
const LIST_OVERRIDES_KEY = 'tasks.listOverrides';

/** The desktop defaults ("do what we've always done"). */
export const TASK_BEHAVIOUR_DEFAULTS: TaskBehaviour = {
  cascadeEnabled: true,
  autoDate: true,
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

function parseCheckoff(stored: string | null): CheckoffMode {
  return stored === 'cycle' ? 'cycle' : 'toggle';
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

/** Read all five synced knobs + the per-list override map, or the desktop
 *  defaults when unset/unreadable. */
export async function readTaskBehaviour(): Promise<TaskBehaviour> {
  try {
    const [cascade, autoDate, checkoff, carryOver, trigger, overrides] = await Promise.all([
      getUserPref(CASCADE_KEY),
      getUserPref(AUTO_DATE_KEY),
      getUserPref(CHECKOFF_KEY),
      getUserPref(CARRY_OVER_KEY),
      getUserPref(DAY_START_TRIGGER_KEY),
      getUserPref(LIST_OVERRIDES_KEY),
    ]);
    return {
      cascadeEnabled: parseBool(cascade),
      autoDate: parseBool(autoDate),
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
