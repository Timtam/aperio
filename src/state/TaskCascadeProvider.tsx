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
const WRITE_DEBOUNCE_MS = 150;

export type CarryOverDefault = 'ask' | 'today' | 'backlog';

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
  const [hydrating, setHydrating] = useState(true);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      getUserPref(CASCADE_KEY).catch(() => null),
      getUserPref(AUTO_DATE_KEY).catch(() => null),
      getUserPref(CARRY_OVER_KEY).catch(() => null),
    ])
      .then(([cascadeRaw, autoDateRaw, carryOverRaw]) => {
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
      })
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

  const setEnabled = useCallback((value: boolean) => {
    setEnabledState(value);
  }, []);
  const setAutoDate = useCallback((value: boolean) => {
    setAutoDateState(value);
  }, []);
  const setCarryOverDefault = useCallback((value: CarryOverDefault) => {
    setCarryOverDefaultState(value);
  }, []);

  const value = useMemo<TaskCascadeContextValue>(
    () => ({
      enabled,
      setEnabled,
      autoDate,
      setAutoDate,
      carryOverDefault,
      setCarryOverDefault,
      hydrating,
    }),
    [
      enabled,
      setEnabled,
      autoDate,
      setAutoDate,
      carryOverDefault,
      setCarryOverDefault,
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
