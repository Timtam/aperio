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
 * Global "couple parent and subtask status" preference.
 *
 * When enabled (the default) the cascade planners in `taskCascade.ts`
 * propagate status changes between a task and its descendants /
 * ancestors per the rules documented there. Users can opt out from
 * the Settings dialog's Tasks tab; the planners then degrade to a
 * single-row write or a no-op.
 *
 * Lives in a context so the (rare) flip doesn't make every consumer
 * re-fetch from `user_prefs` — `useTaskStatusToggle`, `TaskDialog`
 * and the settings panel all share one snapshot.
 *
 * Storage: a single string under the `tasks.cascadeStatusCoupling`
 * key in `user_prefs`. Default for missing keys: `enabled`. Only the
 * literal value `"false"` is treated as "opt out"; anything else
 * (including the explicit `"true"` we write back) means enabled.
 */

const PREF_KEY = 'tasks.cascadeStatusCoupling';
const WRITE_DEBOUNCE_MS = 150;

interface TaskCascadeContextValue {
  /** True when parent/subtask status coupling is active. */
  enabled: boolean;
  /** Set the preference and persist asynchronously. */
  setEnabled: (value: boolean) => void;
  /** True until the initial hydration round-trip returns. */
  hydrating: boolean;
}

const TaskCascadeContext = createContext<TaskCascadeContextValue | null>(null);

export function TaskCascadeProvider({ children }: { children: ReactNode }) {
  // Default-on so first paint behaves correctly even before the
  // user_prefs round-trip resolves.
  const [enabled, setEnabledState] = useState(true);
  const [hydrating, setHydrating] = useState(true);

  useEffect(() => {
    let cancelled = false;
    getUserPref(PREF_KEY)
      .then((raw) => {
        if (cancelled) return;
        // Treat anything other than the literal "false" string as
        // enabled — keeps the format trivial to inspect by hand and
        // forward-compatible with future option values.
        if (raw === 'false') setEnabledState(false);
      })
      .catch(() => {
        // Backend unreachable during init; default-on is the safe
        // fallback (the cascade is the documented behaviour).
      })
      .finally(() => {
        if (!cancelled) setHydrating(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Debounced persistence — the checkbox can flip rapidly while the
  // user reads the hint, and there's no value in spamming SQLite.
  const writeTimer = useRef<number | null>(null);
  useEffect(() => {
    if (hydrating) return;
    if (writeTimer.current !== null) {
      window.clearTimeout(writeTimer.current);
    }
    writeTimer.current = window.setTimeout(() => {
      void setUserPref(PREF_KEY, enabled ? 'true' : 'false');
    }, WRITE_DEBOUNCE_MS);
    return () => {
      if (writeTimer.current !== null) {
        window.clearTimeout(writeTimer.current);
        writeTimer.current = null;
      }
    };
  }, [enabled, hydrating]);

  const setEnabled = useCallback((value: boolean) => {
    setEnabledState(value);
  }, []);

  const value = useMemo<TaskCascadeContextValue>(
    () => ({ enabled, setEnabled, hydrating }),
    [enabled, setEnabled, hydrating],
  );

  return (
    <TaskCascadeContext.Provider value={value}>
      {children}
    </TaskCascadeContext.Provider>
  );
}

/**
 * Read the cascade-coupling preference. Throws when called outside
 * the provider — a wiring bug, never a runtime condition.
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
