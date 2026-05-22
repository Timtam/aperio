import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';

import { getUserPref, setUserPref } from '../api/client';

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

export type CarryOverDefault = 'ask' | 'today' | 'backlog';

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

interface TaskCascadeContextValue {
  /** True when parent/subtask status coupling is active. */
  enabled: boolean;
  /** Set the cascade-coupling preference. Debounced-persisted. */
  setEnabled: (value: boolean) => void;
  /** True when "started → pin to today" auto-date is active. */
  autoDate: boolean;
  /** Set the auto-date preference. Debounced-persisted. */
  setAutoDate: (value: boolean) => void;
  /** Carry-over default action used by `CarryOverChecker`. */
  carryOverDefault: CarryOverDefault;
  /** Set the carry-over default. Debounced-persisted. */
  setCarryOverDefault: (value: CarryOverDefault) => void;
  /** When the day-start checkers should fire on a long-running app. */
  dayStartTrigger: DayStartTrigger;
  /** Set the day-start-trigger preference. Debounced-persisted. */
  setDayStartTrigger: (value: DayStartTrigger) => void;
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

const TaskCascadeContext = createContext<TaskCascadeContextValue | null>(null);

export function TaskCascadeProvider({ children }: { children: ReactNode }) {
  // Defaults are the "do what we've always done" behaviour so first
  // paint matches the legacy app even before user_prefs hydrates.
  const [enabled, setEnabledState] = useState(true);
  const [autoDate, setAutoDateState] = useState(true);
  const [carryOverDefault, setCarryOverDefaultState] =
    useState<CarryOverDefault>('ask');
  // Default '00:00' means "as soon as the local date rolls over",
  // which is what users of always-on PCs expect.
  const [dayStartTrigger, setDayStartTriggerState] =
    useState<DayStartTrigger>('00:00');
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
      getUserPref(LIST_OVERRIDES_KEY).catch(() => null),
    ])
      .then(
        ([cascadeRaw, autoDateRaw, carryOverRaw, triggerRaw, listOverridesRaw]) => {
          if (cancelled) return;
          // Cascade + auto-date follow the same on/off convention as
          // before — only literal "false" toggles the default off.
          if (cascadeRaw === 'false') setEnabledState(false);
          if (autoDateRaw === 'false') setAutoDateState(false);
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

  // ── Debounced persistence, one timer per knob ─────────────────────
  //
  // A separate timer per field keeps the writes scoped — flipping the
  // cascade doesn't flush a stale auto-date write, etc. The pattern
  // matches the original single-field implementation, just repeated.

  const cascadeTimer = useRef<number | null>(null);
  useEffect(() => {
    if (hydrating) return;
    if (cascadeTimer.current !== null) {
      window.clearTimeout(cascadeTimer.current);
    }
    cascadeTimer.current = window.setTimeout(() => {
      void setUserPref(CASCADE_KEY, enabled ? 'true' : 'false');
    }, WRITE_DEBOUNCE_MS);
    return () => {
      if (cascadeTimer.current !== null) {
        window.clearTimeout(cascadeTimer.current);
        cascadeTimer.current = null;
      }
    };
  }, [enabled, hydrating]);

  const autoDateTimer = useRef<number | null>(null);
  useEffect(() => {
    if (hydrating) return;
    if (autoDateTimer.current !== null) {
      window.clearTimeout(autoDateTimer.current);
    }
    autoDateTimer.current = window.setTimeout(() => {
      void setUserPref(AUTO_DATE_KEY, autoDate ? 'true' : 'false');
    }, WRITE_DEBOUNCE_MS);
    return () => {
      if (autoDateTimer.current !== null) {
        window.clearTimeout(autoDateTimer.current);
        autoDateTimer.current = null;
      }
    };
  }, [autoDate, hydrating]);

  const carryOverTimer = useRef<number | null>(null);
  useEffect(() => {
    if (hydrating) return;
    if (carryOverTimer.current !== null) {
      window.clearTimeout(carryOverTimer.current);
    }
    carryOverTimer.current = window.setTimeout(() => {
      void setUserPref(CARRY_OVER_KEY, carryOverDefault);
    }, WRITE_DEBOUNCE_MS);
    return () => {
      if (carryOverTimer.current !== null) {
        window.clearTimeout(carryOverTimer.current);
        carryOverTimer.current = null;
      }
    };
  }, [carryOverDefault, hydrating]);

  const dayStartTriggerTimer = useRef<number | null>(null);
  useEffect(() => {
    if (hydrating) return;
    if (dayStartTriggerTimer.current !== null) {
      window.clearTimeout(dayStartTriggerTimer.current);
    }
    dayStartTriggerTimer.current = window.setTimeout(() => {
      void setUserPref(DAY_START_TRIGGER_KEY, dayStartTrigger);
    }, WRITE_DEBOUNCE_MS);
    return () => {
      if (dayStartTriggerTimer.current !== null) {
        window.clearTimeout(dayStartTriggerTimer.current);
        dayStartTriggerTimer.current = null;
      }
    };
  }, [dayStartTrigger, hydrating]);

  const listOverridesTimer = useRef<number | null>(null);
  useEffect(() => {
    if (hydrating) return;
    if (listOverridesTimer.current !== null) {
      window.clearTimeout(listOverridesTimer.current);
    }
    listOverridesTimer.current = window.setTimeout(() => {
      // Empty map serialises to "{}" — still a valid value, but we
      // don't need to keep an empty pref hanging around. Storing
      // the empty object is harmless either way; keep it simple.
      void setUserPref(LIST_OVERRIDES_KEY, JSON.stringify(listOverrides));
    }, WRITE_DEBOUNCE_MS);
    return () => {
      if (listOverridesTimer.current !== null) {
        window.clearTimeout(listOverridesTimer.current);
        listOverridesTimer.current = null;
      }
    };
  }, [listOverrides, hydrating]);

  const setEnabled = useCallback((value: boolean) => {
    setEnabledState(value);
  }, []);
  const setAutoDate = useCallback((value: boolean) => {
    setAutoDateState(value);
  }, []);
  const setCarryOverDefault = useCallback((value: CarryOverDefault) => {
    setCarryOverDefaultState(value);
  }, []);
  const setDayStartTrigger = useCallback((value: DayStartTrigger) => {
    setDayStartTriggerState(value);
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
      carryOverDefault,
      setCarryOverDefault,
      dayStartTrigger,
      setDayStartTrigger,
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
      carryOverDefault,
      setCarryOverDefault,
      dayStartTrigger,
      setDayStartTrigger,
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

/**
 * Read every task preference plus the hydration flag. Existing
 * consumers that destructure `{ enabled }` continue to work; new
 * consumers can pull `autoDate` and `carryOverDefault` from the same
 * call.
 */
export function useTaskCascadeEnabled(): TaskCascadeContextValue {
  const ctx = useContext(TaskCascadeContext);
  if (!ctx) {
    throw new Error(
      'useTaskCascadeEnabled must be used inside <TaskCascadeProvider>',
    );
  }
  return ctx;
}
